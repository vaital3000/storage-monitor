//! The cleanup modules of the window: at most one discovery per module at a time, each on a
//! thread of its own and observed through `modules:state` events, and the items of every
//! module's last discovery that succeeded, kept in memory for the Cleanup screen and for the
//! batches. Nothing here is persisted: a launch starts with every module idle.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use chrono::{DateTime, Utc};
use storage_monitor_core::cleanup::Held;
use storage_monitor_core::module::{Availability, Context, Item, Level, Module};
use storage_monitor_core::system::System;

use crate::scan_manager::panic_text;
use crate::views::{ModuleStatus, ModuleView};

pub const MODULES_STATE_EVENT: &str = "modules:state";

/// Where the manager's events go: the Tauri app in production, a recorder in tests.
///
/// Called while the manager's lock is held, so that the order of the events is the order of
/// the states: an implementation must never call back into [`ModuleManager`].
pub trait ModuleEmitter: Send + Sync + 'static {
    fn emit(&self, view: &ModuleView);
}

impl<R: tauri::Runtime> ModuleEmitter for tauri::AppHandle<R> {
    fn emit(&self, view: &ModuleView) {
        if let Err(err) = tauri::Emitter::emit(self, MODULES_STATE_EVENT, view) {
            eprintln!("cannot emit {MODULES_STATE_EVENT}: {err}");
        }
    }
}

/// The registry of the build, and where each module stands.
#[derive(Clone)]
pub struct ModuleManager {
    inner: Arc<Shared>,
}

struct Shared {
    registry: Vec<Box<dyn Module>>,
    sys: Arc<dyn System>,
    /// `None` only on a machine where the home folder cannot be determined: every discovery
    /// then fails with that sentence, and no batch runs — the guards of a cleanup are built
    /// from this folder, and there is no honest substitute for it.
    home: Option<PathBuf>,
    data_dir: PathBuf,
    /// One per module of `registry`, in the same order.
    slots: Mutex<Vec<Slot>>,
}

