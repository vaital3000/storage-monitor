//! Deletion batches: where the scan the window holds, the action engine and the record of
//! what was deleted meet.
//!
//! A batch is always re-planned here from the paths alone. Nothing the UI sends is trusted
//! beyond the paths and the mode — not a preview it was shown, and not the sizes in it.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, Utc};
use storage_monitor_core::action::{
    ActionLog, BlockReason, EntryOutcome, EntryResult, EntryStatus, Limits, Mode, Outcome, Plan,
    PlanEntry, Preview, PreviewEntry, execute, preview,
};
use storage_monitor_core::scan::NodeKind;
use storage_monitor_core::system::System;

use crate::scan_manager::{ScanManager, TreeState, panic_text};
use crate::views::BatchResult;

/// The queue of one: batches run one at a time in this process.
///
/// Tauri does not serialize invocations, and both action commands hand their body to
/// `spawn_blocking`, so two batches can be in flight at once. They would each resolve ids
/// against the same generation, and the second splice would then be dropped for a
/// generation the first one moved — the deletions land, the tree keeps a row for one of
/// them, and nothing but a scan repairs it. Queueing them instead costs the second batch
/// the wait and makes two batches over overlapping paths deterministic.
///
/// The dialog's busy state is the other half of this, and the cheaper half to lose: this
/// one holds wherever the invocation came from.
#[derive(Debug, Default)]
pub struct BatchLock(Mutex<()>);

impl BatchLock {
    /// Waits for the batch in front, if there is one. Poisoning is not a reason to refuse a
    /// deletion the user asked for: the guard protects an order, not data.
    fn enter(&self) -> MutexGuard<'_, ()> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a batch is running right now. Tests assert through it that the queue is held
    /// for the whole of [`run_batch`], which is the part no observer can see from outside.
    #[cfg(test)]
    fn is_held(&self) -> bool {
        self.0.try_lock().is_err()
    }
}

/// What a batch would do, with nothing touched: every path against the guards and the disk.
///
/// Safe to call as often as the dialog needs it. The verdict is a moment's truth and the
/// batch re-checks all of it, so a preview that has gone stale costs nothing.
pub fn preview_batch(
    manager: &ScanManager,
    sys: &dyn System,
    paths: &[PathBuf],
    mode: Mode,
) -> Preview {
    let plan = plan_for(manager, paths, mode);
    match limits_of(manager) {
        Some(limits) => preview(&plan, &limits, sys),
        None => refused_preview(&plan),
    }
}

/// Deletes what the guards allow, records the whole batch, patches the tree, and reports
/// which of those three the window has to tell the user about.
///
/// The order is the order of consequences. The log is written before the tree is patched: it
/// is the record of something that has already happened, while the splice is a rebuild of
/// the arena with a live assertion in it.
///
/// Neither of the last two failures fails the batch — the files are gone, and a batch that
/// came back as an error would say the opposite — but neither is swallowed either.
/// [`BatchResult::recorded`] and [`BatchResult::tree_stale`] carry them to the dialog,
/// because a bundled `.app` has no stderr and both leave something only the user can act on:
/// a deletion with no record of it, and an Explorer showing a row for something that is gone.
///
/// Every entry that was removed **or** failed is rescanned, not just the removed ones:
/// `remove` is not atomic, so a failure can leave most of a tree deleted, and the rescan is
/// what makes the Explorer agree with the disk again.
///
/// **The rule for whoever adds the next step here.** An `Err` from `action_run` means the
/// batch did not run, and the dialog is written against exactly that: every failure of a
/// batch that *did* run is a field of [`BatchResult`], never an error. So nothing added
/// after [`execute`] may panic out of this function — which is why the splice is caught
/// rather than trusted, and why the same has to be done for whatever comes next.
///
/// In a debug build that guarantee is already weaker than it reads, and deliberately: the
/// `debug_assert`s in [`touched`] and in `execute`'s own `freed_bytes` invariant fire after
/// the entries are gone, so a developer build can answer `Err` for a batch that deleted
/// everything it was asked to. That is what an assertion is for. It is not the contract the
/// UI is written against, and it is not a licence to add a fallible step here.
pub fn run_batch(
    manager: &ScanManager,
    sys: &dyn System,
    log: &ActionLog,
    batches: &BatchLock,
    paths: Vec<PathBuf>,
    mode: Mode,
) -> BatchResult {
    // Held for the whole batch, plan included: two batches that resolve ids against one
    // generation lose a patch whichever of them splices second.
    let _queued = batches.enter();
    let plan = plan_for(manager, &paths, mode);
    let outcome = match limits_of(manager) {
        Some(limits) => {
            let checked = preview(&plan, &limits, sys);
            execute(&checked, &limits, sys)
        }
        None => refused_outcome(&plan, sys.now()),
    };
    let recorded = match log.append(&outcome) {
        Ok(()) => true,
        Err(err) => {
            eprintln!("cannot record the batch in {}: {err}", log.path().display());
            false
        }
    };
    let tree_stale = stale_after(panic::catch_unwind(AssertUnwindSafe(|| {
        manager.patch_paths(&touched(&paths, &outcome))
    })));
    BatchResult {
        outcome,
        recorded,
        tree_stale,
    }
}

