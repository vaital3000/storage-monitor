//! The one scan a window runs at a time: started on a worker thread, observed through
//! [`ScanManager::status`] and the `scan:progress` / `scan:done` events, persisted as a
//! snapshot when it completes and kept in memory for tree queries.

use std::any::Any;
use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use storage_monitor_core::paths;
use storage_monitor_core::scan::{
    NodeId, NodeKind, ScanOptions, ScanProgress, ScanResult, ScanStats, Tree, replace_subtrees,
    rescan_path, scan,
};
use storage_monitor_core::snapshot::{
    DEFAULT_FILE_THRESHOLD, DEFAULT_KEEP, Delta, Snapshot, SnapshotStore, deltas, top_growers,
};

use crate::views::{ScanState, ScanStatus};

/// Files smaller than this are left out of snapshots.
/// Snapshots kept in the store, across all roots.
/// Growers computed when a scan finishes; the `top_growers` command takes a prefix.
pub const GROWERS_KEPT: usize = 50;
/// Default interval of the `scan:progress` events.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

pub const PROGRESS_EVENT: &str = "scan:progress";
pub const DONE_EVENT: &str = "scan:done";

/// Where the manager's events go: the Tauri app in production, a recorder in tests.
pub trait StatusEmitter: Send + Sync + 'static {
    fn emit(&self, event: &str, status: &ScanStatus);
}

impl StatusEmitter for tauri::AppHandle {
    fn emit(&self, event: &str, status: &ScanStatus) {
        if let Err(err) = tauri::Emitter::emit(self, event, status) {
            eprintln!("cannot emit {event}: {err}");
        }
    }
}

#[derive(Clone)]
pub struct ScanManager {
    inner: Arc<Mutex<Inner>>,
    snapshots_dir: PathBuf,
    progress_interval: Duration,
}

#[derive(Default)]
struct Inner {
    state: ScanState,
    /// Bumped by every `start`, so the ticker of an earlier scan stops.
    generation: u64,
    root: Option<PathBuf>,
    progress: Arc<ScanProgress>,
    started: Option<Instant>,
    duration_ms: u64,
    result: Option<Arc<ScanResult>>,
    previous_sizes: Option<Arc<HashMap<String, u64>>>,
    previous_taken_at: Option<DateTime<Utc>>,
    growers: Vec<Delta>,
    error: Option<String>,
}

impl Default for ScanManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanManager {
    /// Snapshots go to [`paths::snapshots_dir`].
    pub fn new() -> Self {
        Self::with_snapshots_dir(paths::snapshots_dir())
    }

    pub fn with_snapshots_dir(snapshots_dir: PathBuf) -> Self {
        Self {
            inner: Arc::default(),
            snapshots_dir,
            progress_interval: PROGRESS_INTERVAL,
        }
    }

