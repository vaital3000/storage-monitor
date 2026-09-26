//! Cleanup batches: where the modules the window holds, the cleanup engine, the record and
//! the Explorer's tree meet — `actions.rs`'s twin for items instead of paths.
//!
//! A batch is always planned again here from the requests. Nothing the UI sends is trusted
//! beyond the item ids, the actions, the options and the mode.

use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use storage_monitor_core::action::{ActionLog, EntryResult, Limits, Mode};
use storage_monitor_core::cleanup::{self, CleanupOutcome, Progress, Request};

use crate::actions::{BatchLock, stale_after};
use crate::module_manager::{ModuleEmitter, ModuleManager};
use crate::scan_manager::ScanManager;
use crate::views::{CleanupPreviews, CleanupResult};

pub const CLEANUP_PROGRESS_EVENT: &str = "cleanup:progress";

/// How many items `cleanup_items` hands the UI when it asks for no number of its own.
pub(crate) const DEFAULT_ITEMS_LIMIT: usize = 2000;

/// Where a batch's progress goes: the Tauri app in production, a recorder in tests.
pub trait ProgressEmitter: Send + Sync {
    fn emit(&self, progress: &Progress);
}

impl<R: tauri::Runtime> ProgressEmitter for tauri::AppHandle<R> {
    fn emit(&self, progress: &Progress) {
        if let Err(err) = tauri::Emitter::emit(self, CLEANUP_PROGRESS_EVENT, progress) {
            eprintln!("cannot emit {CLEANUP_PROGRESS_EVENT}: {err}");
        }
    }
}

/// The guards a module's deletions answer to: the home folder as the only root (phase 2b
/// design, section 8). Without a home folder there is no honest root to build them from,
/// and no batch runs.
fn limits(modules: &ModuleManager) -> Result<Limits, String> {
    modules
        .home()
        .map(|home| Limits::for_home(home.to_path_buf()))
        .ok_or_else(|| "cannot determine the home folder".to_owned())
}

/// What a batch would do in each mode, with nothing touched.
pub fn preview_cleanup(
    modules: &ModuleManager,
    requests: &[Request],
) -> Result<CleanupPreviews, String> {
    let limits = limits(modules)?;
    let sys = modules.system();
    Ok(modules.with_held(|held| CleanupPreviews {
        trash: cleanup::preview(requests, held, &limits, sys, Mode::Trash),
        permanent: cleanup::preview(requests, held, &limits, sys, Mode::Permanent),
    }))
}

/// Runs a batch, records it, forgets what it removed, patches the Explorer's tree and starts
/// a rediscovery of the modules involved — and reports which of those the window has to tell
/// the user about.
///
/// The order is `run_batch`'s, for its reasons: the record before the splice, and nothing
/// after the engine that may panic out of here, because `Err` means the batch did not run.
#[allow(clippy::too_many_arguments)]
pub fn run_cleanup(
    modules: &ModuleManager,
    scans: &ScanManager,
    log: &ActionLog,
    batches: &BatchLock,
    states: Arc<dyn ModuleEmitter>,
    progress: &dyn ProgressEmitter,
    requests: Vec<Request>,
    mode: Mode,
) -> Result<CleanupResult, String> {
    let limits = limits(modules)?;
    // Held for the whole batch: both kinds of batch patch the one tree, and a splice built
    // against a generation another batch moved is dropped.
    let _queued = batches.enter();
    let outcome = modules.with_held(|held| {
        cleanup::execute(
            &requests,
            held,
            &limits,
            modules.system(),
            mode,
            &mut |step| progress.emit(&step),
        )
    });
    let recorded = match log.append_cleanup(&outcome) {
        Ok(()) => true,
        Err(err) => {
            eprintln!("cannot record the batch in {}: {err}", log.path().display());
            false
        }
    };
    // What went is off the screen at once, before the rediscovery has anything to say.
    let removed: Vec<String> = outcome
        .entries
        .iter()
        .filter(|entry| matches!(entry.result, EntryResult::Removed { .. }))
        .map(|entry| entry.item.clone())
        .collect();
    modules.forget(&removed);
    let tree_stale = stale_after(panic::catch_unwind(AssertUnwindSafe(|| {
        scans.patch_paths(&tree_paths(scans, &outcome))
    })));
    let asked: Vec<String> = requests
        .iter()
        .map(|request| request.item.clone())
        .collect();
    let involved = modules.modules_of(&asked);
    if !involved.is_empty() {
        modules.refresh(states, &involved);
    }
    Ok(CleanupResult {
        outcome,
        recorded,
        tree_stale,
    })
}

