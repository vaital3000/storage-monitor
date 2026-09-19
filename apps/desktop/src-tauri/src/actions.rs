//! Deletion batches: where the scan the window holds, the action engine and the record of
//! what was deleted meet.
//!
//! A batch is always re-planned here from the paths alone. Nothing the UI sends is trusted
//! beyond the paths and the mode — not a preview it was shown, and not the sizes in it.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use storage_monitor_core::action::{
    ActionLog, BlockReason, EntryOutcome, EntryResult, EntryStatus, Limits, Mode, Outcome, Plan,
    PlanEntry, Preview, PreviewEntry, execute, preview,
};
use storage_monitor_core::scan::NodeKind;
use storage_monitor_core::system::System;

use crate::scan_manager::ScanManager;

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

/// Deletes what the guards allow, records the whole batch and patches the tree.
///
/// The order is the order of consequences. The log is written before the tree is patched: it
/// is the record of something that has already happened, while the splice is a rebuild of
/// the arena with a live assertion in it. A log that cannot be written is printed to stderr
/// and nothing more — the files are gone either way, and failing the batch afterwards would
/// tell the user nothing they can act on.
///
/// Every entry that was removed **or** failed is rescanned, not just the removed ones:
/// `remove` is not atomic, so a failure can leave most of a tree deleted, and the rescan is
/// what makes the Explorer agree with the disk again.
pub fn run_batch(
    manager: &ScanManager,
    sys: &dyn System,
    log: &ActionLog,
    paths: Vec<PathBuf>,
    mode: Mode,
) -> Outcome {
    let plan = plan_for(manager, &paths, mode);
    let outcome = match limits_of(manager) {
        Some(limits) => {
            let checked = preview(&plan, &limits, sys);
            execute(&checked, &limits, sys)
        }
        None => refused_outcome(&plan, sys.now()),
    };
    if let Err(err) = log.append(&outcome) {
        eprintln!("cannot record the batch in {}: {err}", log.path().display());
    }
    manager.patch_paths(&touched(&paths, &outcome));
    outcome
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
    // outcome be read next to the paths it came from.
    debug_assert_eq!(
        paths.len(),
        outcome.entries.len(),
        "every entry of the plan comes back, in its place"
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
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

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

        let outcome = run_batch(
            &manager,
            &sys,
            &log_in(&data),
            vec![root.join("cache")],
            Mode::Trash,
        );

        assert_eq!(outcome.entries.len(), 1);
        assert_eq!(outcome.entries[0].path, root.join("cache"));
        assert_eq!(outcome.entries[0].kind, NodeKind::Dir);
        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Removed { bytes: cache_size }
        );
        assert_eq!(outcome.freed_bytes, cache_size);
        assert_eq!(outcome.mode, Mode::Trash);
        assert_eq!(outcome.at, sys.now());

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
        let outcome = run_batch(
            &manager,
            &sys,
            &log_in(&data),
            vec![cache.clone()],
            Mode::Permanent,
        );
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o755)).unwrap();

        match &outcome.entries[0].result {
            EntryResult::Failed { message } => assert!(
                message.contains("cannot delete"),
                "the port's own message: {message}"
            ),
            other => panic!("expected a failure, got {other:?}"),
        }
        assert_eq!(outcome.freed_bytes, 0, "a failed entry frees nothing");
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

        let outcome = run_batch(
            &manager,
            &sys,
            &log_in(&data),
            vec![precious.clone()],
            Mode::Permanent,
        );

        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::OutsideRoots
            }
        );
        assert_eq!(outcome.freed_bytes, 0);
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

        let outcome = run_batch(
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

        assert_eq!(outcome.entries.len(), 3);
        let tail = log.tail(10).unwrap();
        assert_eq!(tail.damaged, 0);
        assert_eq!(tail.entries.len(), 3, "one line per entry: {tail:?}");
        assert!(
            tail.entries.iter().all(|entry| entry.at == outcome.at),
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

        let outcome = run_batch(
            &manager,
            &sys,
            &log_in(&data),
            vec![root.join("cache")],
            Mode::Trash,
        );

        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::Missing
            }
        );
        assert_eq!(
            child_names(&root_view(&manager)),
            child_names(&before),
            "the stale row stays"
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

        let outcome = run_batch(
            &manager,
            &sys,
            &log_in(&data),
            vec![root.join("cache"), root.join("logs")],
            Mode::Trash,
        );

        assert_eq!(outcome.freed_bytes, cache_size + logs_size);
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

        let outcome = run_batch(&manager, &sys, &log, vec![root.join("cache")], Mode::Trash);

        assert!(
            fs::symlink_metadata(log.path()).is_err(),
            "the batch was not recorded"
        );
        assert!(matches!(
            outcome.entries[0].result,
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

        let outcome = run_batch(
            &manager,
            &sys,
            &log_in(&data),
            vec![root.join("cache")],
            Mode::Permanent,
        );

        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::OutsideRoots
            },
            "with no scan root, nothing is inside it"
        );
        assert_eq!(outcome.freed_bytes, 0);
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

        let outcome = run_batch(&manager, &sys, &log, Vec::new(), Mode::Permanent);

        assert!(outcome.entries.is_empty());
        assert_eq!(outcome.freed_bytes, 0);
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

        let outcome = run_batch(&manager, &sys, &log_in(&data), vec![asked], Mode::Trash);

        assert!(matches!(
            outcome.entries[0].result,
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