    /// Emits `scan:progress` every `interval` instead of [`PROGRESS_INTERVAL`].
    #[cfg(test)]
    fn with_progress_interval(mut self, interval: Duration) -> Self {
        self.progress_interval = interval;
        self
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Live counters while a scan runs, the final numbers afterwards.
    pub fn status(&self) -> ScanStatus {
        self.lock().status()
    }

    /// Root of the current or last scan.
    pub fn root(&self) -> Option<PathBuf> {
        self.lock().root.clone()
    }

    /// Starts a scan of `root` on a worker thread. The status is emitted as
    /// `scan:progress` every [`PROGRESS_INTERVAL`] while the scan runs and once as
    /// `scan:done` when it is over, whatever the outcome. Both events are emitted
    /// while the manager is locked, so nothing follows `scan:done`.
    pub fn start(
        &self,
        emitter: Arc<dyn StatusEmitter>,
        root: PathBuf,
    ) -> Result<ScanStatus, String> {
        let (progress, generation, previous) = {
            let mut inner = self.lock();
            if inner.state == ScanState::Running {
                return Err("a scan is already running".to_owned());
            }
            let progress = Arc::new(ScanProgress::default());
            let generation = inner.generation + 1;
            let previous = std::mem::replace(
                &mut *inner,
                Inner {
                    state: ScanState::Running,
                    generation,
                    root: Some(root.clone()),
                    progress: Arc::clone(&progress),
                    started: Some(Instant::now()),
                    ..Inner::default()
                },
            );
            (progress, generation, previous)
        };
        // The previous tree can be hundreds of megabytes: free it outside the lock.
        drop(previous);
        let worker = {
            let manager = self.clone();
            let emitter = Arc::clone(&emitter);
            move || manager.run(root, &progress, &*emitter)
        };
        if let Err(err) = thread::Builder::new().name("scan".into()).spawn(worker) {
            let message = format!("cannot start the scan thread: {err}");
            self.lock().fail(message.clone());
            return Err(message);
        }
        let ticker = {
            let manager = self.clone();
            move || manager.tick(generation, &*emitter)
        };
        if let Err(err) = thread::Builder::new()
            .name("scan-progress".into())
            .spawn(ticker)
        {
            // The scan still runs and the UI can poll; only the events are missing.
            eprintln!("cannot start the progress thread: {err}");
        }
        Ok(self.status())
    }

    /// Asks the running scan to stop; the state becomes `Cancelled` once the worker has
    /// finished with the partial tree.
    pub fn cancel(&self) -> ScanStatus {
        let inner = self.lock();
        if inner.state == ScanState::Running {
            inner.progress.cancel();
        }
        inner.status()
    }

    /// Runs `f` on the last result and the size index of the previous snapshot; `None`
    /// while there is no result (idle, running or failed).
    pub fn with_result<T>(
        &self,
        f: impl FnOnce(&ScanResult, Option<&HashMap<String, u64>>) -> T,
    ) -> Option<T> {
        let (result, previous) = {
            let inner = self.lock();
            (
                Arc::clone(inner.result.as_ref()?),
                inner.previous_sizes.clone(),
            )
        };
        Some(f(&result, previous.as_deref()))
    }

    /// The largest growers since the previous snapshot, at most `limit` of the
    /// [`GROWERS_KEPT`] computed when the scan finished.
    pub fn growers(&self, limit: usize) -> Vec<Delta> {
        self.lock().growers.iter().take(limit).cloned().collect()
    }

    /// Which arena the manager holds. Bumped by every scan and by every splice, because
    /// both replace every id in it. Not on the wire: the UI counts its own generations from
    /// `scan:done`, and this one exists so that a patch can tell whether the ids it was
    /// built from are still the manager's.
    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Rescans every path of a finished batch and splices the results into the tree, so the
    /// Explorer agrees with the disk again. Paths the tree does not know are ignored, and
    /// nothing happens at all while no scan result is held — during a scan there is none,
    /// and the scan itself is about to produce a truthful tree. All three of those are
    /// [`TreeState::Current`], the unresolved path included: there is no row on screen for
    /// what the batch deleted, so there is nothing to warn about. That last one is a
    /// positive answer about a lookup that failed, and it is only true while the caller
    /// hands over the spelling the scan recorded — the rule the next paragraph is about.
    ///
    /// **The paths are the ones the UI sent.** [`Tree::find`] matches the spelling the scan
    /// recorded, component by component; the normalized path the guards hand to the `System`
    /// port is a different value whenever the scan root was reached through a symlink or
    /// spelled otherwise than the disk does. Patching by that one is silent — no splice, and
    /// the Explorer goes on showing a directory that is gone.
    ///
    /// Three steps, and the middle one is why they are three:
    ///
    /// 1. under the lock: take the result and resolve the paths to ids;
    /// 2. **off the lock**: rescan every one of them. A rescan walks the disk and can take
    ///    seconds, and every tree query of the window goes through the same lock;
    /// 3. under the lock again: splice, but only if the arena from step 1 is still the one
    ///    the manager holds — a scan that finished meanwhile has replaced every id in it.
    ///
    /// The splice is one call for the whole batch: [`replace_subtrees`] rebuilds the arena,
    /// so its cost is per call and not per path, and ids resolved before it mean nothing
    /// after it.
    ///
    /// Call it from a worker, never from a [`StatusEmitter`]: the rebuild carries a live
    /// `assert!` and takes the lock, and an emitter runs while the manager is already
    /// locked. A panic in it poisons the lock, which [`Self::lock`] recovers from, and
    /// leaves the previous result in place — the tree stays stale rather than broken. The
    /// caller catches that panic and reports the staleness; see `actions::run_batch`.
    #[must_use = "a stale tree is something the window has to tell the user about"]
    pub fn patch_paths(&self, paths: &[PathBuf]) -> TreeState {
        let Some((result, generation, ids)) = self.patch_targets(paths) else {
            return TreeState::Current;
        };
        // Outside the lock. `rescan_targets` is a free function so that it cannot reach the
        // manager even by accident: the window has to stay readable while the disk is walked.
        let (patches, complete) = rescan_targets(&result, &ids);
        TreeState::of(self.install_patches(generation, patches), complete)
    }

    /// Step 1: the result to rescan against, the generation it belongs to, and the ids of
    /// the paths this tree knows. `None` when there is nothing to do.
    ///
    /// Only the two reads are under the lock. [`Tree::find`] walks one sibling group per
    /// component, and a sibling group here is as wide as the directory: the 500 rows a
    /// selection can hold, in a directory with a million children, is seconds of a frozen
    /// window right after a deletion. `plan_for` resolves the same paths off the lock
    /// already, and step 3 re-checks the generation, so nothing is weakened by reading the
    /// tree through the `Arc` instead.
    fn patch_targets(&self, paths: &[PathBuf]) -> Option<(Arc<ScanResult>, u64, Vec<NodeId>)> {
        let (result, generation) = {
            let inner = self.lock();
            (Arc::clone(inner.result.as_ref()?), inner.generation)
        };
        let mut ids: Vec<NodeId> = paths
            .iter()
            .filter_map(|path| result.tree.find(path))
            // The root is not patchable — an arena without one is not a tree — and
            // `replace_subtrees` drops such a patch. Dropping it here instead is what keeps
            // that from costing a full rescan of the scan root first, which is the
            // 25-second walk this whole mechanism exists to avoid. No batch can produce it:
            // the guards refuse the root before anything is touched.
            .filter(|&id| id != Tree::ROOT)
            .collect();
        // One rescan per node: the same directory can arrive twice under two spellings, and
        // `Tree::find` accepts more than one of them (`a/b` and `a/b/.` name one node).
        // Cost only, and only for a direct caller: through `run_batch` the second spelling
        // is blocked as `Nested` long before it gets here, and `replace_subtrees` keeps the
        // last patch for a repeated id either way. Nothing about safety rests on it.
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() {
            return None;
        }
        Some((result, generation, ids))
    }

    /// Step 3: the whole batch in one splice, or nothing. `true` when the tree changed —
    /// `false` says the patch was dropped, which is what leaves the tree stale.
    ///
    /// The rebuild itself runs under the lock — a copy of the arena, a few hundred
    /// milliseconds for a few million nodes — because the tree it is built from has to be
    /// the one it replaces. Installing it bumps the generation: every id in the arena is a
    /// new id now, and a reader holding the old ones is reading a tree that no longer
    /// exists.
    fn install_patches(&self, generation: u64, patches: Vec<(NodeId, Option<Tree>)>) -> bool {
        if patches.is_empty() {
            return false;
        }
        let mut inner = self.lock();
        if inner.generation != generation {
            return false;
        }
        let Some(current) = inner.result.clone() else {
            return false;
        };
        let tree = replace_subtrees(&current.tree, patches);
        let patched = ScanResult {
            root: current.root.clone(),
            started_at: current.started_at,
            duration_ms: current.duration_ms,
            stats: patched_stats(&current.stats, &tree),
            cancelled: current.cancelled,
            tree,
        };
        inner.result = Some(Arc::new(patched));
        inner.generation += 1;
        // The lock goes before `current` does. `current` is a strong reference to the arena
        // that was just replaced — hundreds of megabytes of it — and locals drop in reverse
        // declaration order, so without this line its refcount falls while the guard is
        // still alive. Today that costs nothing, because `patch_paths` holds the same `Arc`
        // until it returns and the deallocation happens there instead; but that is a
        // property of the caller's frame, and this function should not need one to be
        // right. `start` releases the lock before dropping the previous tree for the same
        // reason.
        drop(inner);
        true
    }

    /// Body of the worker thread: a panic anywhere in the scan or the store becomes a
    /// `Failed` state, so the manager can never be stuck in `Running`. The scan and the
    /// store run unlocked; `scan:done` is emitted under the lock, so a ticker that wakes
    /// up afterwards finds the scan over and stays quiet.
    fn run(&self, root: PathBuf, progress: &ScanProgress, emitter: &dyn StatusEmitter) {
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
            scan(&scan_options(root), progress)
                .map(|result| Finished::persist(result, &self.snapshots_dir))
        }));
        let mut inner = self.lock();
        match outcome {
            Ok(Ok(finished)) => inner.complete(finished),
            Ok(Err(err)) => inner.fail(err.to_string()),
            Err(payload) => inner.fail(panic_message(&*payload)),
        }
        emitter.emit(DONE_EVENT, &inner.status());
    }

    /// Body of the progress thread: emits the status every `progress_interval` while the
    /// scan of `generation` runs. The status is emitted under the lock, so no progress
    /// event can follow `scan:done`; the emitter never re-enters the manager.
    fn tick(&self, generation: u64, emitter: &dyn StatusEmitter) {
        loop {
            thread::sleep(self.progress_interval);
            let inner = self.lock();
            if inner.generation != generation || inner.state != ScanState::Running {
                return;
            }
            emitter.emit(PROGRESS_EVENT, &inner.status());
        }
    }
}

