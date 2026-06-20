//! Shared application state managed by Tauri.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::index::IndexStore;

/// Process-wide state available to every command.
pub struct AppState {
    /// Root directory holding all index data and the `indexes.json` manifest.
    root: PathBuf,
    /// Serializes index mutations (create/build/delete) so concurrent manifest
    /// writes don't clobber each other. Read-only commands (list/search) do not
    /// take this lock, so searching stays responsive during a build.
    pub write_lock: Mutex<()>,
}

impl AppState {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            write_lock: Mutex::new(()),
        }
    }

    /// Open a store handle rooted at the app data directory. Cheap: `IndexStore`
    /// just holds the root path and re-reads the manifest per call.
    pub fn store(&self) -> anyhow::Result<IndexStore> {
        IndexStore::new(self.root.clone())
    }
}
