//! Index builder: walk source folders, decode text files, and write them
//! into a Tantivy index.
//!
//! Heavy work (reading + binary detection + encoding decode) is done in
//! parallel with rayon; the resulting prepared documents are then added to
//! the `IndexWriter` sequentially so progress can be reported simply.

use std::collections::{HashMap, HashSet};
use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use rayon::prelude::*;
use tantivy::directory::MmapDirectory;
use tantivy::schema::{Field, Schema, Value};
use tantivy::{doc, Index, IndexWriter, TantivyDocument, Term};

use super::schema::{build_schema, Fields};

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
#[derive(Clone, Copy, Debug, Default)]
pub struct BuildStats {
    /// Documents added or updated this run.
    pub indexed: usize,
    /// Files seen during the walk but not indexed (wrong type, too big,
    /// binary, or unreadable).
    pub skipped: usize,
    /// Total files encountered during the folder walk.
    pub files_seen: usize,
    /// Incremental only: files already up to date and left untouched.
    pub unchanged: usize,
    /// Incremental only: documents removed because their file is gone.
    pub removed: usize,
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
        writer.add_document(make_doc(&fields, pd))?;
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
        ..Default::default()
    })
}

/// Incrementally update an index at `index_dir`: (re)index new and changed
/// files, leave unchanged files alone, and remove documents whose files no
/// longer exist. Works on a fresh (empty) index too, in which case every
/// eligible file is treated as new.
///
/// "Changed" is decided by comparing each file's modified time (millis) to
/// the value stored in the index, so unchanged files are never re-read.
pub fn update_index(
    index_dir: &Path,
    source_folders: &[PathBuf],
    opts: &BuildOptions,
    mut progress: impl FnMut(BuildProgress),
) -> Result<BuildStats> {
    let (schema, fields) = build_schema();
    let index = open_or_create(index_dir, &schema)?;
    let existing = read_existing(&index, &fields)?;
    let mut writer: IndexWriter = index.writer(WRITER_HEAP_BYTES)?;

    let files = collect_files(source_folders);
    let files_seen = files.len();

    // Classify every file in parallel; only changed files are read + decoded.
    let items: Vec<Item> = files
        .par_iter()
        .map(|path| classify(path, opts, &existing))
        .collect();

    let total = items
        .iter()
        .filter(|it| matches!(it.kind, ItemKind::Changed(Some(_))))
        .count();
    progress(BuildProgress { indexed: 0, total });

    let mut present: HashSet<&str> = HashSet::with_capacity(items.len());
    let mut stats = BuildStats {
        files_seen,
        ..Default::default()
    };

    for item in &items {
        match &item.kind {
            ItemKind::Ineligible => stats.skipped += 1,
            ItemKind::Unchanged => {
                present.insert(item.path.as_str());
                stats.unchanged += 1;
            }
            ItemKind::Changed(prepared) => {
                present.insert(item.path.as_str());
                // Replace any previous version of this file.
                writer.delete_term(path_term(fields.path, &item.path));
                match prepared {
                    Some(pd) => {
                        writer.add_document(make_doc(&fields, pd))?;
                        stats.indexed += 1;
                        progress(BuildProgress {
                            indexed: stats.indexed,
                            total,
                        });
                    }
                    // Was eligible by metadata but turned out binary/unreadable;
                    // the delete_term above drops any stale prior version.
                    None => stats.skipped += 1,
                }
            }
        }
    }

    // Remove documents whose files are gone or no longer eligible.
    for path in existing.keys() {
        if !present.contains(path.as_str()) {
            writer.delete_term(path_term(fields.path, path));
            stats.removed += 1;
        }
    }

    writer.commit()?;
    Ok(stats)
}

/// Classification of a single walked file for an incremental update.
struct Item {
    path: String,
    kind: ItemKind,
}

enum ItemKind {
    /// Wrong extension, too large, or unreadable metadata.
    Ineligible,
    /// Already in the index with a matching modified time.
    Unchanged,
    /// New or modified; `Some` if it read cleanly, `None` if binary/unreadable.
    Changed(Option<PreparedDoc>),
}

/// Decide what to do with a file: only read it if it's new or changed.
fn classify(path: &Path, opts: &BuildOptions, existing: &HashMap<String, i64>) -> Item {
    let path_str = path.to_string_lossy().into_owned();
    let ext = file_ext(path);
    if !opts.extensions.iter().any(|e| e == &ext) {
        return Item {
            path: path_str,
            kind: ItemKind::Ineligible,
        };
    }
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => {
            return Item {
                path: path_str,
                kind: ItemKind::Ineligible,
            }
        }
    };
    let size = meta.len();
    if size > opts.max_file_bytes {
        return Item {
            path: path_str,
            kind: ItemKind::Ineligible,
        };
    }
    let modified = file_mtime_millis(&meta);
    if existing.get(&path_str) == Some(&modified) {
        return Item {
            path: path_str,
            kind: ItemKind::Unchanged,
        };
    }
    let prepared = read_prepared(path, path_str.clone(), ext, size, modified);
    Item {
        path: path_str,
        kind: ItemKind::Changed(prepared),
    }
}

