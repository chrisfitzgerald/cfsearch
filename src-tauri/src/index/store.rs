//! Named-index registry.
//!
//! Each index lives in its own subfolder under a store root, and a single
//! `indexes.json` manifest at the root records the set of indexes (name,
//! source folders, document count, last-built time). The desktop app points
//! the store at an app-data directory; tests point it at a temp dir.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use tantivy::Index;

use super::builder::{update_index, BuildOptions, BuildProgress, BuildStats};

const MANIFEST_FILE: &str = "indexes.json";

/// A registered index, as persisted in the manifest and shown to the UI.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IndexInfo {
    /// User-facing, unique name.
    pub name: String,
    /// Subfolder (relative to the store root) holding the Tantivy data.
    pub dir: String,
    /// Source folders indexed into this index.
    pub folders: Vec<String>,
    /// Live document count after the last build.
    pub doc_count: u64,
    /// Last successful build time, unix milliseconds.
    pub last_built: Option<i64>,
}

/// Manages the set of named indexes under a root directory.
pub struct IndexStore {
    root: PathBuf,
}

impl IndexStore {
    /// Open (creating if needed) a store rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)
            .with_context(|| format!("creating store root {}", root.display()))?;
        Ok(Self { root })
    }

    /// All registered indexes (empty if none yet).
    pub fn list(&self) -> Result<Vec<IndexInfo>> {
        let path = self.manifest_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let data =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&data).context("parsing index manifest")
    }

    /// Look up one index by name.
    pub fn get(&self, name: &str) -> Result<IndexInfo> {
        self.list()?
            .into_iter()
            .find(|i| i.name == name)
            .ok_or_else(|| anyhow!("no index named {name:?}"))
    }

    /// Absolute path to an index's Tantivy data directory.
    pub fn index_dir(&self, info: &IndexInfo) -> PathBuf {
        self.root.join(&info.dir)
    }

    /// Register a new (empty, not-yet-built) index.
    pub fn create(&self, name: &str, folders: Vec<String>) -> Result<IndexInfo> {
        let name = name.trim();
        if name.is_empty() {
            bail!("index name must not be empty");
        }
        let mut infos = self.list()?;
        if infos.iter().any(|i| i.name == name) {
            bail!("an index named {name:?} already exists");
        }
        let info = IndexInfo {
            name: name.to_string(),
            dir: self.unique_dir(&infos, name),
            folders,
            doc_count: 0,
            last_built: None,
        };
        infos.push(info.clone());
        self.save(&infos)?;
        Ok(info)
    }

    /// Incrementally (re)build an index and refresh its manifest entry.
    pub fn build(
        &self,
        name: &str,
        opts: &BuildOptions,
        progress: impl FnMut(BuildProgress),
    ) -> Result<(IndexInfo, BuildStats)> {
        let info = self.get(name)?;
        let dir = self.index_dir(&info);
        let folders: Vec<PathBuf> = info.folders.iter().map(PathBuf::from).collect();

        let stats = update_index(&dir, &folders, opts, progress)?;
        let doc_count = count_docs(&dir).unwrap_or(0);

        let mut infos = self.list()?;
        let updated = infos
            .iter_mut()
            .find(|i| i.name == name)
            .ok_or_else(|| anyhow!("index {name:?} vanished during build"))?;
        updated.doc_count = doc_count;
        updated.last_built = Some(now_millis());
        let updated = updated.clone();
        self.save(&infos)?;
        Ok((updated, stats))
    }

    /// Remove an index from the manifest and delete its data directory.
    pub fn delete(&self, name: &str) -> Result<()> {
        let mut infos = self.list()?;
        let pos = infos
            .iter()
            .position(|i| i.name == name)
            .ok_or_else(|| anyhow!("no index named {name:?}"))?;
        let info = infos.remove(pos);
        let dir = self.index_dir(&info);
        if dir.exists() {
            remove_dir_all_retry(&dir).with_context(|| format!("removing {}", dir.display()))?;
        }
        self.save(&infos)?;
        Ok(())
    }

    fn manifest_path(&self) -> PathBuf {
        self.root.join(MANIFEST_FILE)
    }

    fn save(&self, infos: &[IndexInfo]) -> Result<()> {
        let data = serde_json::to_string_pretty(infos)?;
        fs::write(self.manifest_path(), data).context("writing index manifest")
    }

    /// Pick a filesystem-safe, collision-free subfolder name for `name`.
    fn unique_dir(&self, infos: &[IndexInfo], name: &str) -> String {
        let base = slugify(name);
        let mut candidate = base.clone();
        let mut n = 2;
        while infos.iter().any(|i| i.dir == candidate) || self.root.join(&candidate).exists() {
            candidate = format!("{base}-{n}");
            n += 1;
        }
        candidate
    }
}

/// Convert an arbitrary name into a lowercase, dash-separated slug.
fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "index".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Live document count of the index stored at `index_dir`.
fn count_docs(index_dir: &Path) -> Result<u64> {
    let index = Index::open_in_dir(index_dir)?;
    let reader = index
        .reader_builder()
        .reload_policy(tantivy::ReloadPolicy::Manual)
        .try_into()?;
    Ok(reader.searcher().num_docs())
}

/// Remove a directory tree, retrying briefly. On Windows, memory-mapped index
/// files can take a moment to be released after readers drop, during which a
/// delete fails with "directory not empty".
fn remove_dir_all_retry(dir: &Path) -> std::io::Result<()> {
    use std::thread::sleep;
    use std::time::Duration;

    let mut last_err = None;
    for attempt in 0..5u64 {
        match fs::remove_dir_all(dir) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                sleep(Duration::from_millis(40 * (attempt + 1)));
            }
        }
    }
    Err(last_err.expect("loop runs at least once"))
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::SearchEngine;

    #[test]
    fn create_build_search_delete() {
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        fs::write(src.path().join("note.txt"), b"hello tantivy world").unwrap();

        let store = IndexStore::new(root.path()).unwrap();
        let info = store
            .create("My Notes", vec![src.path().to_string_lossy().into_owned()])
            .unwrap();
        assert_eq!(info.dir, "my-notes");
        assert_eq!(info.doc_count, 0);
        assert!(info.last_built.is_none());

        let (info, stats) = store
            .build("My Notes", &BuildOptions::default(), |_| {})
            .unwrap();
        assert_eq!(stats.indexed, 1);
        assert_eq!(info.doc_count, 1);
        assert!(info.last_built.is_some());

        // Manifest persisted the updated count.
        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].doc_count, 1);

        // Search works against the index dir.
        let engine = SearchEngine::open(&store.index_dir(&info)).unwrap();
        assert_eq!(engine.search("tantivy", 10).unwrap().len(), 1);

        // Drop the engine first: on Windows the open mmaps would otherwise
        // block deletion of the index directory.
        drop(engine);

        // Delete removes both the entry and the data directory.
        let dir = store.index_dir(&info);
        store.delete("My Notes").unwrap();
        assert!(store.list().unwrap().is_empty());
        assert!(!dir.exists());
    }

    #[test]
    fn duplicate_name_rejected() {
        let root = tempfile::tempdir().unwrap();
        let store = IndexStore::new(root.path()).unwrap();
        store.create("Docs", vec![]).unwrap();
        assert!(store.create("Docs", vec![]).is_err());
    }

    #[test]
    fn slugify_handles_messy_names() {
        assert_eq!(slugify("My Notes!"), "my-notes");
        assert_eq!(slugify("  C:/Work/2025  "), "c-work-2025");
        assert_eq!(slugify("***"), "index");
    }
}