/// What the splice leaves behind, for the window to say.
///
/// The `Err` is a panic in the rebuild, whose ceiling is a live `assert!` even in a release
/// build. It fires after the files are gone and after the record is written, so letting it
/// out of [`run_batch`] would cost the command its result — and the dialog would say the
/// batch did not finish about a batch that deleted everything it was asked to. Caught, it
/// costs the tree its accuracy instead, which is what this reports and what a scan repairs.
fn stale_after(patched: Result<TreeState, Box<dyn Any + Send>>) -> bool {
    match patched {
        Ok(state) => state == TreeState::Stale,
        Err(payload) => {
            eprintln!("the tree patch panicked: {}", panic_text(&*payload));
            true
        }
    }
}

/// The batch as the engine takes it.
///
/// The kind and the size come from the tree — the size especially, because it is the number
/// the Explorer showed the user and the one the dialog is about; re-reading it would mean
/// walking the subtree of every selected directory. A path the tree does not know carries
/// neither: the guards still judge it, and `preview` reads its kind from the disk before
/// anything is touched.
fn plan_for(manager: &ScanManager, paths: &[PathBuf], mode: Mode) -> Plan {
    let known: Vec<Option<(NodeKind, u64)>> = manager
        .with_result(|result, _| {
            paths
                .iter()
                // The spelling the UI sent, which is the spelling the scan recorded. The
                // normalized form the guards produce is not a key of this tree.
                .map(|path| {
                    result
                        .tree
                        .find(path)
                        .and_then(|id| result.tree.get(id))
                        .map(|node| (node.kind, node.size))
                })
                .collect()
        })
        .unwrap_or_else(|| vec![None; paths.len()]);
    let entries = paths
        .iter()
        .zip(known)
        .map(|(path, known)| {
            let (kind, size) = known.unwrap_or((NodeKind::Other, 0));
            PlanEntry {
                path: path.clone(),
                kind,
                size,
            }
        })
        .collect();
    Plan { entries, mode }
}

/// The rules for the scan the window holds, or `None` when nothing has been scanned.
///
/// The root, deliberately, and not the tree, because the two part company in two states.
/// A **failed** scan leaves a root and no result. So does a scan that is **running**:
/// `ScanManager::start` clears the result and keeps the root, so a batch can be deleting
/// paths the walker is enumerating this second — which is safe, and is the state that made
/// this a root question rather than a tree question. In both, the guards are exactly the
/// rules the user's choice of root implies, and only the sizes are missing; [`plan_for`]
/// says what that costs.
///
/// A **cancelled** scan is not one of them: `Inner::complete` installs its partial tree like
/// any other, so it has a root and a result, and a batch patches it like any other.
///
/// Not limits over an empty root: every path starts with the empty path, so rules 4 and 5
/// would pass for everything on the machine, and the `debug_assert` that says so is gone in
/// a release build.
fn limits_of(manager: &ScanManager) -> Option<Limits> {
    manager.root().map(Limits::for_scan_root)
}

/// The paths of the batch whose branch on disk may have changed, in the spelling the caller
/// handed in — never the normalized one the outcome carries, which [`crate::scan_manager::ScanManager::patch_paths`]
/// explains.
fn touched(paths: &[PathBuf], outcome: &Outcome) -> Vec<PathBuf> {
    // `preview` and `execute` both keep every entry in its place, which is what lets the
    // outcome be read next to the paths it came from. Both halves are checked, because the
    // one that matters is the pairing and `zip` answers a broken one by silently
    // truncating: in a release build a batch would then patch the wrong paths, or none.
    debug_assert_eq!(
        paths.len(),
        outcome.entries.len(),
        "every entry of the plan comes back"
    );
    debug_assert!(
        paths
            .iter()
            .zip(&outcome.entries)
            .all(|(path, entry)| path.file_name() == entry.path.file_name()),
        "every entry of the plan comes back in its place: the guards normalize a path but \
         keep the caller's own last component, so the pairs have to agree about it"
    );
    paths
        .iter()
        .zip(&outcome.entries)
        .filter(|(_, entry)| {
            matches!(
                entry.result,
                EntryResult::Removed { .. } | EntryResult::Failed { .. }
            )
        })
        .map(|(path, _)| path.clone())
        .collect()
}