/// Read every live document's path + modified time from the index.
fn read_existing(index: &Index, fields: &Fields) -> Result<HashMap<String, i64>> {
    // Manual reload avoids spawning a watcher thread that would keep the
    // index alive (and its mmaps open) after this function returns.
    let reader = index
        .reader_builder()
        .reload_policy(tantivy::ReloadPolicy::Manual)
        .try_into()?;
    let searcher = reader.searcher();
    let mut map = HashMap::new();
    for segment in searcher.segment_readers() {
        let store = segment.get_store_reader(0)?;
        for doc_id in segment.doc_ids_alive() {
            let doc: TantivyDocument = store.get(doc_id)?;
            let path = doc
                .get_first(fields.path)
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if path.is_empty() {
                continue;
            }
            let modified = doc
                .get_first(fields.modified)
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            map.insert(path.to_string(), modified);
        }
    }
    Ok(map)
}

/// Build the indexable document for a prepared file.
fn make_doc(fields: &Fields, pd: &PreparedDoc) -> TantivyDocument {
    doc!(
        fields.path => pd.path.clone(),
        fields.filename => pd.filename.clone(),
        fields.ext => pd.ext.clone(),
        fields.content => pd.content.clone(),
        fields.modified => pd.modified,
        fields.size => pd.size,
    )
}

/// The exact-match term used to find/delete a document by its path.
fn path_term(path_field: Field, path: &str) -> Term {
    Term::from_field_text(path_field, path)
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
/// Used by the full-rebuild path; the incremental path uses `classify`.
fn prepare_file(path: &Path, opts: &BuildOptions) -> Result<Option<PreparedDoc>> {
    let ext = file_ext(path);
    if !opts.extensions.iter().any(|e| e == &ext) {
        return Ok(None);
    }
    let meta = fs::metadata(path)?;
    let size = meta.len();
    if size > opts.max_file_bytes {
        return Ok(None);
    }
    let modified = file_mtime_millis(&meta);
    Ok(read_prepared(
        path,
        path.to_string_lossy().into_owned(),
        ext,
        size,
        modified,
    ))
}

/// Lowercased file extension (without the dot), or "" if none.
fn file_ext(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// File modified time as unix milliseconds (0 if unavailable). Milliseconds
/// give enough resolution to detect edits between consecutive rebuilds.
fn file_mtime_millis(meta: &Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Read + binary-check + decode a file already known to be eligible by
/// extension and size. Returns `None` if it reads as binary or errors.
fn read_prepared(
    path: &Path,
    path_str: String,
    ext: String,
    size: u64,
    modified: i64,
) -> Option<PreparedDoc> {
    let bytes = fs::read(path).ok()?;
    if looks_binary(&bytes) {
        return None;
    }
    let content = decode_text(&bytes);
    let filename = path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("")
        .to_string();
    Some(PreparedDoc {
        path: path_str,
        filename,
        ext,
        content,
        modified,
        size,
    })
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

    #[test]
    fn incremental_add_modify_remove() {
        use filetime::{set_file_mtime, FileTime};

        let src = tempfile::tempdir().unwrap();
        let idx = tempfile::tempdir().unwrap();
        let folders = [src.path().to_path_buf()];
        let opts = BuildOptions::default();

        write(src.path(), "a.txt", b"alpha content");
        write(src.path(), "b.txt", b"beta content");

        // First update on an empty index = full build.
        let s = update_index(idx.path(), &folders, &opts, |_| {}).unwrap();
        assert_eq!((s.indexed, s.unchanged, s.removed), (2, 0, 0));

        // No changes -> everything is left untouched.
        let s = update_index(idx.path(), &folders, &opts, |_| {}).unwrap();
        assert_eq!((s.indexed, s.unchanged, s.removed), (0, 2, 0));

        // Edit a.txt and bump its mtime -> only a.txt is re-indexed.
        write(src.path(), "a.txt", b"alpha updated content");
        set_file_mtime(
            src.path().join("a.txt"),
            FileTime::from_unix_time(2_000_000_000, 0),
        )
        .unwrap();
        let s = update_index(idx.path(), &folders, &opts, |_| {}).unwrap();
        assert_eq!((s.indexed, s.unchanged, s.removed), (1, 1, 0));

        // Add c.txt, delete b.txt -> one add, one removal.
        write(src.path(), "c.txt", b"gamma content");
        fs::remove_file(src.path().join("b.txt")).unwrap();
        let s = update_index(idx.path(), &folders, &opts, |_| {}).unwrap();
        assert_eq!((s.indexed, s.removed), (1, 1));

        // Final state: a.txt + c.txt only.
        let index = Index::open_in_dir(idx.path()).unwrap();
        assert_eq!(index.reader().unwrap().searcher().num_docs(), 2);
    }
}