/// A scan that ran to its end, with what the snapshot store knew about its root. Built
/// on the worker thread, outside the lock, so installing it is cheap.
struct Finished {
    result: ScanResult,
    previous_taken_at: Option<DateTime<Utc>>,
    /// Sizes by path of the previous snapshot of the same root.
    previous_sizes: Option<Arc<HashMap<String, u64>>>,
    growers: Vec<Delta>,
}

impl Finished {
    /// Saves a snapshot of a complete scan and compares it with the previous one of the
    /// same root. A cancelled scan is partial: it is neither saved nor compared. Store
    /// failures are logged and never fail the scan.
    fn persist(result: ScanResult, snapshots_dir: &Path) -> Self {
        if result.cancelled {
            return Self {
                result,
                previous_taken_at: None,
                previous_sizes: None,
                growers: Vec::new(),
            };
        }
        let store = SnapshotStore::new(snapshots_dir.to_path_buf());
        let previous = store.latest_for(&result.root).unwrap_or_else(|err| {
            eprintln!("cannot load the previous snapshot: {err}");
            None
        });
        let current = Snapshot::from_result(&result, DEFAULT_FILE_THRESHOLD);
        if let Err(err) = store.save(&current) {
            eprintln!("cannot save the snapshot: {err}");
        }
        if let Err(err) = store.prune(DEFAULT_KEEP) {
            eprintln!("cannot prune the snapshots: {err}");
        }
        let growers = previous
            .as_ref()
            .map(|p| top_growers(&deltas(p, &current), GROWERS_KEPT))
            .unwrap_or_default();
        Self {
            result,
            previous_taken_at: previous.as_ref().map(|p| p.taken_at),
            previous_sizes: previous.map(|p| Arc::new(p.size_index())),
            growers,
        }
    }
}

impl Inner {
    fn complete(&mut self, finished: Finished) {
        self.state = if finished.result.cancelled {
            ScanState::Cancelled
        } else {
            ScanState::Done
        };
        self.duration_ms = finished.result.duration_ms;
        self.previous_taken_at = finished.previous_taken_at;
        self.previous_sizes = finished.previous_sizes;
        self.growers = finished.growers;
        self.result = Some(Arc::new(finished.result));
    }

    fn fail(&mut self, message: String) {
        self.state = ScanState::Failed;
        self.error = Some(message);
        self.duration_ms = self.elapsed_ms();
    }

    fn elapsed_ms(&self) -> u64 {
        self.started
            .map_or(0, |started| started.elapsed().as_millis() as u64)
    }

    fn status(&self) -> ScanStatus {
        let running = self.state == ScanState::Running;
        let live = self.progress.snapshot();
        let (files, dirs, bytes, errors) = match &self.result {
            Some(result) => {
                let stats = &result.stats;
                (stats.files, stats.dirs, stats.bytes, stats.errors)
            }
            None => (live.files, live.dirs, live.bytes, live.errors),
        };
        ScanStatus {
            state: self.state,
            root: self
                .root
                .as_ref()
                .map(|root| root.to_string_lossy().into_owned()),
            files,
            dirs,
            bytes,
            errors,
            current_path: if running {
                live.current_path
            } else {
                String::new()
            },
            duration_ms: if running {
                self.elapsed_ms()
            } else {
                self.duration_ms
            },
            error: self.error.clone(),
            has_previous: self.previous_sizes.is_some(),
            previous_taken_at: self.previous_taken_at,
        }
    }
}

