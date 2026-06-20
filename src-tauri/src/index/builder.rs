//! Index builder: walk source folders, decode text files, and write them
//! into a Tantivy index.
//!
//! Heavy work (reading + binary detection + encoding decode) is done in
//! parallel with rayon; the resulting prepared documents are then added to
//! the `IndexWriter` sequentially so progress can be reported simply.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use rayon::prelude::*;
use tantivy::directory::MmapDirectory;
use tantivy::schema::Schema;
use tantivy::{doc, Index, IndexWriter};

use super::schema::build_schema;

/// File extensions treated as plain text and eligible for indexing.
pub const DEFAULT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "rst", "csv", "tsv", "log", "json", "jsonl", "ndjson", "xml", "yaml",
    "yml", "toml", "ini", "cfg", "conf", "env", "properties", "rs", "py", "js", "mjs", "cjs", "ts",
    "tsx", "jsx", "java", "kt", "c", "h", "cpp", "cc", "hpp", "cs", "go", "rb", "php", "swift",
    "scala", "sql", "sh", "bash", "zsh", "ps1", "bat", "html", "htm", "css", "scss", "less", "vue",
    "svelte", "tex", "org", "asciidoc", "adoc",
];

/// Default per-file size cap (20 MB). Larger files are skipped.
const DEFAULT_MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// Heap budget for the index writer (50 MB).
const WRITER_HEAP_BYTES: usize = 50_000_000;

/// Number of leading bytes inspected by the binary-file heuristic.
const BINARY_SNIFF_BYTES: usize = 8192;

/// Tunable options for an index build.
pub struct BuildOptions {
    /// Lowercase extensions (without the dot) that are eligible for indexing.
    pub extensions: Vec<String>,
    /// Files larger than this many bytes are skipped.
    pub max_file_bytes: u64,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            extensions: DEFAULT_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }
}

/// Progress update emitted while documents are written.
#[derive(Clone, Copy, Debug)]
pub struct BuildProgress {
    pub indexed: usize,
    pub total: usize,
}

/// Summary of a completed build.
#[derive(Clone, Copy, Debug)]
pub struct BuildStats {
    /// Number of documents written to the index.
    pub indexed: usize,
    /// Files seen during the walk but not indexed (wrong type, too big,
    /// binary, or unreadable).
    pub skipped: usize,
    /// Total files encountered during the folder walk.
    pub files_seen: usize,
}

/// A file that has been read and decoded, ready to be added to the index.
struct PreparedDoc {
    path: String,
    filename: String,
    ext: String,
    content: String,
    modified: i64,
    size: u64,
}

/// Build (full rebuild) an index at `index_dir` from the given source folders.
///
/// Any existing documents in the index are cleared first, so calling this
/// repeatedly is idempotent. Incremental rebuilds come in a later milestone.
pub fn build_index(
    index_dir: &Path,
    source_folders: &[PathBuf],
    opts: &BuildOptions,
    mut progress: impl FnMut(BuildProgress),
) -> Result<BuildStats> {
    let (schema, fields) = build_schema();
    let index = open_or_create(index_dir, &schema)?;
    let mut writer: IndexWriter = index.writer(WRITER_HEAP_BYTES)?;

    // Full rebuild: drop everything currently in the index.
    writer.delete_all_documents()?;

    // Walk all folders and collect candidate files.
    let files = collect_files(source_folders);
    let files_seen = files.len();

    // Read + decode in parallel; files that should be skipped become `None`.
    let prepared: Vec<PreparedDoc> = files
        .par_iter()
        .filter_map(|path| match prepare_file(path, opts) {
            Ok(Some(doc)) => Some(doc),
            _ => None,
        })
        .collect();

    let total = prepared.len();
    progress(BuildProgress { indexed: 0, total });

    // Add prepared docs sequentially so progress is straightforward to report.
    for (i, pd) in prepared.iter().enumerate() {
        writer.add_document(doc!(
            fields.path => pd.path.clone(),
            fields.filename => pd.filename.clone(),
            fields.ext => pd.ext.clone(),
            fields.content => pd.content.clone(),
            fields.modified => pd.modified,
            fields.size => pd.size,
        ))?;
        progress(BuildProgress {
            indexed: i + 1,
            total,
        });
    }

    writer.commit()?;

    Ok(BuildStats {
        indexed: total,
        skipped: files_seen - total,
        files_seen,
    })
}

/// Open an existing index at `index_dir`, or create a new one there.
fn open_or_create(index_dir: &Path, schema: &Schema) -> Result<Index> {
    fs::create_dir_all(index_dir)
        .with_context(|| format!("creating index dir {}", index_dir.display()))?;
    let dir = MmapDirectory::open(index_dir)
        .with_context(|| format!("opening index dir {}", index_dir.display()))?;
    let index = Index::open_or_create(dir, schema.clone()).context("opening Tantivy index")?;
    Ok(index)
}