#[derive(Default)]
struct Slot {
    status: Status,
    /// The items of the last discovery that succeeded. Kept while a refresh runs and when one
    /// fails — a refresh that failed does not empty the screen, it says so — and dropped when
    /// the module becomes unavailable, since nothing could act on them then.
    items: Option<Arc<Vec<Item>>>,
    discovered_at: Option<DateTime<Utc>>,
    /// Bumped by every refresh, so that a discovery that finishes after a newer one started
    /// is dropped rather than installed over it.
    generation: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Status {
    #[default]
    Idle,
    Discovering,
    Ready,
    Unavailable(String),
    Failed(String),
}

/// What one discovery found.
enum Found {
    Items(Vec<Item>),
    Unavailable(String),
    Failed(String),
}

impl ModuleManager {
    pub fn new(
        registry: Vec<Box<dyn Module>>,
        sys: Arc<dyn System>,
        home: Option<PathBuf>,
        data_dir: PathBuf,
    ) -> Self {
        let slots = registry.iter().map(|_| Slot::default()).collect();
        Self {
            inner: Arc::new(Shared {
                registry,
                sys,
                home,
                data_dir,
                slots: Mutex::new(slots),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Slot>> {
        self.inner
            .slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The port every module and every batch of this manager goes through.
    pub fn system(&self) -> &dyn System {
        &*self.inner.sys
    }

    /// The home folder the guards of a cleanup batch are built from.
    pub fn home(&self) -> Option<&Path> {
        self.inner.home.as_deref()
    }

    /// Every module, in the registry's order.
    pub fn list(&self) -> Vec<ModuleView> {
        let slots = self.lock();
        self.inner
            .registry
            .iter()
            .zip(slots.iter())
            .map(|(module, slot)| view(module.as_ref(), slot))
            .collect()
    }

    /// Starts a discovery of every module named in `ids` — of every module when `ids` is
    /// empty — each on a thread of its own, and answers the views as they stand once all of
    /// them are `discovering`. An id no module has is ignored.
    ///
    /// A module already discovering is started again: its older discovery is dropped when it
    /// finishes, whatever it found.
    pub fn refresh(&self, emitter: Arc<dyn ModuleEmitter>, ids: &[String]) -> Vec<ModuleView> {
        let targets: Vec<usize> = self
            .inner
            .registry
            .iter()
            .enumerate()
            .filter(|(_, module)| {
                ids.is_empty() || ids.iter().any(|id| id == module.descriptor().id)
            })
            .map(|(index, _)| index)
            .collect();
        // Every target is marked under one lock, and the answer taken there too, before any
        // thread starts: a module that finds nothing answers in microseconds, and an answer
        // built afterwards would say `ready` or `discovering` depending on the scheduler.
        let (started, views) = {
            let mut slots = self.lock();
            let mut started = Vec::with_capacity(targets.len());
            for index in targets {
                let slot = &mut slots[index];
                slot.generation += 1;
                slot.status = Status::Discovering;
                emitter.emit(&view(self.inner.registry[index].as_ref(), slot));
                started.push((index, slot.generation));
            }
            let views: Vec<ModuleView> = self
                .inner
                .registry
                .iter()
                .zip(slots.iter())
                .map(|(module, slot)| view(module.as_ref(), slot))
                .collect();
            (started, views)
        };
        for (index, generation) in started {
            let worker = {
                let manager = self.clone();
                let emitter = Arc::clone(&emitter);
                move || manager.discover(index, generation, &*emitter)
            };
            let name = format!("discover-{}", self.inner.registry[index].descriptor().id);
            if let Err(err) = thread::Builder::new().name(name).spawn(worker) {
                self.finish(
                    index,
                    generation,
                    Found::Failed(format!("cannot start the discovery: {err}")),
                    &*emitter,
                );
            }
        }
        views
    }

    /// Body of a discovery thread. The module runs outside the lock; a panic in it ends the
    /// module `failed` rather than `discovering` for ever, the way the scan worker does.
    fn discover(&self, index: usize, generation: u64, emitter: &dyn ModuleEmitter) {
        let module = self.inner.registry[index].as_ref();
        let found = match &self.inner.home {
            None => Found::Failed("cannot determine the home folder".to_owned()),
            Some(home) => {
                let ctx = Context {
                    sys: &*self.inner.sys,
                    home,
                    data_dir: &self.inner.data_dir,
                };
                panic::catch_unwind(AssertUnwindSafe(|| match module.availability(&ctx) {
                    Availability::Unavailable { reason } => Found::Unavailable(reason),
                    Availability::Available => match module.discover(&ctx) {
                        Ok(items) => Found::Items(items),
                        Err(err) => Found::Failed(err.to_string()),
                    },
                }))
                .unwrap_or_else(|payload| Found::Failed(panicked(&*payload)))
            }
        };
        self.finish(index, generation, found, emitter);
    }

    /// Installs what a discovery found, unless a newer refresh has started since, and says so.
    fn finish(&self, index: usize, generation: u64, found: Found, emitter: &dyn ModuleEmitter) {
        let now = self.inner.sys.now();
        let mut slots = self.lock();
        let slot = &mut slots[index];
        if slot.generation != generation {
            return;
        }
        match found {
            Found::Items(items) => {
                slot.status = Status::Ready;
                slot.items = Some(Arc::new(items));
                slot.discovered_at = Some(now);
            }
            Found::Unavailable(reason) => {
                slot.status = Status::Unavailable(reason);
                slot.items = None;
                slot.discovered_at = None;
            }
            Found::Failed(message) => slot.status = Status::Failed(message),
        }
        emitter.emit(&view(self.inner.registry[index].as_ref(), slot));
    }

    /// Every held item, largest first, at most `limit` of them, and how many there are.
    pub fn items(&self, limit: usize) -> (Vec<Item>, usize) {
        let held: Vec<Arc<Vec<Item>>> = self
            .lock()
            .iter()
            .filter_map(|slot| slot.items.clone())
            .collect();
        let mut items: Vec<Item> = held
            .iter()
            .flat_map(|items| items.iter().cloned())
            .collect();
        let total = items.len();
        items.sort_by(|a, b| {
            b.size
                .bytes
                .cmp(&a.size.bytes)
                .then_with(|| a.title.cmp(&b.title))
        });
        items.truncate(limit);
        (items, total)
    }

    /// Runs `f` over the held items and the modules they came from — what the cleanup engine
    /// plans a batch against. The lock is held only to take the lists.
    pub fn with_held<T>(&self, f: impl FnOnce(&Held) -> T) -> T {
        let held: Vec<Option<Arc<Vec<Item>>>> =
            self.lock().iter().map(|slot| slot.items.clone()).collect();
        let pairs = self
            .inner
            .registry
            .iter()
            .zip(&held)
            .filter_map(|(module, items)| {
                items
                    .as_ref()
                    .map(|items| (module.as_ref(), items.as_slice()))
            });
        f(&Held::new(pairs))
    }

    /// Drops the items with these ids until the next discovery: what a batch removed is not
    /// on the screen any more, even before the rediscovery that follows it has finished.
    pub fn forget(&self, item_ids: &[String]) {
        if item_ids.is_empty() {
            return;
        }
        for slot in self.lock().iter_mut() {
            if let Some(items) = &slot.items
                && items.iter().any(|item| item_ids.contains(&item.id))
            {
                let kept: Vec<Item> = items
                    .iter()
                    .filter(|item| !item_ids.contains(&item.id))
                    .cloned()
                    .collect();
                slot.items = Some(Arc::new(kept));
            }
        }
    }

    /// The ids of the modules these items belong to, in the registry's order.
    pub fn modules_of(&self, item_ids: &[String]) -> Vec<String> {
        self.inner
            .registry
            .iter()
            .map(|module| module.descriptor().id)
            .filter(|id| {
                item_ids.iter().any(|item| {
                    item.split_once(':')
                        .is_some_and(|(module, _)| module == *id)
                })
            })
            .map(str::to_owned)
            .collect()
    }
}

/// A module as the UI sees it. The totals are over the items it holds, which is what the
/// screen lists — including while a refresh runs, and after one failed.
fn view(module: &dyn Module, slot: &Slot) -> ModuleView {
    let descriptor = module.descriptor();
    let items = slot.items.as_deref().map(Vec::as_slice).unwrap_or_default();
    let (status, reason) = match &slot.status {
        Status::Idle => (ModuleStatus::Idle, None),
        Status::Discovering => (ModuleStatus::Discovering, None),
        Status::Ready => (ModuleStatus::Ready, None),
        Status::Unavailable(reason) => (ModuleStatus::Unavailable, Some(reason.clone())),
        Status::Failed(message) => (ModuleStatus::Failed, Some(message.clone())),
    };
    ModuleView {
        id: descriptor.id.to_owned(),
        name: descriptor.name.to_owned(),
        description: descriptor.description.to_owned(),
        status,
        reason,
        item_count: items.len(),
        total_bytes: items.iter().map(|item| item.size.bytes).sum(),
        safe_bytes: items
            .iter()
            .filter(|item| item.verdict.level == Level::Safe)
            .map(|item| item.size.bytes)
            .sum(),
        discovered_at: slot.discovered_at,
    }
}

fn panicked(payload: &(dyn Any + Send)) -> String {
    format!("the module panicked: {}", panic_text(payload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::Duration;

    use storage_monitor_core::action::Mode;
    use storage_monitor_core::module::{
        ActionSpec, Descriptor, ModuleError, Reason, Size, Step, Verdict,
    };
    use storage_monitor_core::system::TestSystem;

    const WAIT: Duration = Duration::from_secs(5);

    /// Every view the manager emitted, in order, through a channel a test can wait on.
    struct Recorder(Mutex<Sender<ModuleView>>);

    impl ModuleEmitter for Recorder {
        fn emit(&self, view: &ModuleView) {
            let _ = self.0.lock().unwrap().send(view.clone());
        }
    }

    fn recorder() -> (Arc<dyn ModuleEmitter>, Receiver<ModuleView>) {
        let (tx, rx) = mpsc::channel();
        (Arc::new(Recorder(Mutex::new(tx))), rx)
    }

    fn next(rx: &Receiver<ModuleView>) -> ModuleView {
        rx.recv_timeout(WAIT).expect("an event")
    }

    fn item(module: &str, name: &str, bytes: u64, level: Level) -> Item {
        Item {
            id: format!("{module}:{name}"),
            module: module.to_owned(),
            kind: "thing".to_owned(),
            title: name.to_owned(),
            subtitle: None,
            path: None,
            size: Size::exact(bytes),
            last_used: None,
            verdict: Verdict::new(level, vec![Reason::new("because", "Because")]),
            facts: Vec::new(),
            actions: Vec::new(),
        }
    }

    /// A module a test drives: what it finds, whether it is there, whether it fails or
    /// panics, and — through `gate` — when its discovery may finish.
    struct Probe {
        id: &'static str,
        items: Mutex<Vec<Item>>,
        available: AtomicBool,
        failing: AtomicBool,
        panicking: AtomicBool,
        /// When set, a discovery waits for one message before it answers.
        gate: Mutex<Option<Receiver<()>>>,
        calls: AtomicUsize,
    }

    impl Probe {
        fn new(id: &'static str, items: Vec<Item>) -> Self {
            Self {
                id,
                items: Mutex::new(items),
                available: AtomicBool::new(true),
                failing: AtomicBool::new(false),
                panicking: AtomicBool::new(false),
                gate: Mutex::new(None),
                calls: AtomicUsize::new(0),
            }
        }
    }

    /// A registry entry that shares its [`Probe`] with the test driving it.
    struct Probed(Arc<Probe>);

    impl Module for Probed {
        fn descriptor(&self) -> Descriptor {
            Descriptor {
                id: self.0.id,
                name: "Probe",
                description: "A module a test drives",
                tools: &[],
            }
        }

        fn availability(&self, _ctx: &Context) -> Availability {
            if self.0.available.load(Ordering::SeqCst) {
                Availability::Available
            } else {
                Availability::Unavailable {
                    reason: "`probe` is not installed".to_owned(),
                }
            }
        }

        fn discover(&self, _ctx: &Context) -> Result<Vec<Item>, ModuleError> {
            let probe = &self.0;
            let call = probe.calls.fetch_add(1, Ordering::SeqCst);
            let gate = probe.gate.lock().unwrap().take();
            let items = probe.items.lock().unwrap().clone();
            if let Some(gate) = gate {
                // Only the first call waits; the answer is what the items were when it began.
                let _ = gate.recv_timeout(WAIT);
            }
            if probe.panicking.load(Ordering::SeqCst) {
                panic!("probe exploded on call {call}");
            }
            if probe.failing.load(Ordering::SeqCst) {
                return Err(ModuleError::Other("the daemon did not answer".to_owned()));
            }
            Ok(items)
        }

        fn plan(&self, _: &Item, _: &ActionSpec, _: &[String], _: Mode) -> Vec<Step> {
            Vec::new()
        }
    }

    fn manager(sys: &Arc<TestSystem>, probes: &[Arc<Probe>]) -> ModuleManager {
        let registry: Vec<Box<dyn Module>> = probes
            .iter()
            .map(|probe| Box::new(Probed(Arc::clone(probe))) as Box<dyn Module>)
            .collect();
        ModuleManager::new(
            registry,
            Arc::clone(sys) as Arc<dyn System>,
            Some(sys.root().to_path_buf()),
            sys.root().join("data"),
        )
    }

    #[test]
    fn a_module_is_idle_until_refreshed() {
        let sys = Arc::new(TestSystem::new());
        let probe = Arc::new(Probe::new(
            "probe",
            vec![item("probe", "a", 1, Level::Safe)],
        ));
        let manager = manager(&sys, &[Arc::clone(&probe)]);
        let views = manager.list();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].status, ModuleStatus::Idle);
        assert_eq!(views[0].item_count, 0);
        assert_eq!(probe.calls.load(Ordering::SeqCst), 0, "nothing ran");
    }

    #[test]
    fn refresh_discovers_and_emits_each_state() {
        let sys = Arc::new(TestSystem::new());
        let probe = Arc::new(Probe::new(
            "probe",
            vec![
                item("probe", "a", 10, Level::Safe),
                item("probe", "b", 5, Level::Review),
            ],
        ));
        let manager = manager(&sys, &[probe]);
        let (emitter, rx) = recorder();
        let answered = manager.refresh(emitter, &[]);
        assert_eq!(answered[0].status, ModuleStatus::Discovering);
        assert_eq!(next(&rx).status, ModuleStatus::Discovering);
        let ready = next(&rx);
        assert_eq!(ready.status, ModuleStatus::Ready);
        assert_eq!(ready.item_count, 2);
        assert_eq!(ready.total_bytes, 15);
        assert_eq!(ready.safe_bytes, 10);
        assert_eq!(ready.discovered_at, Some(sys.now()));
        assert_eq!(ready.reason, None);
    }

    #[test]
    fn an_unavailable_module_says_why_and_holds_no_items() {
        let sys = Arc::new(TestSystem::new());
        let probe = Arc::new(Probe::new(
            "probe",
            vec![item("probe", "a", 1, Level::Safe)],
        ));
        let manager = manager(&sys, &[Arc::clone(&probe)]);
        let (emitter, rx) = recorder();
        manager.refresh(Arc::clone(&emitter), &[]);
        next(&rx);
        assert_eq!(next(&rx).item_count, 1);

        probe.available.store(false, Ordering::SeqCst);
        manager.refresh(emitter, &[]);
        next(&rx);
        let gone = next(&rx);
        assert_eq!(gone.status, ModuleStatus::Unavailable);
        assert_eq!(gone.reason.as_deref(), Some("`probe` is not installed"));
        assert_eq!(gone.item_count, 0, "nothing could act on them");
        assert!(manager.items(10).0.is_empty());
    }

    #[test]
    fn a_module_that_fails_keeps_the_items_of_its_last_success() {
        let sys = Arc::new(TestSystem::new());
        let probe = Arc::new(Probe::new(
            "probe",
            vec![item("probe", "a", 1, Level::Safe)],
        ));
        let manager = manager(&sys, &[Arc::clone(&probe)]);
        let (emitter, rx) = recorder();
        manager.refresh(Arc::clone(&emitter), &[]);
        next(&rx);
        next(&rx);

        probe.failing.store(true, Ordering::SeqCst);
        manager.refresh(emitter, &[]);
        next(&rx);
        let failed = next(&rx);
        assert_eq!(failed.status, ModuleStatus::Failed);
        assert_eq!(failed.reason.as_deref(), Some("the daemon did not answer"));
        assert_eq!(
            failed.item_count, 1,
            "a failed refresh does not empty the screen"
        );
        assert_eq!(manager.items(10).0.len(), 1);
    }

    #[test]
    fn a_panicking_module_ends_failed() {
        let sys = Arc::new(TestSystem::new());
        let probe = Arc::new(Probe::new("probe", Vec::new()));
        probe.panicking.store(true, Ordering::SeqCst);
        let manager = manager(&sys, &[probe]);
        let (emitter, rx) = recorder();
        manager.refresh(emitter, &[]);
        next(&rx);
        let failed = next(&rx);
        assert_eq!(failed.status, ModuleStatus::Failed);
        let reason = failed.reason.unwrap();
        assert!(reason.starts_with("the module panicked: "), "{reason}");
        assert!(reason.contains("probe exploded"), "{reason}");
    }

    #[test]
    fn an_older_discovery_cannot_overwrite_a_newer_one() {
        let sys = Arc::new(TestSystem::new());
        let probe = Arc::new(Probe::new(
            "probe",
            vec![item("probe", "old", 1, Level::Safe)],
        ));
        let (release, gate) = mpsc::channel();
        *probe.gate.lock().unwrap() = Some(gate);
        let manager = manager(&sys, &[Arc::clone(&probe)]);
        let (emitter, rx) = recorder();

        // The first discovery starts and waits at the gate with "old".
        manager.refresh(Arc::clone(&emitter), &[]);
        assert_eq!(next(&rx).status, ModuleStatus::Discovering);
        while probe.calls.load(Ordering::SeqCst) == 0 {
            thread::sleep(Duration::from_millis(5));
        }
        // A second one starts, finds "new" and finishes first.
        *probe.items.lock().unwrap() = vec![item("probe", "new", 2, Level::Safe)];
        manager.refresh(Arc::clone(&emitter), &[]);
        assert_eq!(next(&rx).status, ModuleStatus::Discovering);
        assert_eq!(next(&rx).status, ModuleStatus::Ready);

        // The first one finishes last, and is dropped: no event, and "new" stays.
        release.send(()).unwrap();
        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "a stale discovery says nothing"
        );
        let titles: Vec<String> = manager
            .items(10)
            .0
            .into_iter()
            .map(|item| item.title)
            .collect();
        assert_eq!(titles, vec!["new"]);
    }

    #[test]
    fn refresh_starts_only_the_modules_it_names() {
        let sys = Arc::new(TestSystem::new());
        let first = Arc::new(Probe::new("first", Vec::new()));
        let second = Arc::new(Probe::new("second", Vec::new()));
        let manager = manager(&sys, &[Arc::clone(&first), Arc::clone(&second)]);
        let (emitter, rx) = recorder();
        let views = manager.refresh(emitter, &["second".to_owned(), "nobody".to_owned()]);
        assert_eq!(views[0].status, ModuleStatus::Idle);
        assert_eq!(views[1].status, ModuleStatus::Discovering);
        next(&rx);
        assert_eq!(next(&rx).id, "second");
        assert_eq!(first.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn items_are_largest_first_across_modules_and_capped() {
        let sys = Arc::new(TestSystem::new());
        let first = Arc::new(Probe::new(
            "first",
            vec![
                item("first", "small", 1, Level::Safe),
                item("first", "large", 30, Level::Safe),
            ],
        ));
        let second = Arc::new(Probe::new(
            "second",
            vec![item("second", "middle", 20, Level::Keep)],
        ));
        let manager = manager(&sys, &[first, second]);
        let (emitter, rx) = recorder();
        manager.refresh(emitter, &[]);
        for _ in 0..4 {
            next(&rx);
        }
        let (items, total) = manager.items(2);
        let titles: Vec<&str> = items.iter().map(|item| item.title.as_str()).collect();
        assert_eq!(titles, vec!["large", "middle"]);
        assert_eq!(total, 3);
    }

    #[test]
    fn held_items_come_with_their_module() {
        let sys = Arc::new(TestSystem::new());
        let probe = Arc::new(Probe::new(
            "probe",
            vec![item("probe", "a", 1, Level::Safe)],
        ));
        let manager = manager(&sys, &[probe]);
        let (emitter, rx) = recorder();
        manager.refresh(emitter, &[]);
        next(&rx);
        next(&rx);
        manager.with_held(|held| {
            let (module, item) = held.get("probe:a").expect("held");
            assert_eq!(module.descriptor().id, "probe");
            assert_eq!(item.title, "a");
            assert!(held.get("probe:b").is_none());
        });
    }

    #[test]
    fn forget_drops_items_until_the_next_discovery() {
        let sys = Arc::new(TestSystem::new());
        let probe = Arc::new(Probe::new(
            "probe",
            vec![
                item("probe", "a", 1, Level::Safe),
                item("probe", "b", 2, Level::Safe),
            ],
        ));
        let manager = manager(&sys, &[probe]);
        let (emitter, rx) = recorder();
        manager.refresh(Arc::clone(&emitter), &[]);
        next(&rx);
        next(&rx);
        manager.forget(&["probe:b".to_owned()]);
        assert_eq!(manager.items(10).0.len(), 1);
        assert_eq!(manager.list()[0].item_count, 1);
        manager.refresh(emitter, &[]);
        next(&rx);
        next(&rx);
        assert_eq!(
            manager.items(10).0.len(),
            2,
            "the next discovery finds what is there"
        );
    }

    #[test]
    fn modules_of_names_the_modules_of_some_items() {
        let sys = Arc::new(TestSystem::new());
        let first = Arc::new(Probe::new("first", Vec::new()));
        let second = Arc::new(Probe::new("second", Vec::new()));
        let manager = manager(&sys, &[first, second]);
        assert_eq!(
            manager.modules_of(&["second:x".to_owned(), "nobody:y".to_owned()]),
            vec!["second".to_owned()]
        );
    }

    #[test]
    fn without_a_home_folder_every_discovery_fails() {
        let sys = Arc::new(TestSystem::new());
        let probe = Arc::new(Probe::new("probe", Vec::new()));
        let manager = ModuleManager::new(
            vec![Box::new(Probed(Arc::clone(&probe)))],
            Arc::clone(&sys) as Arc<dyn System>,
            None,
            sys.root().to_path_buf(),
        );
        let (emitter, rx) = recorder();
        manager.refresh(emitter, &[]);
        next(&rx);
        let failed = next(&rx);
        assert_eq!(failed.status, ModuleStatus::Failed);
        assert_eq!(
            failed.reason.as_deref(),
            Some("cannot determine the home folder")
        );
        assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
    }
}
