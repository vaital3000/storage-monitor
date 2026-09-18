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
use storage_monitor_core::scan::{ScanOptions, ScanProgress, ScanResult, scan};
use storage_monitor_core::snapshot::{Delta, Snapshot, SnapshotStore, deltas, top_growers};

use crate::views::{ScanState, ScanStatus};

/// Files smaller than this are left out of snapshots.
const SNAPSHOT_FILE_THRESHOLD: u64 = 10 * 1024 * 1024;
/// Snapshots kept in the store, across all roots.
const SNAPSHOTS_KEPT: usize = 10;
/// Growers computed when a scan finishes; the `top_growers` command takes a prefix.
const GROWERS_KEPT: usize = 50;
/// Interval of the `scan:progress` events.
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
        }
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
    /// `scan:done` when it is over, whatever the outcome.
    pub fn start(
        &self,
        emitter: Arc<dyn StatusEmitter>,
        root: PathBuf,
    ) -> Result<ScanStatus, String> {
        let (progress, generation) = {
            let mut inner = self.lock();
            if inner.state == ScanState::Running {
                return Err("a scan is already running".to_owned());
            }
            let progress = Arc::new(ScanProgress::default());
            *inner = Inner {
                state: ScanState::Running,
                generation: inner.generation + 1,
                root: Some(root.clone()),
                progress: Arc::clone(&progress),
                started: Some(Instant::now()),
                ..Inner::default()
            };
            (progress, inner.generation)
        };
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

    /// The largest growers since the previous snapshot, at most `limit` of them.
    pub fn growers(&self, limit: usize) -> Vec<Delta> {
        self.lock().growers.iter().take(limit).cloned().collect()
    }

    /// Body of the worker thread: a panic anywhere in the scan or the store becomes a
    /// `Failed` state, so the manager can never be stuck in `Running`.
    fn run(&self, root: PathBuf, progress: &ScanProgress, emitter: &dyn StatusEmitter) {
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
            scan(&ScanOptions::new(root), progress)
                .map(|result| Finished::persist(result, &self.snapshots_dir))
        }));
        let status = {
            let mut inner = self.lock();
            match outcome {
                Ok(Ok(finished)) => inner.complete(finished),
                Ok(Err(err)) => inner.fail(err.to_string()),
                Err(payload) => inner.fail(panic_message(&*payload)),
            }
            inner.status()
        };
        emitter.emit(DONE_EVENT, &status);
    }

    /// Body of the progress thread: emits the status every [`PROGRESS_INTERVAL`] while
    /// the scan of `generation` runs.
    fn tick(&self, generation: u64, emitter: &dyn StatusEmitter) {
        loop {
            thread::sleep(PROGRESS_INTERVAL);
            let status = {
                let inner = self.lock();
                if inner.generation != generation || inner.state != ScanState::Running {
                    return;
                }
                inner.status()
            };
            emitter.emit(PROGRESS_EVENT, &status);
        }
    }
}

/// A scan that ran to its end, with what the snapshot store knew about its root.
struct Finished {
    result: ScanResult,
    previous: Option<Snapshot>,
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
                previous: None,
                growers: Vec::new(),
            };
        }
        let store = SnapshotStore::new(snapshots_dir.to_path_buf());
        let previous = store.latest_for(&result.root).unwrap_or_else(|err| {
            eprintln!("cannot load the previous snapshot: {err}");
            None
        });
        let current = Snapshot::from_result(&result, SNAPSHOT_FILE_THRESHOLD);
        if let Err(err) = store.save(&current) {
            eprintln!("cannot save the snapshot: {err}");
        }
        if let Err(err) = store.prune(SNAPSHOTS_KEPT) {
            eprintln!("cannot prune the snapshots: {err}");
        }
        let growers = previous
            .as_ref()
            .map(|p| top_growers(&deltas(p, &current), GROWERS_KEPT))
            .unwrap_or_default();
        Self {
            result,
            previous,
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
        self.previous_taken_at = finished.previous.as_ref().map(|p| p.taken_at);
        self.previous_sizes = finished.previous.map(|p| Arc::new(p.size_index()));
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

fn panic_message(payload: &(dyn Any + Send)) -> String {
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned());
    format!("the scan panicked: {message}")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;

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
        assert!(finished.previous.is_none());
        assert!(finished.growers.is_empty());
        assert!(!data.path().join("snapshots").exists());
        let mut inner = Inner::default();
        inner.complete(finished);
        assert_eq!(inner.state, ScanState::Cancelled);
        assert_eq!(inner.status().files, 0);
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