/// Recursively collect every regular file under the given folders.
///
/// Respects `.gitignore` and skips hidden VCS dirs by default, which keeps
/// noise like `.git/` out of the index.
fn collect_files(folders: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for folder in folders {
        for result in WalkBuilder::new(folder).hidden(false).build() {
            if let Ok(entry) = result {
                if entry.file_type().map_or(false, |ft| ft.is_file()) {
                    out.push(entry.into_path());
                }
            }
        }
    }
    out
}

/// Read and decode a single file, returning `None` if it should be skipped.
fn prepare_file(path: &Path, opts: &BuildOptions) -> Result<Option<PreparedDoc>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if !opts.extensions.iter().any(|e| e == &ext) {
        return Ok(None);
    }

    let meta = fs::metadata(path)?;
    let size = meta.len();
    if size > opts.max_file_bytes {
        return Ok(None);
    }

    let modified = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let bytes = fs::read(path)?;
    if looks_binary(&bytes) {
        return Ok(None);
    }

    let content = decode_text(&bytes);
    let filename = path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("")
        .to_string();

    Ok(Some(PreparedDoc {
        path: path.to_string_lossy().into_owned(),
        filename,
        ext,
        content,
        modified,
        size,
    }))
}

/// Heuristic: a NUL byte in the first chunk almost always means binary data.
fn looks_binary(bytes: &[u8]) -> bool {
    let n = bytes.len().min(BINARY_SNIFF_BYTES);
    bytes[..n].contains(&0)
}

/// Decode bytes to a `String`, detecting the encoding (UTF-8, UTF-16,
/// legacy code pages, …) with chardetng and decoding with encoding_rs.
fn decode_text(bytes: &[u8]) -> String {
    // We index trusted local files (not web content), so we allow UTF-8 and
    // ISO-2022-JP as detection results for the widest coverage.
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
    detector.feed(bytes, true);
    let encoding = detector.guess(None, chardetng::Utf8Detection::Allow);
    let (decoded, _, _) = encoding.decode(bytes);
    decoded.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tantivy::collector::TopDocs;
    use tantivy::query::QueryParser;

    fn write(dir: &Path, name: &str, contents: &[u8]) {
        fs::write(dir.join(name), contents).unwrap();
    }

    #[test]
    fn builds_index_and_finds_terms() {
        let src = tempfile::tempdir().unwrap();
        let idx = tempfile::tempdir().unwrap();

        write(src.path(), "a.txt", b"the quick brown fox");
        write(src.path(), "b.md", b"lazy dog jumps over");
        // Allowed extension but contains NUL bytes -> skipped as binary.
        write(src.path(), "c.txt", b"binary\0\0\0data fox");
        // Disallowed extension -> skipped even though it contains "fox".
        write(src.path(), "d.png", b"not indexed ext fox");

        let stats = build_index(
            idx.path(),
            &[src.path().to_path_buf()],
            &BuildOptions::default(),
            |_| {},
        )
        .unwrap();

        assert_eq!(stats.indexed, 2, "only a.txt and b.md should be indexed");
        assert_eq!(stats.files_seen, 4);
        assert_eq!(stats.skipped, 2);

        // The index should contain exactly the two indexed docs.
        let (_schema, fields) = build_schema();
        let index = Index::open_in_dir(idx.path()).unwrap();
        let searcher = index.reader().unwrap().searcher();
        assert_eq!(searcher.num_docs(), 2);

        // "fox" only survives in a.txt (the other two files were skipped).
        let qp = QueryParser::for_index(&index, vec![fields.content]);
        let query = qp.parse_query("fox").unwrap();
        let hits = searcher
            .search(&query, &TopDocs::with_limit(10).order_by_score())
            .unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn respects_size_cap() {
        let src = tempfile::tempdir().unwrap();
        let idx = tempfile::tempdir().unwrap();

        write(src.path(), "big.txt", &vec![b'a'; 2048]);
        write(src.path(), "small.txt", b"tiny");

        let opts = BuildOptions {
            max_file_bytes: 100,
            ..BuildOptions::default()
        };
        let stats = build_index(idx.path(), &[src.path().to_path_buf()], &opts, |_| {}).unwrap();
        assert_eq!(stats.indexed, 1, "big.txt exceeds the cap and is skipped");
    }

    #[test]
    fn rebuild_is_idempotent() {
        let src = tempfile::tempdir().unwrap();
        let idx = tempfile::tempdir().unwrap();
        write(src.path(), "a.txt", b"hello world");

        let folders = [src.path().to_path_buf()];
        build_index(idx.path(), &folders, &BuildOptions::default(), |_| {}).unwrap();
        let stats = build_index(idx.path(), &folders, &BuildOptions::default(), |_| {}).unwrap();

        assert_eq!(stats.indexed, 1);
        let index = Index::open_in_dir(idx.path()).unwrap();
        let searcher = index.reader().unwrap().searcher();
        assert_eq!(searcher.num_docs(), 1, "rebuild must not duplicate docs");
    }
}
