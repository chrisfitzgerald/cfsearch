//! Tauri IPC commands exposed to the web UI.

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_opener::OpenerExt;

use crate::index::{BuildOptions, IndexInfo};
use crate::search::{SearchEngine, SearchHit};
use crate::state::AppState;

/// Default number of search results returned when the UI doesn't specify.
const DEFAULT_SEARCH_LIMIT: usize = 50;

/// Progress payload emitted on the `build-progress` event during a build.
#[derive(Clone, Serialize)]
struct BuildProgressEvent {
    name: String,
    indexed: usize,
    total: usize,
}

/// List all registered indexes.
#[tauri::command]
pub fn list_indexes(state: State<AppState>) -> Result<Vec<IndexInfo>, String> {
    state.store().map_err(err)?.list().map_err(err)
}

/// Register a new (empty, not-yet-built) index from a set of source folders.
#[tauri::command]
pub fn create_index(
    state: State<AppState>,
    name: String,
    folders: Vec<String>,
) -> Result<IndexInfo, String> {
    let _guard = state.write_lock.lock().unwrap();
    state.store().map_err(err)?.create(&name, folders).map_err(err)
}

/// (Re)build an index, emitting `build-progress` events as documents are added.
#[tauri::command]
pub fn build_index(
    app: AppHandle,
    state: State<AppState>,
    name: String,
) -> Result<IndexInfo, String> {
    let _guard = state.write_lock.lock().unwrap();
    let store = state.store().map_err(err)?;
    let event_name = name.clone();
    let (info, _stats) = store
        .build(&name, &BuildOptions::default(), move |p| {
            let _ = app.emit(
                "build-progress",
                BuildProgressEvent {
                    name: event_name.clone(),
                    indexed: p.indexed,
                    total: p.total,
                },
            );
        })
        .map_err(err)?;
    Ok(info)
}

/// Delete an index and its on-disk data.
#[tauri::command]
pub fn delete_index(state: State<AppState>, name: String) -> Result<(), String> {
    let _guard = state.write_lock.lock().unwrap();
    state.store().map_err(err)?.delete(&name).map_err(err)
}

/// Search an index. A fresh engine is opened per call so no mmap is held open
/// between searches (which keeps deletes unblocked on Windows).
#[tauri::command]
pub fn search(
    state: State<AppState>,
    index: String,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<SearchHit>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let store = state.store().map_err(err)?;
    let info = store.get(&index).map_err(err)?;
    let engine = SearchEngine::open(&store.index_dir(&info)).map_err(err)?;
    engine
        .search(&query, limit.unwrap_or(DEFAULT_SEARCH_LIMIT))
        .map_err(err)
}

/// Open a file in its default application.
#[tauri::command]
pub fn open_path(app: AppHandle, path: String) -> Result<(), String> {
    app.opener().open_path(path, None::<String>).map_err(err)
}

/// Reveal a file in the system file manager (Explorer/Finder).
#[tauri::command]
pub fn reveal_path(app: AppHandle, path: String) -> Result<(), String> {
    app.opener().reveal_item_in_dir(path).map_err(err)
}

/// Map any displayable error to a string for the IPC boundary.
fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}