/// Whether the tree the manager holds can still be shown as a description of the disk.
///
/// A deletion is the one thing that makes the difference visible: the row is either gone
/// from the Explorer or it is a row for something that is not there any more, and only the
/// code that tried to patch it knows which. It is an enum rather than the `bool` it wraps
/// because the two booleans it is built from run the other way — `install_patches` returns
/// `true` for *installed* — and a silent inversion here would report the good case as the
/// bad one for ever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeState {
    /// The tree describes the disk as well as it did before the batch.
    Current,
    /// At least one row the batch deleted is still in the tree, and only a scan can put it
    /// right.
    Stale,
}

impl TreeState {
    /// `installed`: the splice landed. `complete`: every path of the batch produced a patch
    /// to splice. Anything less than both is a tree with a row nobody can act on.
    fn of(installed: bool, complete: bool) -> Self {
        if installed && complete {
            Self::Current
        } else {
            Self::Stale
        }
    }
}

/// Step 2 of [`ScanManager::patch_paths`]: a fresh tree for every id, walked off the lock,
/// and whether every one of them could be looked at.
///
/// Takes the result rather than the manager, so that the step that walks the disk has no
/// way to reach the lock the other two steps take.
///
/// A path that cannot be rescanned is left out of the batch and its branch stays as the
/// scan left it: a rescan fails when nobody can read the path, and a tree that cannot be
/// read is not a tree that can be said to be gone. That is the `false` in the second half
/// of the answer — the branch is still there and may name something that is not. A path
/// that *is* gone is not a failure: it comes back as `None`, which is the patch that drops
/// the branch.
fn rescan_targets(result: &ScanResult, ids: &[NodeId]) -> (Vec<(NodeId, Option<Tree>)>, bool) {
    let options = scan_options(result.root.clone());
    let mut patches = Vec::with_capacity(ids.len());
    let mut complete = true;
    for &id in ids {
        let path = result.tree.path(id);
        match rescan_path(&path, &options) {
            Ok(replacement) => patches.push((id, replacement)),
            Err(err) => {
                eprintln!("cannot rescan {}: {err}", path.display());
                complete = false;
            }
        }
    }
    (patches, complete)
}

/// The options every scan of this app runs with, and every rescan that patches one.
///
/// One function for both, because a rescan that did not keep the rules of the scan it
/// patches would splice in a branch the scan itself would never have produced — a volume it
/// refused to enter, or a directory it was told to exclude. Add an option here, not at one
/// of the two call sites.
fn scan_options(root: PathBuf) -> ScanOptions {
    ScanOptions::new(root)
}