/// The targets of every entry that was removed or failed — a failure may have deleted part of
/// what it was about, as in 2a — in the spelling the scan recorded, which is the only one
/// `ScanManager::patch_paths` can find (phase 2b design, section 10).
fn tree_paths(scans: &ScanManager, outcome: &CleanupOutcome) -> Vec<PathBuf> {
    let Some(root) = scans.root() else {
        return Vec::new();
    };
    let resolved_root = root.canonicalize().unwrap_or_else(|_| root.clone());
    outcome
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.result,
                EntryResult::Removed { .. } | EntryResult::Failed { .. }
            )
        })
        .flat_map(|entry| &entry.targets)
        .filter_map(|target| scan_spelling(target, &root, &resolved_root))
        .collect()
}

/// `target` as the scan spelled it, or `None` when it is not under the scan's root.
///
/// A module does not spell paths the way the scan does — `git` answers `/private/tmp` for a
/// root scanned as `/tmp` — and the engine's targets are normalized besides. So the parent is
/// resolved and the resolved root's prefix swapped for the root as scanned. The remainder then
/// carries the names the disk has, which are the names the walker read: a path the tree still
/// does not know is one the scan never saw, and there is no row to patch.
fn scan_spelling(target: &Path, root: &Path, resolved_root: &Path) -> Option<PathBuf> {
    if target.starts_with(root) {
        return Some(target.to_path_buf());
    }
    let (parent, name) = (target.parent()?, target.file_name()?);
    let resolved = parent.canonicalize().ok()?.join(name);
    let rest = resolved.strip_prefix(resolved_root).ok()?;
    Some(root.join(rest))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::sync::Mutex;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::thread;
    use std::time::{Duration, Instant};

    use storage_monitor_core::action::{EntryStatus, LogResult};
    use storage_monitor_core::cleanup::StepView;
    use storage_monitor_core::module::Module;
    use storage_monitor_core::system::{Reply, System, TestSystem};
    use storage_monitor_module_demo::Demo;

    use super::*;
    use crate::scan_manager::StatusEmitter;
    use crate::views::{ModuleStatus, ModuleView, ScanState, ScanStatus};

    const WAIT: Duration = Duration::from_secs(10);

    struct States(Mutex<Sender<ModuleView>>);

    impl ModuleEmitter for States {
        fn emit(&self, view: &ModuleView) {
            let _ = self.0.lock().unwrap().send(view.clone());
        }
    }

    fn states() -> (Arc<dyn ModuleEmitter>, Receiver<ModuleView>) {
        let (tx, rx) = mpsc::channel();
        (Arc::new(States(Mutex::new(tx))), rx)
    }

    #[derive(Default)]
    struct Steps(Mutex<Vec<Progress>>);

    impl ProgressEmitter for Steps {
        fn emit(&self, progress: &Progress) {
            self.0.lock().unwrap().push(progress.clone());
        }
    }

    struct Silent;

    impl StatusEmitter for Silent {
        fn emit(&self, _event: &str, _status: &ScanStatus) {}
    }

    /// The demo module over a temporary home, discovered: its sandbox seeded where the app
    /// keeps it, `rm` and `touch` scripted to do what the real ones would, through the port.
    struct World {
        sys: Arc<TestSystem>,
        modules: ModuleManager,
        /// Outside the home, where no batch can delete the record and no scan can see it.
        data: tempfile::TempDir,
    }

    impl World {
        fn new() -> Self {
            let sys = Arc::new(TestSystem::new());
            sys.install("rm");
            sys.install("touch");
            let modules = ModuleManager::new(
                vec![Box::new(Demo) as Box<dyn Module>],
                Arc::clone(&sys) as Arc<dyn System>,
                Some(sys.root().to_path_buf()),
                sys.root()
                    .join("Library/Application Support/storage-monitor"),
            );
            let world = Self {
                sys,
                modules,
                data: tempfile::tempdir().unwrap(),
            };
            world.discover();
            world
        }

        /// Refreshes the demo and waits for the answer.
        fn discover(&self) {
            let (emitter, rx) = states();
            self.modules.refresh(emitter, &[]);
            wait_for_ready(&rx);
        }

        fn path_of(&self, title: &str) -> PathBuf {
            let (items, _) = self.modules.items(100);
            items
                .into_iter()
                .find(|item| item.title == title)
                .and_then(|item| item.path)
                .unwrap_or_else(|| panic!("no {title}"))
        }

        /// Scripts the housekeeping of a folder batch and the `rm` of an object, both doing
        /// what the real tools would.
        fn script_tools(&self) {
            let sandbox = self.path_of("build-cache").parent().unwrap().to_path_buf();
            self.sys.script(
                "touch",
                &[sandbox.join(".last-cleanup").as_os_str()],
                Reply::ok(),
            );
            let old = self.path_of("old.object");
            let port = Arc::clone(&self.sys);
            let doomed = old.clone();
            self.sys.script(
                "rm",
                &[old.as_os_str()],
                Reply::ok().then(move || port.remove(&doomed).unwrap()),
            );
        }

        fn log(&self) -> ActionLog {
            ActionLog::new(self.data.path().join("actions.jsonl"))
        }

        fn scans(&self) -> ScanManager {
            ScanManager::with_snapshots_dir(self.data.path().join("snapshots"))
        }

        fn run(&self, scans: &ScanManager, requests: Vec<Request>, mode: Mode) -> CleanupResult {
            let (emitter, _rx) = states();
            run_cleanup(
                &self.modules,
                scans,
                &self.log(),
                &BatchLock::default(),
                emitter,
                &Steps::default(),
                requests,
                mode,
            )
            .expect("the batch runs")
        }
    }

    fn wait_for_ready(rx: &Receiver<ModuleView>) -> ModuleView {
        let deadline = Instant::now() + WAIT;
        loop {
            let view = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("the module answers");
            if view.status != ModuleStatus::Discovering {
                assert_eq!(view.status, ModuleStatus::Ready, "{view:?}");
                return view;
            }
        }
    }

    fn scan(scans: &ScanManager, root: &Path) {
        scans
            .start(Arc::new(Silent), root.to_path_buf())
            .expect("the scan starts");
        let deadline = Instant::now() + WAIT;
        while scans.status().state == ScanState::Running {
            assert!(Instant::now() < deadline, "the scan did not finish");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(scans.status().state, ScanState::Done);
    }

    fn in_tree(scans: &ScanManager, path: &Path) -> bool {
        scans
            .with_result(|result, _| result.tree.find(path).is_some())
            .expect("a scan result")
    }

    fn request(item: &str, action: &str) -> Request {
        Request {
            item: format!("demo:{item}"),
            action: action.to_owned(),
            options: Vec::new(),
        }
    }

    fn two() -> Vec<Request> {
        vec![
            request("build-cache", "delete"),
            request("old.object", "remove"),
        ]
    }

    #[test]
    fn preview_answers_both_modes_aligned() {
        let world = World::new();
        let previews = preview_cleanup(&world.modules, &two()).unwrap();
        let (trash, permanent) = (&previews.trash, &previews.permanent);
        assert_eq!(trash.entries.len(), 2);
        assert_eq!(permanent.entries.len(), 2);
        assert_eq!(trash.entries[0].item, permanent.entries[0].item);
        // The folder: two steps in Trash mode, one in Permanent.
        assert_eq!(trash.entries[0].steps.len(), 2);
        assert!(matches!(trash.entries[0].steps[0], StepView::Trash { .. }));
        assert!(trash.entries[0].reversible);
        assert_eq!(permanent.entries[0].steps.len(), 1);
        assert!(matches!(
            permanent.entries[0].steps[0],
            StepView::Delete { .. }
        ));
        // The object: the same command in both, and never undone by the Trash.
        assert!(!trash.entries[1].reversible);
        assert_eq!(trash.entries[1].steps, permanent.entries[1].steps);
        assert!(
            trash
                .entries
                .iter()
                .all(|entry| entry.status == EntryStatus::Ready),
            "{trash:?}"
        );
        assert!(world.sys.ran().is_empty(), "a preview runs nothing");
    }

    #[test]
    fn a_batch_cleans_records_forgets_and_rediscovers() {
        let world = World::new();
        world.script_tools();
        let (build, old) = (world.path_of("build-cache"), world.path_of("old.object"));
        let (emitter, rx) = states();
        let result = run_cleanup(
            &world.modules,
            &world.scans(),
            &world.log(),
            &BatchLock::default(),
            emitter,
            &Steps::default(),
            two(),
            Mode::Trash,
        )
        .unwrap();

        let results: Vec<&EntryResult> = result.outcome.entries.iter().map(|e| &e.result).collect();
        assert!(
            results
                .iter()
                .all(|r| matches!(r, EntryResult::Removed { .. })),
            "{results:?}"
        );
        assert!(world.sys.trash_dir().join("build-cache").is_dir());
        assert!(!build.exists() && !old.exists());
        assert!(result.recorded);
        assert!(!result.tree_stale);

        let tail = world.log().tail(10).unwrap();
        assert_eq!(tail.entries.len(), 2);
        let source = tail.entries[0].source.as_ref().expect("a cleanup line");
        assert_eq!(source.module, "demo");
        assert_eq!(tail.entries[0].result, LogResult::Removed);
        assert_eq!(
            tail.entries[0].mode,
            Mode::Permanent,
            "the object left for good"
        );
        assert_eq!(
            tail.entries[1].mode,
            Mode::Trash,
            "the folder is in the Trash"
        );

        // Gone from the screen at once, and a rediscovery of the demo is on its way.
        let titles: Vec<String> = world
            .modules
            .items(100)
            .0
            .into_iter()
            .map(|i| i.title)
            .collect();
        assert!(!titles.contains(&"build-cache".to_owned()), "{titles:?}");
        assert!(!titles.contains(&"old.object".to_owned()), "{titles:?}");
        let ready = wait_for_ready(&rx);
        assert_eq!(ready.id, "demo");
        assert_eq!(ready.item_count, 3);
    }

    #[test]
    fn a_batch_patches_the_explorer_tree() {
        let world = World::new();
        world.script_tools();
        let scans = world.scans();
        scan(&scans, world.sys.root());
        let build = world.path_of("build-cache");
        assert!(in_tree(&scans, &build), "the scan saw it");
        let before = scans.status().bytes;

        let result = world.run(&scans, vec![request("build-cache", "delete")], Mode::Trash);
        assert!(!result.tree_stale);
        assert!(!in_tree(&scans, &build), "the row is gone");
        assert!(scans.status().bytes < before, "and the root shrank");
    }

    #[test]
    fn a_target_spelled_otherwise_is_moved_into_the_scan_spelling() {
        let world = World::new();
        world.script_tools();
        // The home scanned through a symlink: the tree spells every path through the link,
        // while the demo's paths are the resolved ones.
        let links = tempfile::tempdir().unwrap();
        let through = links.path().join("home");
        symlink(world.sys.root(), &through).unwrap();
        let scans = world.scans();
        scan(&scans, &through);
        let build = world.path_of("build-cache");
        let spelled = through.join(build.strip_prefix(world.sys.root()).unwrap());
        assert!(in_tree(&scans, &spelled), "the scan's spelling");
        assert!(!in_tree(&scans, &build), "which is not the module's");

        let result = world.run(&scans, vec![request("build-cache", "delete")], Mode::Trash);
        assert!(!result.tree_stale);
        assert!(!in_tree(&scans, &spelled), "patched all the same");
    }

    #[test]
    fn a_target_the_scan_never_saw_is_not_stale() {
        let world = World::new();
        let scans = world.scans();
        // Scanned before the sandbox existed.
        let sandbox = world.path_of("build-cache").parent().unwrap().to_path_buf();
        world.sys.remove(&sandbox).unwrap();
        scan(&scans, world.sys.root());
        world.discover();
        world.script_tools();

        let result = world.run(&scans, vec![request("build-cache", "delete")], Mode::Trash);
        assert!(matches!(
            result.outcome.entries[0].result,
            EntryResult::Removed { .. }
        ));
        assert!(
            !result.tree_stale,
            "no row was left behind, so nothing to warn about"
        );
    }

    #[test]
    fn progress_is_emitted_per_entry() {
        let world = World::new();
        world.script_tools();
        let steps = Steps::default();
        let (emitter, _rx) = states();
        run_cleanup(
            &world.modules,
            &world.scans(),
            &world.log(),
            &BatchLock::default(),
            emitter,
            &steps,
            two(),
            Mode::Trash,
        )
        .unwrap();
        let seen = steps.0.into_inner().unwrap();
        assert_eq!(
            seen.iter().map(|p| (p.done, p.total)).collect::<Vec<_>>(),
            vec![(0, 2), (1, 2), (2, 2)]
        );
        assert_eq!(seen[0].current.as_deref(), Some("build-cache"));
        assert_eq!(seen[2].current, None);
    }

    #[test]
    fn cleanup_and_explorer_batches_share_the_queue() {
        let world = World::new();
        world.script_tools();
        let batches = Arc::new(BatchLock::default());

        /// Checks, while the batch runs, that the queue both kinds of batch wait in is held.
        struct Watching(Arc<BatchLock>, Mutex<Vec<bool>>);

        impl ProgressEmitter for Watching {
            fn emit(&self, _progress: &Progress) {
                self.1.lock().unwrap().push(self.0.is_held());
            }
        }

        let watching = Watching(Arc::clone(&batches), Mutex::default());
        let (emitter, _rx) = states();
        run_cleanup(
            &world.modules,
            &world.scans(),
            &world.log(),
            &batches,
            emitter,
            &watching,
            two(),
            Mode::Trash,
        )
        .unwrap();
        let held = watching.1.into_inner().unwrap();
        assert!(
            !held.is_empty() && held.iter().all(|held| *held),
            "{held:?}"
        );
        assert!(!batches.is_held(), "and let go of afterwards");
    }

    #[test]
    fn an_unrecorded_batch_still_reports_its_outcome() {
        let world = World::new();
        world.script_tools();
        // A folder where the log's own folder should be: nothing can be appended.
        let blocked = world.data.path().join("blocked");
        fs::write(&blocked, b"a file").unwrap();
        let (emitter, _rx) = states();
        let result = run_cleanup(
            &world.modules,
            &world.scans(),
            &ActionLog::new(blocked.join("actions.jsonl")),
            &BatchLock::default(),
            emitter,
            &Steps::default(),
            two(),
            Mode::Trash,
        )
        .unwrap();
        assert!(!result.recorded);
        assert!(matches!(
            result.outcome.entries[0].result,
            EntryResult::Removed { .. }
        ));
    }

    #[test]
    fn without_a_home_folder_no_batch_runs() {
        let sys = Arc::new(TestSystem::new());
        let modules = ModuleManager::new(
            vec![Box::new(Demo) as Box<dyn Module>],
            Arc::clone(&sys) as Arc<dyn System>,
            None,
            sys.root().to_path_buf(),
        );
        assert!(preview_cleanup(&modules, &two()).is_err());
        let data = tempfile::tempdir().unwrap();
        let (emitter, _rx) = states();
        let err = run_cleanup(
            &modules,
            &ScanManager::with_snapshots_dir(data.path().join("snapshots")),
            &ActionLog::new(data.path().join("actions.jsonl")),
            &BatchLock::default(),
            emitter,
            &Steps::default(),
            two(),
            Mode::Trash,
        )
        .unwrap_err();
        assert_eq!(err, "cannot determine the home folder");
        assert!(
            !data.path().join("actions.jsonl").exists(),
            "nothing ran, nothing recorded"
        );
    }

    #[test]
    fn scan_spelling_leaves_a_path_outside_the_scan_alone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().canonicalize().unwrap().join("x");
        assert_eq!(scan_spelling(&target, &root, &root), None);
        assert_eq!(
            scan_spelling(&root.join("a/b"), &root, &root),
            Some(root.join("a/b")),
            "the scan's own spelling passes as it is"
        );
    }
}
