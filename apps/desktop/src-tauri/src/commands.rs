//! Tauri commands: thin wrappers over the scan manager and the core library. Payloads
//! are the camelCase types of `views.rs`; errors are strings for the UI.

use std::path::PathBuf;
use std::sync::Arc;

use storage_monitor_core::disk::{self, DiskUsage};
use storage_monitor_core::paths;
use storage_monitor_core::scan::{NodeId, Tree};
use storage_monitor_core::snapshot::Delta;
use tauri::{AppHandle, State};

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

/// The folders that grew the most since the previous snapshot, at most `limit` (default: 10).
#[tauri::command]
pub fn top_growers(manager: State<'_, ScanManager>, limit: Option<usize>) -> Vec<Delta> {
    manager.growers(limit.unwrap_or(DEFAULT_GROWERS_LIMIT))
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
