//! Tauri commands: thin wrappers over the scan manager and the core library. Payloads
//! are the camelCase types of `views.rs`; errors are strings for the UI.

use std::path::PathBuf;
use std::sync::Arc;

use storage_monitor_core::action::{ActionLog, Mode, Outcome, Preview};
use storage_monitor_core::disk::{self, DiskUsage};
use storage_monitor_core::paths;
use storage_monitor_core::scan::{NodeId, Tree};
use storage_monitor_core::snapshot::Delta;
use storage_monitor_core::system::RealSystem;
use tauri::{AppHandle, State};

use crate::actions;
use crate::scan_manager::ScanManager;
use crate::views::{NodeView, ScanStatus};

const DEFAULT_CHILDREN_LIMIT: usize = 500;
const DEFAULT_GROWERS_LIMIT: usize = 10;

fn home() -> Result<PathBuf, String> {
    paths::home_dir().ok_or_else(|| "cannot determine the home folder".to_owned())
}

/// The folder scanned when the UI does not pick one.
#[tauri::command]
pub fn default_root() -> Result<String, String> {
    home().map(|home| home.to_string_lossy().into_owned())
}

/// Starts a scan of `root` (default: the home folder); progress arrives as events.
#[tauri::command]
pub fn scan_start(
    app: AppHandle,
    manager: State<'_, ScanManager>,
    root: Option<String>,
) -> Result<ScanStatus, String> {
    let root = match root {
        Some(root) => PathBuf::from(root),
        None => home()?,
    };
    manager.start(Arc::new(app), root)
}

#[tauri::command]
pub fn scan_status(manager: State<'_, ScanManager>) -> ScanStatus {
    manager.status()
}

#[tauri::command]
pub fn scan_cancel(manager: State<'_, ScanManager>) -> ScanStatus {
    manager.cancel()
}

/// One page of the last scan's tree: the node `id` (default: the root) with up to
/// `limit` children (default: 500).
#[tauri::command]
pub fn tree_node(
    manager: State<'_, ScanManager>,
    id: Option<NodeId>,
    limit: Option<usize>,
) -> Result<NodeView, String> {
    let id = id.unwrap_or(Tree::ROOT);
    let limit = limit.unwrap_or(DEFAULT_CHILDREN_LIMIT);
    manager
        .with_result(|result, previous| NodeView::build(&result.tree, id, limit, previous))
        .ok_or_else(|| "no scan result".to_owned())?
        .ok_or_else(|| format!("unknown node {id}"))
}

/// Usage of the volume holding `path` (default: the scan root, else the home folder).
#[tauri::command]
pub fn disk_usage(
    manager: State<'_, ScanManager>,
    path: Option<String>,
) -> Result<DiskUsage, String> {
    let path = match path {
        Some(path) => PathBuf::from(path),
        None => match manager.root() {
            Some(root) => root,
            None => home()?,
        },
    };
    disk::disk_usage(&path)
        .map_err(|err| format!("cannot read the volume of {}: {err}", path.display()))
}

/// The folders that grew the most since the previous snapshot, at most `limit` (default:
/// 10). The manager keeps only the [`GROWERS_KEPT`](crate::scan_manager::GROWERS_KEPT)
/// largest growers of a scan, so no `limit` returns more than 50.
#[tauri::command]
pub fn top_growers(manager: State<'_, ScanManager>, limit: Option<usize>) -> Vec<Delta> {
    manager.growers(limit.unwrap_or(DEFAULT_GROWERS_LIMIT))
}

/// What deleting `paths` would do, without touching anything: one row per path, with the
/// guards' verdict on it.
#[tauri::command]
pub async fn action_preview(
    manager: State<'_, ScanManager>,
    sys: State<'_, RealSystem>,
    paths: Vec<String>,
    mode: Mode,
) -> Result<Preview, String> {
    let (manager, sys) = (manager.inner().clone(), sys.inner().clone());
    let paths = to_paths(paths);
    off_the_event_loop(move || actions::preview_batch(&manager, &sys, &paths, mode)).await
}

/// Deletes `paths`, records the batch and patches the tree.
///
/// Takes paths and a mode, never a finished preview: a preview that came over the wire is a
/// claim, and the guards run here on this side of it.
#[tauri::command]
pub async fn action_run(
    manager: State<'_, ScanManager>,
    sys: State<'_, RealSystem>,
    log: State<'_, ActionLog>,
    paths: Vec<String>,
    mode: Mode,
) -> Result<Outcome, String> {
    let (manager, sys, log) = (
        manager.inner().clone(),
        sys.inner().clone(),
        log.inner().clone(),
    );
    let paths = to_paths(paths);
    off_the_event_loop(move || actions::run_batch(&manager, &sys, &log, paths, mode)).await
}

fn to_paths(paths: Vec<String>) -> Vec<PathBuf> {
    paths.into_iter().map(PathBuf::from).collect()
}

/// Runs `body` on a blocking thread, so the window keeps painting while a batch deletes and
/// walks the disk — seconds of work, where every other command is a lookup. A panic in it,
/// and the rebuild of the arena carries a live assertion, comes back as an error for the
/// dialog instead of taking the window down.
async fn off_the_event_loop<T: Send + 'static>(
    body: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(body)
        .await
        .map_err(|err| format!("the batch did not finish: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_root_is_the_home_folder() {
        let root = default_root().unwrap();
        assert_eq!(Some(PathBuf::from(&root)), paths::home_dir());
        assert!(root.starts_with('/'), "{root}");
    }
}
