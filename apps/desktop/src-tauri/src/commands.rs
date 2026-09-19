//! Tauri commands: thin wrappers over the scan manager and the core library. Payloads
//! are the camelCase types of `views.rs`; errors are strings for the UI.

use std::path::PathBuf;
use std::sync::Arc;

use storage_monitor_core::action::{ActionLog, LogTail, Mode, Preview};
use storage_monitor_core::disk::{self, DiskUsage};
use storage_monitor_core::paths;
use storage_monitor_core::scan::{NodeId, Tree};
use storage_monitor_core::snapshot::Delta;
use storage_monitor_core::system::RealSystem;
use tauri::{AppHandle, Runtime, State};

use crate::actions::{self, BatchLock};
use crate::scan_manager::ScanManager;
use crate::views::{BatchResult, NodeView, ScanStatus};

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
///
/// Generic over the runtime, as every command reached through a generic
/// [`configure`](crate::configure) has to be: the handle is the event sink, and the mock
/// runtime the registration test builds over has a handle of its own.
#[tauri::command]
pub fn scan_start<R: Runtime>(
    app: AppHandle<R>,
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

/// Deletes `paths`, records the batch and patches the tree. The two warnings of
/// [`BatchResult`] come back with the outcome: an `Err` here means the batch did not run.
///
/// Takes paths and a mode, never a finished preview: a preview that came over the wire is a
/// claim, and the guards run here on this side of it.
#[tauri::command]
pub async fn action_run(
    manager: State<'_, ScanManager>,
    sys: State<'_, RealSystem>,
    log: State<'_, ActionLog>,
    batches: State<'_, Arc<BatchLock>>,
    paths: Vec<String>,
    mode: Mode,
) -> Result<BatchResult, String> {
    let (manager, sys, log, batches) = (
        manager.inner().clone(),
        sys.inner().clone(),
        log.inner().clone(),
        Arc::clone(batches.inner()),
    );
    let paths = to_paths(paths);
    off_the_event_loop(move || actions::run_batch(&manager, &sys, &log, &batches, paths, mode))
        .await
}

/// What the app has deleted, for the Activity screen: the last `limit` entries of the
/// action log, newest first (default 100), and how many lines could not be read.
///
/// An `Err` means the log could not be read — the same meaning [`action_run`] gives it, one
/// stage on: the read did not happen. It is not the answer for a log with nothing in it,
/// which is an empty list and no error, and the screen must not draw the two the same way.
#[tauri::command]
pub fn activity_log(log: State<'_, ActionLog>, limit: Option<usize>) -> Result<LogTail, String> {
    actions::activity(log.inner(), limit)
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
    fn a_body_that_returns_comes_back_as_its_value() {
        let value = tauri::async_runtime::block_on(off_the_event_loop(|| 7)).unwrap();
        assert_eq!(value, 7);
    }

    /// A panic on the blocking thread reaches the UI as an error, which reads as "the batch
    /// did not run" — and that is a lie about any panic raised *after* the entries were
    /// deleted. It is why `run_batch` catches the splice itself: what happens here must stay
    /// the report of a batch that never got that far.
    #[test]
    fn a_panicking_body_comes_back_as_an_error() {
        let err = tauri::async_runtime::block_on(off_the_event_loop(|| {
            panic!("the arena rebuild ran past its ceiling")
        }))
        .unwrap_err();
        assert!(err.contains("did not finish"), "{err}");
    }

    #[test]
    fn default_root_is_the_home_folder() {
        let root = default_root().unwrap();
        assert_eq!(Some(PathBuf::from(&root)), paths::home_dir());
        assert!(root.starts_with('/'), "{root}");
    }

    /// The command over the [`ActionLog`] the app manages, where the meaning of `Err` is
    /// fixed for the screen that Task 11 and the Activity page are written against: a log
    /// nothing has written yet resolves as an empty list, and a log that cannot be read
    /// rejects. The same file, in both states, so nothing but the read can explain the
    /// difference.
    #[test]
    fn the_activity_command_reads_the_log_the_app_manages() {
        use storage_monitor_core::action::{EntryOutcome, EntryResult, Outcome};
        use storage_monitor_core::scan::NodeKind;
        use tauri::Manager;

        let app = tauri::test::mock_app();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actions.jsonl");
        app.manage(ActionLog::new(path.clone()));

        assert_eq!(
            activity_log(app.state(), None).expect("nothing has been deleted yet"),
            LogTail::default(),
            "a log that was never written is an empty list, not a failure"
        );

        let entry = |name: &str| EntryOutcome {
            path: PathBuf::from(name),
            kind: NodeKind::File,
            result: EntryResult::Removed { bytes: 1_024 },
        };
        ActionLog::new(path.clone())
            .append(&Outcome {
                entries: vec![entry("/h/older"), entry("/h/newer")],
                freed_bytes: 2_048,
                at: chrono::Utc::now(),
                mode: Mode::Trash,
            })
            .expect("the log is written");
        let tail = activity_log(app.state(), Some(1)).expect("the log reads");
        assert_eq!(
            tail.entries.len(),
            1,
            "the limit reaches the read: {tail:?}"
        );
        assert_eq!(tail.entries[0].path, "/h/newer", "newest first");

        // The same log, now impossible to read: a directory in its place.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let err = activity_log(app.state(), None)
            .expect_err("a read that failed must not reach the screen as an empty list");
        assert!(err.contains("action log"), "{err}");
    }
}