/// A batch with no scan root behind it: nothing is inside a root that does not exist, and
/// the honest verdict for every entry is the one the guards would give.
fn refused_preview(plan: &Plan) -> Preview {
    Preview {
        entries: plan
            .entries
            .iter()
            .map(|entry| PreviewEntry {
                path: entry.path.clone(),
                kind: entry.kind,
                size: entry.size,
                status: EntryStatus::Blocked(BlockReason::OutsideRoots),
            })
            .collect(),
        total_bytes: 0,
        mode: plan.mode,
    }
}

/// The same refusal, one stage on. It reaches the log like any other batch: the record is
/// of what the app was asked to delete, and a row the user asked for and did not get is
/// exactly what the Activity screen is for.
fn refused_outcome(plan: &Plan, at: DateTime<Utc>) -> Outcome {
    Outcome {
        entries: plan
            .entries
            .iter()
            .map(|entry| EntryOutcome {
                path: entry.path.clone(),
                kind: entry.kind,
                result: EntryResult::Skipped {
                    reason: BlockReason::OutsideRoots,
                },
            })
            .collect(),
        freed_bytes: 0,
        at,
        mode: plan.mode,
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, Metadata};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    use chrono::DateTime;
    use storage_monitor_core::system::SystemError;

    use storage_monitor_core::action::{
        ActionLog, BlockReason, EntryResult, Limits, LogResult, Mode,
    };
    use storage_monitor_core::scan::{NodeKind, Tree};
    use storage_monitor_core::system::{System, TestSystem};

    use super::*;
    use crate::running_as_root;
    use crate::scan_manager::{ScanManager, StatusEmitter};
    use crate::views::{NodeView, ScanState, ScanStatus};

    /// The manager insists on an emitter; these tests read the state instead of the events.
    struct Silent;

    impl StatusEmitter for Silent {
        fn emit(&self, _event: &str, _status: &ScanStatus) {}
    }

    /// A manager holding a finished scan of `dir`, with its snapshots in a temp directory.
    /// The directory is returned because it has to outlive the manager, and because the
    /// action log of a test goes in it — outside the scanned root, where a batch cannot
    /// delete it and a rescan cannot see it.
    fn scanned(dir: &Path) -> (ScanManager, tempfile::TempDir) {
        let data = tempfile::tempdir().unwrap();
        let manager = ScanManager::with_snapshots_dir(data.path().join("snapshots"));
        manager
            .start(Arc::new(Silent), dir.to_path_buf())
            .expect("the scan starts");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = manager.status();
            if status.state != ScanState::Running {
                assert_eq!(status.state, ScanState::Done, "{status:?}");
                return (manager, data);
            }
            assert!(
                Instant::now() < deadline,
                "the scan did not finish: {status:?}"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn log_in(data: &tempfile::TempDir) -> ActionLog {
        ActionLog::new(data.path().join("actions.jsonl"))
    }

    /// A batch with a queue of its own: only the two tests about the queue have a second
    /// batch for it to order against.
    fn run_one(
        manager: &ScanManager,
        sys: &dyn System,
        log: &ActionLog,
        paths: Vec<PathBuf>,
        mode: Mode,
    ) -> BatchResult {
        run_batch(manager, sys, log, &BatchLock::default(), paths, mode)
    }

    /// The root of the tree the manager holds, as the Explorer would ask for it.
    fn root_view(manager: &ScanManager) -> NodeView {
        manager
            .with_result(|result, previous| {
                NodeView::build(&result.tree, Tree::ROOT, 500, previous)
            })
            .expect("a scan result")
            .expect("the root node")
    }

    fn child_names(view: &NodeView) -> Vec<String> {
        view.children.iter().map(|c| c.name.clone()).collect()
    }

    /// The rows the Explorer would draw for a directory below the root.
    fn children_of(manager: &ScanManager, path: &Path) -> Vec<String> {
        let view = manager
            .with_result(|result, previous| {
                let id = result
                    .tree
                    .find(path)
                    .unwrap_or_else(|| panic!("{} is not in the tree", path.display()));
                NodeView::build(&result.tree, id, 500, previous)
            })
            .expect("a scan result")
            .expect("the node");
        child_names(&view)
    }

    fn child_size(view: &NodeView, name: &str) -> u64 {
        view.children
            .iter()
            .find(|child| child.name == name)
            .unwrap_or_else(|| panic!("no child {name} in {:?}", child_names(view)))
            .size
    }

    fn write_file(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    #[test]
    fn a_trashed_directory_leaves_the_tree_and_shrinks_its_parent() {
        let sys = TestSystem::new();
        let root = sys.root().to_path_buf();
        write_file(&root.join("cache/inner/blob.bin"), 8_192);
        write_file(&root.join("keep.bin"), 4_096);
        let (manager, data) = scanned(&root);
        let before = root_view(&manager);
        let cache_size = child_size(&before, "cache");
        let generation = manager.generation();

        let batch = run_one(
            &manager,
            &sys,
            &log_in(&data),
            vec![root.join("cache")],
            Mode::Trash,
        );

        assert_eq!(batch.outcome.entries.len(), 1);
        assert_eq!(batch.outcome.entries[0].path, root.join("cache"));
        assert_eq!(batch.outcome.entries[0].kind, NodeKind::Dir);
        assert_eq!(
            batch.outcome.entries[0].result,
            EntryResult::Removed { bytes: cache_size }
        );
        assert_eq!(batch.outcome.freed_bytes, cache_size);
        assert_eq!(batch.outcome.mode, Mode::Trash);
        assert_eq!(batch.outcome.at, sys.now());
        assert!(batch.recorded, "the record was written");
        assert!(!batch.tree_stale, "and the Explorer agrees with the disk");

        assert!(
            fs::symlink_metadata(root.join("cache")).is_err(),
            "the directory left the disk"
        );
        assert_eq!(
            fs::read(sys.trash_dir().join("cache/inner/blob.bin")).unwrap(),
            vec![b'x'; 8_192],
            "and is in the Trash with its contents"
        );

        let view = root_view(&manager);
        assert_eq!(child_names(&view), vec!["keep.bin"], "the row is gone");
        assert_eq!(view.size, before.size - cache_size, "the parent shrank");
        assert_eq!(
            manager.generation(),
            generation + 1,
            "the ids changed, so the generation had to"
        );
    }

    #[test]
    fn a_partially_deleted_directory_keeps_what_is_left() {
        if running_as_root() {
            return;
        }
        let sys = TestSystem::new();
        let root = sys.root().to_path_buf();
        let cache = root.join("cache");
        write_file(&cache.join("inner/blob.bin"), 8_192);
        write_file(&root.join("keep.bin"), 4_096);
        let (manager, data) = scanned(&root);
        let before = root_view(&manager);
        let cache_before = child_size(&before, "cache");

        // `cache` stays readable but nothing can be unlinked from it: `remove_dir_all`
        // empties `inner` and then fails to remove `inner` itself. A real partial failure,
        // which is what the rescan after a batch exists for.
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o555)).unwrap();
        let batch = run_one(
            &manager,
            &sys,
            &log_in(&data),
            vec![cache.clone()],
            Mode::Permanent,
        );
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o755)).unwrap();

        match &batch.outcome.entries[0].result {
            EntryResult::Failed { message } => assert!(
                message.contains("cannot delete"),
                "the port's own message: {message}"
            ),
            other => panic!("expected a failure, got {other:?}"),
        }
        assert_eq!(batch.outcome.freed_bytes, 0, "a failed entry frees nothing");
        assert!(batch.recorded);
        assert!(
            !batch.tree_stale,
            "the remainder was spliced in, so the tree is right about it"
        );
        assert!(
            fs::symlink_metadata(cache.join("inner")).is_ok(),
            "the remainder is still on disk"
        );
        assert!(
            fs::symlink_metadata(cache.join("inner/blob.bin")).is_err(),
            "what was deleted before the failure is gone"
        );

        let view = root_view(&manager);
        assert!(
            child_names(&view).contains(&"cache".to_owned()),
            "the row stays: {:?}",
            child_names(&view)
        );
        let cache_after = child_size(&view, "cache");
        assert!(
            cache_after < cache_before,
            "the remainder is smaller: {cache_after} vs {cache_before}"
        );
        assert_eq!(
            view.size,
            before.size - (cache_before - cache_after),
            "the parent lost exactly what the branch lost"
        );
        assert_eq!(
            child_size(&view, "keep.bin"),
            child_size(&before, "keep.bin"),
            "the row nobody selected did not move"
        );
    }

    #[test]
    fn a_blocked_entry_changes_nothing() {
        let sys = TestSystem::new();
        let root = sys.root().to_path_buf();
        write_file(&root.join("cache/blob.bin"), 8_192);
        let outside = tempfile::tempdir().unwrap();
        let precious = outside.path().join("precious.bin");
        fs::write(&precious, b"keep").unwrap();
        let (manager, data) = scanned(&root);
        let before = root_view(&manager);
        let generation = manager.generation();

        let batch = run_one(
            &manager,
            &sys,
            &log_in(&data),
            vec![precious.clone()],
            Mode::Permanent,
        );

        assert_eq!(
            batch.outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::OutsideRoots
            }
        );
        assert_eq!(batch.outcome.freed_bytes, 0);
        assert!(batch.recorded);
        assert!(!batch.tree_stale, "nothing was deleted to be stale about");
        assert_eq!(fs::read(&precious).unwrap(), b"keep", "nothing was touched");
        let view = root_view(&manager);
        assert_eq!(child_names(&view), child_names(&before));
        assert_eq!(view.size, before.size);
        assert_eq!(
            manager.generation(),
            generation,
            "no patch, so the ids did not change"
        );
    }

    #[test]
    fn every_entry_of_the_batch_reaches_the_log() {
        let sys = TestSystem::new();
        let root = sys.root().to_path_buf();
        write_file(&root.join("cache/blob.bin"), 8_192);
        write_file(&root.join("keep.bin"), 4_096);
        let outside = tempfile::tempdir().unwrap();
        let precious = outside.path().join("precious.bin");
        fs::write(&precious, b"keep").unwrap();
        let (manager, data) = scanned(&root);
        let cache_size = child_size(&root_view(&manager), "cache");
        let log = log_in(&data);

        let batch = run_one(
            &manager,
            &sys,
            &log,
            vec![
                root.join("cache"),
                precious.clone(),
                root.join("never-existed.bin"),
            ],
            Mode::Trash,
        );

        assert_eq!(batch.outcome.entries.len(), 3);
        let tail = log.tail(10).unwrap();
        assert_eq!(tail.damaged, 0);
        assert_eq!(tail.entries.len(), 3, "one line per entry: {tail:?}");
        assert!(
            tail.entries
                .iter()
                .all(|entry| entry.at == batch.outcome.at),
            "one timestamp for the batch: {tail:?}"
        );
        assert!(
            tail.entries.iter().all(|entry| entry.mode == Mode::Trash),
            "{tail:?}"
        );
        let line = |path: &Path| {
            tail.entries
                .iter()
                .find(|entry| entry.path == path.to_string_lossy())
                .unwrap_or_else(|| panic!("no line for {}: {tail:?}", path.display()))
                .clone()
        };
        let removed = line(&root.join("cache"));
        assert_eq!(removed.result, LogResult::Removed);
        assert_eq!(removed.bytes, cache_size);
        assert_eq!(removed.kind, NodeKind::Dir);
        assert_eq!(removed.detail, None);
        let blocked = line(&precious);
        assert_eq!(blocked.result, LogResult::Skipped);
        assert_eq!(blocked.detail.as_deref(), Some("outsideRoots"));
        assert_eq!(blocked.bytes, 0);
        let missing = line(&root.join("never-existed.bin"));
        assert_eq!(missing.result, LogResult::Skipped);
        assert_eq!(missing.detail.as_deref(), Some("missing"));
        assert_eq!(
            child_names(&root_view(&manager)),
            vec!["keep.bin"],
            "and the row that left the tree is the one that was removed"
        );
    }

    /// The rescan after a batch reports what this app did, not what the disk looks like. An
    /// entry that was already gone was never touched, so nothing is spliced for it: a splice
    /// costs a rebuild of the whole arena, and a rescan takes hard-linked bytes back from a
    /// twin outside the branch (design, section 6). The row stays until the next scan, as
    /// every other row does that the disk changed behind the app's back.
    #[test]
    fn an_entry_that_was_already_gone_leaves_the_tree_alone() {
        let sys = TestSystem::new();
        let root = sys.root().to_path_buf();
        write_file(&root.join("cache/blob.bin"), 8_192);
        write_file(&root.join("keep.bin"), 4_096);
        let (manager, data) = scanned(&root);
        let before = root_view(&manager);
        let generation = manager.generation();
        fs::remove_dir_all(root.join("cache")).unwrap();

        let batch = run_one(
            &manager,
            &sys,
            &log_in(&data),
            vec![root.join("cache")],
            Mode::Trash,
        );

        assert_eq!(
            batch.outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::Missing
            }
        );
        assert_eq!(
            child_names(&root_view(&manager)),
            child_names(&before),
            "the stale row stays"
        );
        assert!(
            !batch.tree_stale,
            "and no warning goes with it: the tree is wrong about a row this batch did not \
             delete, which is true of every row the disk changed behind the app's back. The \
             flag is about what the batch itself left behind, or it means nothing."
        );
        assert_eq!(root_view(&manager).size, before.size);
        assert_eq!(
            manager.generation(),
            generation,
            "nothing was spliced, so no id changed"
        );
    }

    /// The batch splices once, so both ids have to survive the rebuild the other one causes.
    /// Patching one path at a time with ids resolved before the first splice passes the
    /// single-entry tests above and destroys the tree here.
    #[test]
    fn two_directories_of_one_batch_both_leave_the_tree() {
        let sys = TestSystem::new();
        let root = sys.root().to_path_buf();
        write_file(&root.join("cache/blob.bin"), 8_192);
        write_file(&root.join("logs/log.txt"), 12_288);
        write_file(&root.join("keep.bin"), 4_096);
        let (manager, data) = scanned(&root);
        let before = root_view(&manager);
        let (cache_size, logs_size) = (child_size(&before, "cache"), child_size(&before, "logs"));

        let batch = run_one(
            &manager,
            &sys,
            &log_in(&data),
            vec![root.join("cache"), root.join("logs")],
            Mode::Trash,
        );

        assert_eq!(batch.outcome.freed_bytes, cache_size + logs_size);
        let view = root_view(&manager);
        assert_eq!(child_names(&view), vec!["keep.bin"], "both rows are gone");
        assert_eq!(
            child_size(&view, "keep.bin"),
            child_size(&before, "keep.bin")
        );
        assert_eq!(view.size, before.size - cache_size - logs_size);
    }

    #[test]
    fn a_log_that_cannot_be_written_does_not_stop_the_batch() {
        let sys = TestSystem::new();
        let root = sys.root().to_path_buf();
        write_file(&root.join("cache/blob.bin"), 8_192);
        let (manager, data) = scanned(&root);
        // A file where the log wants a directory: `create_dir_all` fails, and with it the
        // append. The batch has already deleted something and must say so anyway.
        let wall = data.path().join("not-a-directory");
        fs::write(&wall, b"x").unwrap();
        let log = ActionLog::new(wall.join("actions.jsonl"));

        let batch = run_one(&manager, &sys, &log, vec![root.join("cache")], Mode::Trash);

        assert!(
            fs::symlink_metadata(log.path()).is_err(),
            "the batch was not recorded"
        );
        assert!(
            !batch.recorded,
            "and the dialog is told: deleted, with no record of it"
        );
        assert!(!batch.tree_stale, "the tree was patched all the same");
        assert!(matches!(
            batch.outcome.entries[0].result,
            EntryResult::Removed { .. }
        ));
        assert!(fs::symlink_metadata(root.join("cache")).is_err());
        assert!(
            child_names(&root_view(&manager)).is_empty(),
            "and the tree was still patched"
        );
    }

    #[test]
    fn a_batch_without_a_scan_deletes_nothing() {
        let sys = TestSystem::new();
        let root = sys.root().to_path_buf();
        write_file(&root.join("cache/blob.bin"), 8_192);
        let data = tempfile::tempdir().unwrap();
        let manager = ScanManager::with_snapshots_dir(data.path().join("snapshots"));

        let batch = run_one(
            &manager,
            &sys,
            &log_in(&data),
            vec![root.join("cache")],
            Mode::Permanent,
        );

        assert_eq!(
            batch.outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::OutsideRoots
            },
            "with no scan root, nothing is inside it"
        );
        assert_eq!(batch.outcome.freed_bytes, 0);
        assert!(batch.recorded && !batch.tree_stale);
        assert!(
            fs::symlink_metadata(root.join("cache/blob.bin")).is_ok(),
            "nothing was deleted"
        );
    }

    /// A selection can be empty — the confirmation dialog is not the only way in, and a
    /// batch of nothing must cost nothing: no line in the record of deletions, and no
    /// rebuild of the arena.
    #[test]
    fn an_empty_batch_does_nothing_at_all() {
        let sys = TestSystem::new();
        let root = sys.root().to_path_buf();
        write_file(&root.join("cache/blob.bin"), 8_192);
        let (manager, data) = scanned(&root);
        let before = root_view(&manager);
        let generation = manager.generation();
        let log = log_in(&data);

        let batch = run_one(&manager, &sys, &log, Vec::new(), Mode::Permanent);

        assert!(batch.outcome.entries.is_empty());
        assert_eq!(batch.outcome.freed_bytes, 0);
        assert!(
            batch.recorded && !batch.tree_stale,
            "nothing happened, so there is nothing to warn about"
        );
        assert!(
            fs::symlink_metadata(log.path()).is_err(),
            "nothing happened, so nothing is recorded"
        );
        assert_eq!(child_names(&root_view(&manager)), child_names(&before));
        assert_eq!(manager.generation(), generation);
    }

    #[test]
    fn a_preview_carries_what_the_tree_knows_and_touches_nothing() {
        let sys = TestSystem::new();
        let root = sys.root().to_path_buf();
        write_file(&root.join("cache/blob.bin"), 8_192);
        let (manager, _data) = scanned(&root);
        let cache_size = child_size(&root_view(&manager), "cache");

        let preview = preview_batch(
            &manager,
            &sys,
            &[root.join("cache"), root.join("never-existed.bin")],
            Mode::Trash,
        );

        assert_eq!(preview.mode, Mode::Trash);
        assert_eq!(preview.entries[0].path, root.join("cache"));
        assert_eq!(preview.entries[0].kind, NodeKind::Dir);
        assert_eq!(
            preview.entries[0].size, cache_size,
            "the size the Explorer showed"
        );
        assert_eq!(preview.entries[1].size, 0, "a path the tree never saw");
        assert_eq!(
            preview.total_bytes, cache_size,
            "blocked entries promise nothing"
        );
        assert!(fs::symlink_metadata(root.join("cache/blob.bin")).is_ok());
    }

    /// A port that does the real thing and then lets a test change the world, in the one
    /// window nothing else can reach: between the deletion and the rescan that follows it.
    /// Both of the batch's own failure reports need that window — a rescan that cannot look,
    /// and a second batch arriving mid-flight.
    struct Meddling<'a> {
        inner: &'a TestSystem,
        /// Runs after the port was *asked* to delete, whether or not it managed to: a hook
        /// that ran only after a real removal would quietly not run for an entry a test
        /// failed on purpose, which is the other half of what this seam is for.
        after_attempt: Box<dyn Fn() + Send + Sync + 'a>,
    }

    impl System for Meddling<'_> {
        fn symlink_metadata(&self, path: &Path) -> Result<Metadata, SystemError> {
            self.inner.symlink_metadata(path)
        }

        fn move_to_trash(&self, path: &Path) -> Result<(), SystemError> {
            let done = self.inner.move_to_trash(path);
            (self.after_attempt)();
            done
        }

        fn remove(&self, path: &Path) -> Result<(), SystemError> {
            let done = self.inner.remove(path);
            (self.after_attempt)();
            done
        }

        fn now(&self) -> DateTime<Utc> {
            self.inner.now()
        }
    }

    /// The deletion happened and the rescan cannot look: the row for a directory that is
    /// gone stays in the tree, and the only honest thing left to do is say so. Without the
    /// warning this is the one failure the user cannot even see — the batch reports success
    /// and the Explorer quietly disagrees with the disk until the next scan.
    ///
    /// Two paths, and only one of them locked, because one path cannot state the rule: when
    /// every rescan of a batch fails there is nothing left to splice either, so the patch is
    /// dropped and the tree would be called stale for the other reason. It takes a batch
    /// that splices *and* loses a path to tell the two apart.
    #[test]
    fn a_batch_whose_rescan_cannot_look_reports_a_stale_tree() {
        if running_as_root() {
            return;
        }
        let sys = TestSystem::new();
        let root = sys.root().to_path_buf();
        let (locked, open) = (root.join("locked"), root.join("open"));
        write_file(&locked.join("cache/blob.bin"), 8_192);
        write_file(&open.join("cache/blob.bin"), 4_096);
        let (manager, data) = scanned(&root);
        // Locked only once the deletions are done: the guards refuse an entry whose parent
        // cannot be resolved, so locking it earlier would mean nothing was deleted at all.
        let meddling = Meddling {
            inner: &sys,
            after_attempt: Box::new(|| {
                fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
            }),
        };

        let batch = run_one(
            &manager,
            &meddling,
            &log_in(&data),
            vec![locked.join("cache"), open.join("cache")],
            Mode::Trash,
        );

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            batch
                .outcome
                .entries
                .iter()
                .all(|entry| matches!(entry.result, EntryResult::Removed { .. })),
            "both deletions went through: {:?}",
            batch.outcome.entries
        );
        assert!(batch.recorded);
        assert!(
            batch.tree_stale,
            "one rescan could not look, so the tree still names what was deleted"
        );
        assert!(
            fs::symlink_metadata(locked.join("cache")).is_err(),
            "and that directory really is gone"
        );
        assert_eq!(
            children_of(&manager, &locked),
            vec!["cache"],
            "the stale row is exactly what the warning is about"
        );
        assert!(
            children_of(&manager, &open).is_empty(),
            "while the path that could be rescanned was spliced in the same call"
        );
    }

    /// The one branch of a batch no fixture can reach: the rebuild's assertion firing. What
    /// it must not do is turn a deletion that happened into a command that says it did not.
    #[test]
    fn a_splice_that_panicked_leaves_the_tree_stale() {
        assert!(!stale_after(Ok(TreeState::Current)));
        assert!(stale_after(Ok(TreeState::Stale)));
        assert!(
            stale_after(Err(Box::new("the rebuild ran past its ceiling"))),
            "a panic costs the tree its accuracy, and the batch keeps its report"
        );
    }

    /// The queue is held for the whole batch, not just around the splice. Nothing outside
    /// the process can observe that, so the observation is made from inside it, at the one
    /// moment that matters: files are already leaving the disk.
    #[test]
    fn a_batch_holds_the_queue_from_the_plan_to_the_patch() {
        let sys = TestSystem::new();
        let root = sys.root().to_path_buf();
        write_file(&root.join("cache/blob.bin"), 8_192);
        let (manager, data) = scanned(&root);
        let batches = BatchLock::default();
        assert!(!batches.is_held(), "and it is free when nothing runs");
        let held = Mutex::new(Vec::new());
        let meddling = Meddling {
            inner: &sys,
            after_attempt: Box::new(|| held.lock().unwrap().push(batches.is_held())),
        };

        let batch = run_batch(
            &manager,
            &meddling,
            &log_in(&data),
            &batches,
            vec![root.join("cache")],
            Mode::Trash,
        );

        assert!(batch.recorded && !batch.tree_stale);
        assert_eq!(
            *held.lock().unwrap(),
            vec![true],
            "a second batch would have had to wait"
        );
        assert!(!batches.is_held(), "and the queue is free again afterwards");
    }

    /// Two batches at once are what the queue is for: without it they resolve ids against
    /// one generation and whichever splices second is dropped, leaving a row for a
    /// directory that is gone.
    #[test]
    fn two_batches_at_once_both_reach_the_tree() {
        let sys = TestSystem::new();
        let root = sys.root().to_path_buf();
        write_file(&root.join("cache/blob.bin"), 8_192);
        write_file(&root.join("logs/log.txt"), 12_288);
        write_file(&root.join("keep.bin"), 4_096);
        let (manager, data) = scanned(&root);
        let batches = BatchLock::default();
        let log = log_in(&data);

        let (manager, sys, log, batches, root) = (&manager, &sys, &log, &batches, &root);
        thread::scope(|scope| {
            for name in ["cache", "logs"] {
                scope.spawn(move || {
                    let batch = run_batch(
                        manager,
                        sys,
                        log,
                        batches,
                        vec![root.join(name)],
                        Mode::Trash,
                    );
                    assert!(batch.recorded, "{name}");
                    assert!(!batch.tree_stale, "{name}: the patch was not dropped");
                });
            }
        });

        assert_eq!(
            child_names(&root_view(manager)),
            vec!["keep.bin"],
            "both rows left the tree"
        );
        assert_eq!(log.tail(10).unwrap().entries.len(), 2);
    }

    /// The tree is addressed by the spelling the scan recorded, never by the normalized
    /// path the guards produce. A scan root reached through a symlink is where the two
    /// differ: the guards resolve it and `Tree::find` answers only to the other one, so a
    /// patch keyed on the guards' path silently misses and the Explorer goes on showing a
    /// directory that is gone.
    #[test]
    fn a_batch_under_a_symlinked_root_patches_the_tree_anyway() {
        let sys = TestSystem::new();
        write_file(&sys.root().join("fixture/cache/blob.bin"), 8_192);
        write_file(&sys.root().join("fixture/keep.bin"), 4_096);
        let root = sys.root().join("link");
        std::os::unix::fs::symlink(sys.root().join("fixture"), &root).unwrap();
        let (manager, data) = scanned(&root);

        let asked = root.join("cache");
        let checked = Limits::for_scan_root(root.clone()).check(&asked).unwrap();
        assert_ne!(
            checked.path, asked,
            "the guards resolve the link, or this test proves nothing"
        );
        assert!(
            manager
                .with_result(|result, _| result.tree.find(&checked.path).is_none()
                    && result.tree.find(&asked).is_some())
                .unwrap(),
            "only the spelling the scan recorded is a key of the tree"
        );

        let batch = run_one(&manager, &sys, &log_in(&data), vec![asked], Mode::Trash);

        assert!(matches!(
            batch.outcome.entries[0].result,
            EntryResult::Removed { .. }
        ));
        assert!(fs::symlink_metadata(sys.root().join("fixture/cache")).is_err());
        assert_eq!(
            child_names(&root_view(&manager)),
            vec!["keep.bin"],
            "the row is gone"
        );
    }
}