/// The stats of a scan whose tree has just been patched.
///
/// `bytes`, `files` and `dirs` describe the tree, and the tree has changed: `ScanStats::bytes`
/// is documented as the root node's size, and a header that disagrees with the rows below it
/// — or with the free space the batch just gave back — is a header nobody can trust. They are
/// recomputed here, the first two off the root node and the third in one pass over the arena,
/// which is noise next to the rebuild that just ran.
///
/// `errors` and `hardlinks_skipped` describe the walk instead, like the `duration_ms` and
/// `started_at` kept beside them: a patch has nothing to say about what a scan met on its way
/// or how long it took, and pretending otherwise would date the whole scan to the deletion.
/// The one number a patch can move the wrong way is `bytes`, and not through anything here:
/// a rescan re-attributes hard links inside the branch alone, so a surviving link whose twin
/// lives outside it takes its bytes back and the total *grows* after a deletion (see
/// [`rescan_path`], and the design's section 6). The next full scan puts it right.
fn patched_stats(stats: &ScanStats, tree: &Tree) -> ScanStats {
    ScanStats {
        // Widening, not narrowing: `file_count` is a `u32` because that is what the walker
        // counts into, so a tree big enough to overflow it has overflowed there first.
        files: u64::from(tree.root().file_count),
        dirs: tree
            .iter()
            .filter(|(_, node)| node.kind == NodeKind::Dir)
            .count() as u64,
        bytes: tree.root().size,
        errors: stats.errors,
        hardlinks_skipped: stats.hardlinks_skipped,
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    format!("the scan panicked: {}", panic_text(payload))
}

/// What a panic payload says, for a caller that has its own sentence to put it in.
pub(crate) fn panic_text(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    use storage_monitor_core::scan::{Node, NodeKind, ScanStats, Subtree, Tree};

    use super::*;

    type Events = Mutex<Vec<(String, ScanStatus)>>;

    impl StatusEmitter for Events {
        fn emit(&self, event: &str, status: &ScanStatus) {
            self.lock()
                .unwrap()
                .push((event.to_owned(), status.clone()));
        }
    }

    /// Records like `Events`, but a progress emit first takes `delay`, like a real emitter
    /// serializing the payload and crossing to the webview. That is the window in which
    /// a `running` status read outside the lock would overtake `scan:done`.
    struct SlowProgressEvents {
        delay: Duration,
        events: Events,
    }

    impl StatusEmitter for SlowProgressEvents {
        fn emit(&self, event: &str, status: &ScanStatus) {
            if event == PROGRESS_EVENT {
                thread::sleep(self.delay);
            }
            self.events.emit(event, status);
        }
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, size) in [
            ("a/one.bin", 30_000usize),
            ("a/two.bin", 10_000),
            ("b.bin", 5_000),
        ] {
            let path = dir.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::File::create(&path)
                .unwrap()
                .write_all(&vec![0u8; size])
                .unwrap();
        }
        dir
    }

    /// `files` one-byte files spread over 50 folders: enough for a scan to outlast a few
    /// ticks of a fast ticker.
    fn wide_fixture(files: usize) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..files {
            let folder = dir.path().join(format!("d{}", i % 50));
            fs::create_dir_all(&folder).unwrap();
            fs::write(folder.join(format!("f{i}")), b"x").unwrap();
        }
        dir
    }

    fn manager_in(data: &Path) -> ScanManager {
        ScanManager::with_snapshots_dir(data.join("snapshots"))
    }

    fn wait_until_finished(manager: &ScanManager) -> ScanStatus {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = manager.status();
            if status.state != ScanState::Running {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the scan did not finish: {status:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn root_children(manager: &ScanManager) -> Vec<String> {
        manager
            .with_result(|result, _| {
                let tree: &Tree = &result.tree;
                tree.children(Tree::ROOT)
                    .map(|id| tree.get(id).unwrap().name.to_string())
                    .collect()
            })
            .unwrap()
    }

    #[test]
    fn a_scan_runs_on_a_thread_saves_a_snapshot_and_emits_done() {
        let data = tempfile::tempdir().unwrap();
        let fixture = fixture();
        let manager = manager_in(data.path());
        let events: Arc<Events> = Arc::default();
        assert_eq!(manager.status().state, ScanState::Idle);
        assert!(manager.status().root.is_none());

        let status = manager
            .start(events.clone(), fixture.path().to_path_buf())
            .unwrap();
        assert_eq!(status.state, ScanState::Running);
        assert_eq!(status.root.as_deref(), fixture.path().to_str());
        assert_eq!(manager.root().as_deref(), Some(fixture.path()));

        let status = wait_until_finished(&manager);
        assert_eq!(status.state, ScanState::Done, "{status:?}");
        assert_eq!((status.files, status.dirs, status.errors), (3, 2, 0));
        assert!(status.bytes > 0);
        assert_eq!(status.current_path, "");
        assert_eq!(status.error, None);
        assert!(!status.has_previous);
        assert!(status.previous_taken_at.is_none());
        assert_eq!(root_children(&manager), vec!["a", "b.bin"]);
        assert!(
            manager
                .with_result(|_, previous| previous.is_none())
                .unwrap()
        );
        assert!(manager.growers(10).is_empty());

        let files = file_names(&data.path().join("snapshots"));
        assert_eq!(files.len(), 2, "{files:?}");
        assert!(files[0].ends_with(".json") && files[1].ends_with(".snap"));

        let events = events.lock().unwrap();
        let (last_event, last_status) = events.last().unwrap();
        assert_eq!(last_event, DONE_EVENT);
        assert_eq!(last_status.state, ScanState::Done);
        // A scan this small may finish before the first tick.
        let progress = &events[..events.len() - 1];
        assert!(
            progress.iter().all(
                |(event, status)| event == PROGRESS_EVENT && status.state == ScanState::Running
            ),
            "{events:?}"
        );
    }

    #[test]
    fn no_event_follows_done_however_fast_the_ticker_runs() {
        // The property under test is "no scan:progress after scan:done". Whether the
        // ticker gets to emit at all before a 3000-file scan finishes depends on the
        // machine, so the scenario is repeated until a run with at least one tick is
        // observed; the ordering property is asserted on every run.
        let mut saw_a_tick = false;
        for _attempt in 0..10 {
            let data = tempfile::tempdir().unwrap();
            let fixture = wide_fixture(3_000);
            let manager = manager_in(data.path()).with_progress_interval(Duration::from_millis(1));
            let emitter = Arc::new(SlowProgressEvents {
                delay: Duration::from_millis(5),
                events: Events::default(),
            });
            manager
                .start(emitter.clone(), fixture.path().to_path_buf())
                .unwrap();
            assert_eq!(wait_until_finished(&manager).state, ScanState::Done);
            // Give a ticker that woke up around the end of the scan the time to emit.
            thread::sleep(Duration::from_millis(50));

            let events = emitter.events.lock().unwrap();
            let done = events
                .iter()
                .position(|(event, _)| event == DONE_EVENT)
                .expect("scan:done was emitted");
            assert_eq!(
                done,
                events.len() - 1,
                "events after scan:done: {:?}",
                &events[done + 1..]
            );
            assert!(
                events[..done].iter().all(|(event, status)| {
                    event == PROGRESS_EVENT && status.state == ScanState::Running
                }),
                "{events:?}"
            );
            assert_eq!(events[done].1.state, ScanState::Done);
            assert_eq!(events[done].1.files, 3_000);
            if done > 0 {
                saw_a_tick = true;
                break;
            }
        }
        assert!(
            saw_a_tick,
            "no run had a progress tick before scan:done in 10 attempts; the fixture is too small"
        );
    }

    #[test]
    fn the_next_scan_of_the_same_root_sees_the_previous_snapshot() {
        let data = tempfile::tempdir().unwrap();
        let fixture = fixture();
        let manager = manager_in(data.path());
        let root = fixture.path().to_path_buf();
        manager
            .start(Arc::new(Events::default()), root.clone())
            .unwrap();
        assert_eq!(wait_until_finished(&manager).state, ScanState::Done);

        fs::File::create(root.join("a/three.bin"))
            .unwrap()
            .write_all(&vec![0u8; 40_000])
            .unwrap();
        manager
            .start(Arc::new(Events::default()), root.clone())
            .unwrap();
        let status = wait_until_finished(&manager);
        assert_eq!(status.state, ScanState::Done, "{status:?}");
        assert!(status.has_previous);
        assert!(status.previous_taken_at.is_some());
        assert_eq!(status.files, 4);
        let previous_root = manager
            .with_result(|_, previous| previous.unwrap().get(root.to_str().unwrap()).copied())
            .unwrap();
        assert!(
            previous_root.is_some(),
            "the previous snapshot indexes the root"
        );
        let growers = manager.growers(10);
        assert_eq!(growers.len(), 1, "{growers:?}");
        assert_eq!(growers[0].path, root.join("a").to_str().unwrap());
        assert!(growers[0].delta > 0);
        assert!(manager.growers(0).is_empty());
        assert_eq!(file_names(&data.path().join("snapshots")).len(), 4);
    }

    #[test]
    fn a_missing_root_fails_and_leaves_the_store_alone() {
        let data = tempfile::tempdir().unwrap();
        let manager = manager_in(data.path());
        let events: Arc<Events> = Arc::default();
        manager
            .start(events.clone(), PathBuf::from("/definitely/missing"))
            .unwrap();
        let status = wait_until_finished(&manager);
        assert_eq!(status.state, ScanState::Failed);
        assert!(
            status.error.as_deref().unwrap().contains("cannot read"),
            "{status:?}"
        );
        assert!(manager.with_result(|_, _| ()).is_none());
        assert!(!data.path().join("snapshots").exists());
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, DONE_EVENT);
        assert_eq!(events[0].1.state, ScanState::Failed);
    }

    #[test]
    fn start_refuses_a_second_scan_while_one_runs() {
        let data = tempfile::tempdir().unwrap();
        let manager = manager_in(data.path());
        manager.lock().state = ScanState::Running;
        let err = manager
            .start(Arc::new(Events::default()), PathBuf::from("/"))
            .unwrap_err();
        assert_eq!(err, "a scan is already running");
        assert_eq!(manager.status().state, ScanState::Running);
    }

    #[test]
    fn cancel_while_idle_changes_nothing() {
        let data = tempfile::tempdir().unwrap();
        let manager = manager_in(data.path());
        assert_eq!(manager.cancel().state, ScanState::Idle);
        assert!(manager.with_result(|_, _| ()).is_none());
    }

    #[test]
    fn a_ticker_of_an_earlier_scan_stops_without_emitting() {
        let data = tempfile::tempdir().unwrap();
        let manager = manager_in(data.path());
        {
            let mut inner = manager.lock();
            inner.state = ScanState::Running;
            inner.generation = 2;
        }
        let events = Events::default();
        manager.tick(1, &events);
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    fn a_cancelled_scan_is_neither_saved_nor_compared() {
        let data = tempfile::tempdir().unwrap();
        let tree = Subtree::with_children(
            Node::new("/root", NodeKind::Dir, 5, 5, 1, 0),
            vec![Subtree::new(Node::new("f", NodeKind::File, 5, 5, 1, 0))],
        )
        .flatten()
        .0;
        let result = ScanResult {
            root: "/root".into(),
            started_at: Utc::now(),
            duration_ms: 1,
            stats: ScanStats::default(),
            cancelled: true,
            tree,
        };
        let finished = Finished::persist(result, &data.path().join("snapshots"));
        assert!(finished.previous_sizes.is_none());
        assert!(finished.previous_taken_at.is_none());
        assert!(finished.growers.is_empty());
        assert!(!data.path().join("snapshots").exists());
        let mut inner = Inner::default();
        inner.complete(finished);
        assert_eq!(inner.state, ScanState::Cancelled);
        assert_eq!(inner.status().files, 0);
    }

    #[test]
    fn persist_indexes_the_previous_snapshot_before_the_result_is_installed() {
        let data = tempfile::tempdir().unwrap();
        let snapshots = data.path().join("snapshots");
        let result = || {
            let tree = Subtree::with_children(
                Node::new("/root", NodeKind::Dir, 5, 5, 1, 0),
                vec![Subtree::new(Node::new("f", NodeKind::File, 5, 5, 1, 0))],
            )
            .flatten()
            .0;
            ScanResult {
                root: "/root".into(),
                started_at: Utc::now(),
                duration_ms: 1,
                stats: ScanStats::default(),
                cancelled: false,
                tree,
            }
        };
        let first = Finished::persist(result(), &snapshots);
        assert!(first.previous_sizes.is_none());
        let second = Finished::persist(result(), &snapshots);
        let sizes = second
            .previous_sizes
            .as_ref()
            .expect("index of the first snapshot");
        assert_eq!(sizes.get("/root"), Some(&5));
        assert!(second.previous_taken_at.is_some());
        let mut inner = Inner::default();
        inner.complete(second);
        assert_eq!(inner.state, ScanState::Done);
        assert!(inner.status().has_previous);
    }

    /// Everything that is deleted while the window is open goes through this: a scan, a
    /// deletion on disk, then the patch.
    fn scanned_fixture() -> (ScanManager, tempfile::TempDir, tempfile::TempDir) {
        let data = tempfile::tempdir().unwrap();
        let fixture = fixture();
        let manager = manager_in(data.path());
        manager
            .start(Arc::new(Events::default()), fixture.path().to_path_buf())
            .unwrap();
        assert_eq!(wait_until_finished(&manager).state, ScanState::Done);
        (manager, fixture, data)
    }

    fn root_child(manager: &ScanManager, name: &str) -> Option<u64> {
        manager
            .with_result(|result, _| {
                let tree = &result.tree;
                tree.children(Tree::ROOT)
                    .find(|id| &*tree.get(*id).unwrap().name == name)
                    .map(|id| tree.get(id).unwrap().size)
            })
            .unwrap()
    }

    #[test]
    fn patching_a_deleted_path_drops_the_branch_and_bumps_the_generation() {
        let (manager, fixture, _data) = scanned_fixture();
        let before = manager
            .with_result(|result, _| result.tree.root().size)
            .unwrap();
        let a = root_child(&manager, "a").expect("the fixture has a/");
        let generation = manager.lock().generation;

        fs::remove_dir_all(fixture.path().join("a")).unwrap();
        assert_eq!(
            manager.patch_paths(&[fixture.path().join("a")]),
            TreeState::Current,
            "the splice landed, so the tree describes the disk again"
        );

        assert_eq!(root_child(&manager, "a"), None, "the branch is gone");
        assert_eq!(root_children(&manager), vec!["b.bin"]);
        assert_eq!(
            manager
                .with_result(|result, _| result.tree.root().size)
                .unwrap(),
            before - a,
        );
        assert_eq!(
            manager.lock().generation,
            generation + 1,
            "every id in the arena changed"
        );
    }

    #[test]
    fn a_patch_leaves_the_stats_of_the_scan_that_ran_and_recomputes_the_tree_facts() {
        let (manager, fixture, _data) = scanned_fixture();
        let before = manager
            .with_result(|result, _| result.stats.clone())
            .unwrap();
        let duration_ms = manager.status().duration_ms;
        assert_eq!((before.files, before.dirs), (3, 2));

        fs::remove_dir_all(fixture.path().join("a")).unwrap();
        assert_eq!(
            manager.patch_paths(&[fixture.path().join("a")]),
            TreeState::Current
        );

        let status = manager.status();
        let after = manager
            .with_result(|result, _| result.stats.clone())
            .unwrap();
        assert_eq!(after.files, 1, "two files left the tree");
        assert_eq!(after.dirs, 1, "and so did one directory");
        assert_eq!(
            after.bytes,
            manager
                .with_result(|result, _| result.tree.root().size)
                .unwrap(),
            "the documented invariant: bytes is the root node's size"
        );
        assert!(after.bytes < before.bytes);
        assert_eq!(
            (after.errors, after.hardlinks_skipped),
            (before.errors, before.hardlinks_skipped),
            "what the walk met is not the patch's to change"
        );
        assert_eq!(
            (status.files, status.dirs, status.bytes),
            (after.files, after.dirs, after.bytes),
            "the header the UI reads says the same"
        );
        assert_eq!(
            status.duration_ms, duration_ms,
            "the walk took what it took"
        );
    }

    /// The other half of the rule, on numbers that are not zero: what the walk met on its
    /// way is not the patch's to change. An unreadable directory counts one error per entry
    /// it could not read, which is not the same as the one node that carries the message —
    /// recomputing either of these from the tree would quietly change the header.
    #[test]
    fn a_patch_keeps_what_the_walk_met() {
        if crate::running_as_root() {
            return;
        }
        let data = tempfile::tempdir().unwrap();
        let fixture = fixture();
        // Two entries that can be listed but not stat'ed: 2 errors, 1 error node.
        let locked = fixture.path().join("locked");
        fs::create_dir(&locked).unwrap();
        for name in ["x.bin", "y.bin"] {
            fs::write(locked.join(name), b"x").unwrap();
        }
        // And one hard link, so `hardlinks_skipped` is not zero either.
        fs::hard_link(
            fixture.path().join("b.bin"),
            fixture.path().join("twin.bin"),
        )
        .unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o444)).unwrap();
        let manager = manager_in(data.path());
        manager
            .start(Arc::new(Events::default()), fixture.path().to_path_buf())
            .unwrap();
        let finished = wait_until_finished(&manager);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(finished.state, ScanState::Done, "{finished:?}");
        let before = manager
            .with_result(|result, _| result.stats.clone())
            .unwrap();
        assert_eq!(before.errors, 2, "two entries could not be read");
        assert_eq!(before.hardlinks_skipped, 1);

        fs::remove_dir_all(fixture.path().join("a")).unwrap();
        assert_eq!(
            manager.patch_paths(&[fixture.path().join("a")]),
            TreeState::Current
        );

        let after = manager
            .with_result(|result, _| result.stats.clone())
            .unwrap();
        assert_eq!(
            after.errors, 2,
            "the walk met two, and the error table holds one"
        );
        assert_eq!(after.hardlinks_skipped, 1);
        assert_eq!(after.files, before.files - 2, "a/ took two files with it");
        assert_eq!(after.dirs, before.dirs - 1);
    }

    /// The stats recomputed from an untouched tree are the ones the walk counted; without
    /// that, every patch would shift the header by whatever the two rules disagree about.
    #[test]
    fn the_recomputed_tree_facts_agree_with_the_walker() {
        let data = tempfile::tempdir().unwrap();
        let fixture = wide_fixture(200);
        let manager = manager_in(data.path());
        manager
            .start(Arc::new(Events::default()), fixture.path().to_path_buf())
            .unwrap();
        assert_eq!(wait_until_finished(&manager).state, ScanState::Done);
        manager
            .with_result(|result, _| {
                let patched = patched_stats(&result.stats, &result.tree);
                assert_eq!(patched.bytes, result.stats.bytes);
                assert_eq!(patched.files, result.stats.files);
                assert_eq!(patched.dirs, result.stats.dirs);
            })
            .unwrap();
    }

    #[test]
    fn a_path_the_tree_does_not_know_is_no_patch_at_all() {
        let (manager, fixture, data) = scanned_fixture();
        let generation = manager.lock().generation;
        let before = root_children(&manager);
        // A second spelling of `a/`, which names the same directory and is not what the
        // scan recorded. Built here rather than taken from `canonicalize`, whose answer
        // depends on whether this machine reaches its temporary directory through a
        // symlink: where it does not, the path would be a key of the tree after all and
        // this test would assert the opposite of what it means.
        let link = data.path().join("link");
        std::os::unix::fs::symlink(fixture.path(), &link).unwrap();
        let through_link = link.join("a");
        assert_ne!(through_link, fixture.path().join("a"));
        assert!(through_link.is_dir(), "it still names the same directory");

        assert_eq!(
            manager.patch_paths(&[
                fixture.path().join("never-existed"),
                PathBuf::from("/elsewhere"),
                through_link,
            ]),
            TreeState::Current,
            "no row of this tree names any of them"
        );

        assert_eq!(root_children(&manager), before);
        assert_eq!(
            manager.lock().generation,
            generation,
            "no splice, so no new ids"
        );
    }

    /// A rescan that fails says nothing about whether the path is there — a permission, a
    /// loop — so its branch stays as the scan left it. Dropping it would tell the user the
    /// directory is gone on the strength of an error that means the opposite.
    #[test]
    fn a_path_that_cannot_be_rescanned_keeps_its_branch() {
        if crate::running_as_root() {
            return;
        }
        let (manager, fixture, _data) = scanned_fixture();
        let generation = manager.lock().generation;
        let before = manager
            .with_result(|result, _| result.tree.root().size)
            .unwrap();
        // Nothing below `a` can even be stat'ed now, so the rescan of `a/one.bin` fails
        // rather than reporting it gone.
        let locked = fixture.path().join("a");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let state = manager.patch_paths(&[locked.join("one.bin")]);

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            state,
            TreeState::Stale,
            "the branch may name something that is gone, and only a scan can tell"
        );
        assert_eq!(root_children(&manager), vec!["a", "b.bin"]);
        assert!(
            manager
                .with_result(|result, _| result.tree.find(&locked.join("one.bin")).is_some())
                .unwrap(),
            "the branch is still there"
        );
        assert_eq!(
            manager
                .with_result(|result, _| result.tree.root().size)
                .unwrap(),
            before
        );
        assert_eq!(
            manager.lock().generation,
            generation,
            "an empty batch of patches is not a splice"
        );
    }

    /// The root is the one node a splice cannot replace, so a patch naming it is dropped
    /// before the rescan rather than after: after would mean walking the whole scan root —
    /// the 25-second rescan the patching exists to avoid — and then throwing the result away.
    #[test]
    fn patching_the_scan_root_is_not_a_patch() {
        let (manager, fixture, _data) = scanned_fixture();
        let generation = manager.lock().generation;
        let before = root_children(&manager);

        // The claim itself: nothing is left to rescan. A splice that merely discarded the
        // result afterwards would satisfy every assertion below it.
        assert!(
            manager
                .patch_targets(&[fixture.path().to_path_buf()])
                .is_none(),
            "the root is dropped before the walk, not after it"
        );
        assert_eq!(
            manager.patch_paths(&[fixture.path().to_path_buf()]),
            TreeState::Current,
            "and nothing was deleted that the tree is now wrong about"
        );

        assert_eq!(root_children(&manager), before);
        assert_eq!(manager.lock().generation, generation);
    }

    #[test]
    fn patching_without_a_result_does_nothing() {
        let data = tempfile::tempdir().unwrap();
        let manager = manager_in(data.path());
        assert_eq!(
            manager.patch_paths(&[PathBuf::from("/anything")]),
            TreeState::Current,
            "there is no tree to be wrong"
        );
        assert_eq!(manager.lock().generation, 0);
        assert!(manager.with_result(|_, _| ()).is_none());
    }

    /// The three steps are a contract: what is rescanned off the lock is spliced back only
    /// if the arena it was resolved against is still the one the manager holds. A scan that
    /// finished meanwhile has replaced every id in it.
    #[test]
    fn a_patch_resolved_against_an_older_arena_is_dropped() {
        let (manager, fixture, _data) = scanned_fixture();
        let (result, generation, ids) = manager
            .patch_targets(&[fixture.path().join("a")])
            .expect("the tree knows a/");
        assert_eq!(ids.len(), 1);
        let (patches, complete) = rescan_targets(&result, &ids);
        assert!(complete);
        assert_eq!(patches.len(), 1);

        // A rescan lands between the resolution and the splice.
        manager
            .start(Arc::new(Events::default()), fixture.path().to_path_buf())
            .unwrap();
        assert_eq!(wait_until_finished(&manager).state, ScanState::Done);
        let current = manager.lock().generation;
        assert_ne!(current, generation);

        assert!(!manager.install_patches(generation, patches));
        assert_eq!(
            manager.lock().generation,
            current,
            "a dropped patch changes nothing, the generation included"
        );
        assert_eq!(root_children(&manager), vec!["a", "b.bin"]);
    }

    /// The same patch, installed on the generation it was resolved against.
    #[test]
    fn a_patch_resolved_against_the_current_arena_is_installed() {
        let (manager, fixture, _data) = scanned_fixture();
        let (result, generation, ids) = manager
            .patch_targets(&[fixture.path().join("a")])
            .expect("the tree knows a/");
        fs::remove_dir_all(fixture.path().join("a")).unwrap();
        // Nothing here can touch the manager: `rescan_targets` takes the result, not the
        // manager, so the walk has no name for the lock. That is the step-2 contract, and
        // it is the signature that holds it, not an assertion — a lock held across this
        // call would hang the suite rather than fail it.
        let (patches, complete) = rescan_targets(&result, &ids);
        assert!(complete);
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].0, ids[0]);
        assert!(patches[0].1.is_none(), "a/ is gone, so it is dropped");

        assert!(manager.install_patches(generation, patches));
        assert_eq!(root_children(&manager), vec!["b.bin"]);
        assert_eq!(manager.lock().generation, generation + 1);
    }

    /// The two booleans the three steps produce, and the one answer the window gets. They
    /// run the other way round — `install_patches` returns `true` for the good case — so the
    /// table is written out rather than reasoned about.
    #[test]
    fn only_an_installed_and_complete_patch_leaves_the_tree_current() {
        assert_eq!(TreeState::of(true, true), TreeState::Current);
        assert_eq!(
            TreeState::of(false, true),
            TreeState::Stale,
            "the splice was dropped"
        );
        assert_eq!(
            TreeState::of(true, false),
            TreeState::Stale,
            "a path could not be rescanned"
        );
        assert_eq!(TreeState::of(false, false), TreeState::Stale);
    }

    #[test]
    fn panic_messages_are_extracted_from_the_payload() {
        let text: Box<dyn Any + Send> = Box::new("boom");
        assert_eq!(panic_message(&*text), "the scan panicked: boom");
        let string: Box<dyn Any + Send> = Box::new(String::from("bang"));
        assert_eq!(panic_message(&*string), "the scan panicked: bang");
        let other: Box<dyn Any + Send> = Box::new(42u8);
        assert_eq!(panic_message(&*other), "the scan panicked: unknown panic");
    }
}
