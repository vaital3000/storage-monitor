//! The two stages of a cleanup batch. [`preview`] plans every request and checks it,
//! touching nothing; [`execute`] plans them all again and runs the steps.
//!
//! Every request comes back, in its place, whether or not it survived the checks — the
//! dialog lists what the user ticked next to what will happen to each row, exactly as the
//! Explorer's does.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::action::{
    BlockReason, EntryResult, EntryStatus, Inspected, Limits, Mode, Refused, delete, inspect,
};
use crate::module::{ActionSpec, Effect, Item, Level, Module, Step, Target, ending, stderr_tail};
use crate::scan::NodeKind;
use crate::system::{Invocation, System, SystemError};

use super::model::{
    CleanupEntry, CleanupEntryOutcome, CleanupOutcome, CleanupPreview, EffectView, Progress,
    Request, StepView,
};

/// How long one command of a batch may run. Removing is slower than looking — a worktree
/// of a large repository, an image of many layers — and ten minutes is long enough for
/// either while still ending a command that hangs.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(600);

/// The latest discovery results, by item id, each with the module it came from.
pub struct Held<'a> {
    items: HashMap<&'a str, (&'a dyn Module, &'a Item)>,
}

impl<'a> Held<'a> {
    pub fn new(results: impl IntoIterator<Item = (&'a dyn Module, &'a [Item])>) -> Self {
        let mut items = HashMap::new();
        for (module, found) in results {
            for item in found {
                items.insert(item.id.as_str(), (module, item));
            }
        }
        Self { items }
    }

    pub fn get(&self, id: &str) -> Option<(&'a dyn Module, &'a Item)> {
        self.items.get(id).copied()
    }
}

/// A request that passed the rules that need no disk: the item is held, it offers the
/// action and the options, and a Keep verdict is forced.
pub struct Screened<'a> {
    pub module: &'a dyn Module,
    pub item: &'a Item,
    pub action: &'a ActionSpec,
}

/// Rules 1 to 3 of the design (section 8), in order; the first that applies decides.
///
/// Public because it is disk-free: `cleanup-cases.json` calls it directly on both sides,
/// the way the guard cases call `drop_nested`.
pub fn screen<'a>(request: &Request, held: &Held<'a>) -> Result<Screened<'a>, BlockReason> {
    // 1. Not among the held results: rediscovered away, or never there.
    let (module, item) = held.get(&request.item).ok_or(BlockReason::Missing)?;
    // 2. The item is no longer what the screen showed: it does not offer the action, or
    //    the action has no such option.
    let action = item
        .action(&request.action)
        .ok_or(BlockReason::KindChanged)?;
    if request
        .options
        .iter()
        .any(|option| action.option(option).is_none())
    {
        return Err(BlockReason::KindChanged);
    }
    // 3. A Keep verdict is lifted by a `force` option and by nothing else — not by an
    //    ordinary option that happens to be on.
    let forced = request
        .options
        .iter()
        .any(|option| action.option(option).is_some_and(|spec| spec.force));
    if item.verdict.level == Level::Keep && !forced {
        return Err(BlockReason::Kept);
    }
    Ok(Screened {
        module,
        item,
        action,
    })
}

/// Whether the Trash can undo `steps` in `mode`: the mode is Trash, and every step is a
/// `Delete` or housekeeping. A command that destroys or removes something is never undone
/// by the Trash, whatever the batch's mode (ADR 0003).
pub fn reversible(steps: &[Step], mode: Mode) -> bool {
    mode == Mode::Trash
        && steps.iter().all(|step| {
            matches!(
                step,
                Step::Delete(_)
                    | Step::Run {
                        effect: Effect::Housekeeping,
                        ..
                    }
            )
        })
}

/// Checks a batch without touching anything. Every request keeps its place in the list.
pub fn preview(
    requests: &[Request],
    held: &Held,
    limits: &Limits,
    sys: &dyn System,
    mode: Mode,
) -> CleanupPreview {
    let planned = plan_all(requests, held, limits, sys, mode);
    let entries: Vec<CleanupEntry> = planned
        .iter()
        .map(|planned| CleanupEntry {
            item: planned.item_id.clone(),
            module: planned.module.clone(),
            title: planned.title.clone(),
            action: planned.action.clone(),
            steps: planned
                .steps
                .iter()
                .map(|step| view(step, mode, &planned.inspected))
                .collect(),
            size: planned.size,
            status: planned.status,
            reversible: reversible(&planned.steps, mode),
        })
        .collect();
    let total_bytes = entries
        .iter()
        .filter(|entry| entry.status == EntryStatus::Ready)
        .fold(0u64, |sum, entry| sum.saturating_add(entry.size));
    CleanupPreview {
        entries,
        total_bytes,
        mode,
    }
}

/// Plans every request again and runs the steps of the ready ones, entry by entry.
///
/// Takes the requests, not a preview: a preview carries commands, and re-checking a command
/// proves nothing about it. So the only steps that ever run are the ones a module planned in
/// this call.
///
/// A failure ends its entry and never the batch. An entry refused before its first step ran
/// is `Skipped` — nothing was touched; one refused or failed after a step ran is `Failed`,
/// because something may have changed.
pub fn execute(
    requests: &[Request],
    held: &Held,
    limits: &Limits,
    sys: &dyn System,
    mode: Mode,
    progress: &mut dyn FnMut(Progress),
) -> CleanupOutcome {
    // One instant for the whole batch, read before anything is touched, as in 2a.
    let at = sys.now();
    let planned = plan_all(requests, held, limits, sys, mode);
    let total = planned.len();
    let mut entries = Vec::with_capacity(total);
    let mut freed_bytes = 0u64;
    for (done, planned) in planned.iter().enumerate() {
        progress(Progress {
            done,
            total,
            current: Some(planned.title.clone()),
        });
        let ran = match planned.status {
            EntryStatus::Blocked(reason) => Ran::refused(reason),
            EntryStatus::Ready => run_steps(planned, mode, limits, sys),
        };
        if let EntryResult::Removed { bytes } = ran.result {
            freed_bytes = freed_bytes.saturating_add(bytes);
        }
        entries.push(CleanupEntryOutcome {
            item: planned.item_id.clone(),
            module: planned.module.clone(),
            title: planned.title.clone(),
            action: planned.action.clone(),
            path: planned.path.clone(),
            kind: planned.kind,
            targets: ran.targets,
            mode: if reversible(&planned.steps, mode) {
                mode
            } else {
                Mode::Permanent
            },
            commands: ran.commands,
            result: ran.result,
        });
    }
    progress(Progress {
        done: total,
        total,
        current: None,
    });
    CleanupOutcome {
        entries,
        freed_bytes,
        at,
        mode,
    }
}

/// One request, planned and checked.
struct Planned {
    item_id: String,
    module: String,
    title: String,
    action: String,
    path: Option<PathBuf>,
    /// The kind of the target whose path is the item's path.
    kind: Option<NodeKind>,
    steps: Vec<Step>,
    /// Each target's normalized path by the path the module wrote, for the ones the preview
    /// got to look at: what the dialog shows.
    inspected: HashMap<PathBuf, PathBuf>,
    /// The judged forms of the targets, for the nesting rule.
    judged: Vec<PathBuf>,
    size: u64,
    status: EntryStatus,
}

/// Rules 1 to 6 over the whole batch.
fn plan_all(
    requests: &[Request],
    held: &Held,
    limits: &Limits,
    sys: &dyn System,
    mode: Mode,
) -> Vec<Planned> {
    let mut planned: Vec<Planned> = requests
        .iter()
        .map(|request| plan_one(request, held, limits, sys, mode))
        .collect();
    // 6. Across the batch, only the entries that are still ready take part — an entry that
    //    will not run cannot swallow the one below it. The predicate is `drop_nested`'s: a
    //    strict ancestor always wins, and between two spellings of one target the earlier
    //    entry does. Two targets of one entry are never compared: a module that deletes a
    //    folder and then prunes inside it has planned one thing, not two.
    let ready: Vec<(usize, &[PathBuf])> = planned
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.status == EntryStatus::Ready)
        .map(|(index, entry)| (index, entry.judged.as_slice()))
        .collect();
    let nested: Vec<usize> = ready
        .iter()
        .filter(|(index, targets)| {
            targets.iter().any(|target| {
                ready.iter().any(|(other, others)| {
                    other != index
                        && others.iter().any(|outer| {
                            target.starts_with(outer) && (outer != target || other < index)
                        })
                })
            })
        })
        .map(|(index, _)| *index)
        .collect();
    for index in nested {
        planned[index].status = EntryStatus::Blocked(BlockReason::Nested);
    }
    planned
}

/// Rules 1 to 5 for one request.
fn plan_one(
    request: &Request,
    held: &Held,
    limits: &Limits,
    sys: &dyn System,
    mode: Mode,
) -> Planned {
    let known = held.get(&request.item).map(|(_, item)| item);
    let mut planned = Planned {
        item_id: request.item.clone(),
        module: known.map(|item| item.module.clone()).unwrap_or_default(),
        title: known.map_or_else(|| request.item.clone(), |item| item.title.clone()),
        action: known
            .and_then(|item| item.action(&request.action))
            .map_or_else(|| request.action.clone(), |action| action.label.clone()),
        path: known.and_then(|item| item.path.clone()),
        kind: None,
        steps: Vec::new(),
        inspected: HashMap::new(),
        judged: Vec::new(),
        size: known
            .and_then(|item| item.action(&request.action))
            .map_or(0, |action| action.estimated_free),
        status: EntryStatus::Ready,
    };
    let screened = match screen(request, held) {
        Ok(screened) => screened,
        Err(reason) => {
            planned.status = EntryStatus::Blocked(reason);
            return planned;
        }
    };
    // 4. The module plans. An empty plan would be reported as done having done nothing.
    planned.steps = screened
        .module
        .plan(screened.item, screened.action, &request.options, mode);
    if planned.steps.is_empty() {
        planned.status = EntryStatus::Blocked(BlockReason::Malformed);
        return planned;
    }
    planned.kind = targets(&planned.steps)
        .find(|(target, _)| Some(&target.path) == planned.path.as_ref())
        .map(|(target, _)| target.kind);
    // 5. Every target through the guards and the disk. The first refusal decides.
    let mut refusal = None;
    for (target, removes) in targets(&planned.steps) {
        match check_target(target, removes, limits, sys) {
            Ok(inspected) => {
                planned
                    .inspected
                    .insert(target.path.clone(), inspected.path);
                planned.judged.push(inspected.judged);
            }
            Err(reason) => {
                refusal = Some(reason);
                break;
            }
        }
    }
    if let Some(reason) = refusal {
        planned.status = EntryStatus::Blocked(reason);
    }
    planned
}

/// Every target the steps delete, with whether a command (rather than the port) deletes it.
fn targets(steps: &[Step]) -> impl Iterator<Item = (&Target, bool)> {
    steps.iter().filter_map(|step| match step {
        Step::Delete(target) => Some((target, false)),
        Step::Run {
            effect: Effect::Removes(target),
            ..
        } => Some((target, true)),
        Step::Run { .. } => None,
    })
}

/// One target through the guards, the disk and the kind the module saw.
///
/// A target a command removes has one rule more: it must be spelled the way the guards
/// resolve it. The port deletes the normalized path of a `Delete`, whatever the module
/// wrote; a command is handed the module's spelling and resolves it itself, later — through
/// a symlinked folder, say, that pointed somewhere else by then. A target that resolves to
/// another spelling is a bug in the module, and refused rather than run.
fn check_target(
    target: &Target,
    removes: bool,
    limits: &Limits,
    sys: &dyn System,
) -> Result<Inspected, BlockReason> {
    let inspected = inspect(&target.path, limits, sys).map_err(|Refused { reason, .. }| reason)?;
    if inspected.kind != target.kind {
        return Err(BlockReason::KindChanged);
    }
    if removes && inspected.path != target.path {
        return Err(BlockReason::Malformed);
    }
    Ok(inspected)
}

/// A step as the dialog shows it: the normalized path when the preview got to look at it,
/// and the module's own otherwise.
fn view(step: &Step, mode: Mode, inspected: &HashMap<PathBuf, PathBuf>) -> StepView {
    let shown = |target: &Target| {
        inspected
            .get(&target.path)
            .unwrap_or(&target.path)
            .to_string_lossy()
            .into_owned()
    };
    match step {
        Step::Delete(target) => match mode {
            Mode::Trash => StepView::Trash {
                path: shown(target),
            },
            Mode::Permanent => StepView::Delete {
                path: shown(target),
            },
        },
        Step::Run { command, effect } => StepView::Run {
            command: command.line(),
            effect: match effect {
                Effect::Housekeeping => EffectView::Housekeeping,
                Effect::Destroys => EffectView::Destroys,
                Effect::Removes(_) => EffectView::Removes,
            },
            path: match effect {
                Effect::Removes(target) => Some(shown(target)),
                _ => None,
            },
        },
    }
}

/// What running one entry produced.
struct Ran {
    result: EntryResult,
    targets: Vec<PathBuf>,
    commands: Vec<Vec<String>>,
}

impl Ran {
    fn refused(reason: BlockReason) -> Self {
        Self {
            result: EntryResult::Skipped { reason },
            targets: Vec::new(),
            commands: Vec::new(),
        }
    }
}

/// The steps of one ready entry, in order, stopping at the first that does not succeed.
fn run_steps(planned: &Planned, mode: Mode, limits: &Limits, sys: &dyn System) -> Ran {
    let total = planned.steps.len();
    let mut ran = Ran {
        result: EntryResult::Removed {
            bytes: planned.size,
        },
        targets: Vec::new(),
        commands: Vec::new(),
    };
    for (index, step) in planned.steps.iter().enumerate() {
        // Whether anything has been done to this entry yet: every earlier step succeeded,
        // since the first that does not ends the entry.
        let started = index > 0;
        let at = |what: String| {
            if total > 1 {
                format!("step {} of {total}, {what}", index + 1)
            } else {
                what
            }
        };
        let refuse = |reason: BlockReason| {
            if started {
                EntryResult::Failed {
                    message: at(refusal(reason).to_owned()),
                }
            } else {
                EntryResult::Skipped { reason }
            }
        };
        let outcome = match step {
            Step::Delete(target) => match check_target(target, false, limits, sys) {
                Err(reason) => Err(refuse(reason)),
                Ok(Inspected { path, .. }) => {
                    ran.targets.push(path.clone());
                    match delete(&path, mode, sys) {
                        Ok(()) => Ok(()),
                        // Gone in the syscall after the check: nothing was destroyed by
                        // this step, the same answer 2a gives to the same race.
                        Err(SystemError::Missing(_)) => Err(refuse(BlockReason::Missing)),
                        Err(err) => Err(EntryResult::Failed {
                            message: at(err.to_string()),
                        }),
                    }
                }
            },
            Step::Run { command, effect } => {
                let checked = match effect {
                    Effect::Removes(target) => match check_target(target, true, limits, sys) {
                        Ok(Inspected { path, .. }) => {
                            ran.targets.push(path);
                            Ok(())
                        }
                        Err(reason) => Err(refuse(reason)),
                    },
                    Effect::Housekeeping | Effect::Destroys => Ok(()),
                };
                checked.and_then(|()| run_command(command, sys, &mut ran.commands, at))
            }
        };
        if let Err(result) = outcome {
            ran.result = result;
            return ran;
        }
    }
    ran
}

/// Locates and runs one command of a step; exit 0 or the entry's failure.
fn run_command(
    command: &crate::module::Command,
    sys: &dyn System,
    commands: &mut Vec<Vec<String>>,
    at: impl Fn(String) -> String,
) -> Result<(), EntryResult> {
    let line = command.line();
    let failed = |what: String| EntryResult::Failed {
        message: at(format!("`{line}`: {what}")),
    };
    let Some(program) = sys.locate(&command.tool) else {
        return Err(failed(format!("`{}` is not installed", command.tool)));
    };
    commands.push(command.argv());
    let invocation = Invocation {
        program,
        args: command.args.clone(),
        cwd: command.cwd.clone(),
        env: command.env.clone(),
        timeout: STEP_TIMEOUT,
    };
    match sys.run(&invocation) {
        Ok(output) if output.success() => Ok(()),
        Ok(output) => {
            let said = stderr_tail(&output.stderr);
            let ended = ending(output.status);
            Err(failed(if said.is_empty() {
                ended
            } else {
                format!("{ended}: {said}")
            }))
        }
        Err(err) => Err(failed(err.to_string())),
    }
}

/// What a refusal means once a step has already run, as the end of a failure message.
fn refusal(reason: BlockReason) -> &'static str {
    match reason {
        BlockReason::Missing => "its target is no longer there",
        BlockReason::KindChanged => "its target is no longer what the preview saw",
        BlockReason::Unreadable => "its target cannot be read",
        BlockReason::OutsideRoots => "its target is outside the home folder",
        BlockReason::Denylisted => "its target is inside a folder this app never deletes from",
        BlockReason::Shielded => "its target is a folder that may not be deleted as a whole",
        BlockReason::IsRoot => "its target is the home folder or one above it",
        BlockReason::Malformed => "its target is not spelled the way it resolves",
        BlockReason::Nested => "another entry contains its target",
        BlockReason::Kept => "the item is marked keep",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::{
        ActionOption, Availability, Command, Context, Descriptor, ModuleError, Reason, Size,
        Verdict,
    };
    use crate::system::{Reply, TestSystem};
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::Path;

    /// A module whose items and plans a test writes out: the engine is what is under test,
    /// and a plan here is whatever the case needs it to be.
    #[derive(Default)]
    struct Scripted {
        items: Vec<Item>,
        plans: HashMap<(String, Mode), Vec<Step>>,
    }

    impl Scripted {
        fn with(mut self, item: Item, trash: Vec<Step>, permanent: Vec<Step>) -> Self {
            self.plans.insert((item.id.clone(), Mode::Trash), trash);
            self.plans
                .insert((item.id.clone(), Mode::Permanent), permanent);
            self.items.push(item);
            self
        }

        /// The same steps in both modes.
        fn both(self, item: Item, steps: Vec<Step>) -> Self {
            self.with(item, steps.clone(), steps)
        }
    }

    impl Module for Scripted {
        fn descriptor(&self) -> Descriptor {
            Descriptor {
                id: "test",
                name: "Test",
                description: "",
                tools: &[],
            }
        }

        fn availability(&self, _ctx: &Context) -> Availability {
            Availability::Available
        }

        fn discover(&self, _ctx: &Context) -> Result<Vec<Item>, ModuleError> {
            Ok(self.items.clone())
        }

        fn plan(
            &self,
            item: &Item,
            _action: &ActionSpec,
            _options: &[String],
            mode: Mode,
        ) -> Vec<Step> {
            self.plans
                .get(&(item.id.clone(), mode))
                .cloned()
                .unwrap_or_default()
        }
    }

    /// An item with one action, `go`, carrying a force option and an ordinary one.
    fn item(name: &str, level: Level, size: u64, path: Option<PathBuf>) -> Item {
        Item {
            id: format!("test:{name}"),
            module: "test".to_owned(),
            kind: "thing".to_owned(),
            title: name.to_owned(),
            subtitle: None,
            path,
            size: Size::exact(size),
            last_used: None,
            verdict: Verdict::new(level, vec![Reason::new("because", "Because")]),
            facts: Vec::new(),
            actions: vec![ActionSpec {
                id: "go".to_owned(),
                label: "Go".to_owned(),
                estimated_free: size,
                options: vec![
                    ActionOption {
                        id: "force".to_owned(),
                        label: "Force".to_owned(),
                        default: false,
                        force: true,
                    },
                    ActionOption {
                        id: "extra".to_owned(),
                        label: "Extra".to_owned(),
                        default: false,
                        force: false,
                    },
                ],
            }],
        }
    }

    fn request(name: &str) -> Request {
        Request {
            item: format!("test:{name}"),
            action: "go".to_owned(),
            options: Vec::new(),
        }
    }

    fn forced(name: &str) -> Request {
        Request {
            options: vec!["force".to_owned()],
            ..request(name)
        }
    }

    fn dir(sys: &TestSystem, name: &str) -> PathBuf {
        let path = sys.root().join(name);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("inside.bin"), b"x").unwrap();
        path
    }

    fn file(sys: &TestSystem, name: &str) -> PathBuf {
        let path = sys.root().join(name);
        fs::write(&path, b"x").unwrap();
        path
    }

    fn delete_step(path: &Path, kind: NodeKind) -> Step {
        Step::Delete(Target::new(path, kind))
    }

    fn run_step(tool: &str, args: &[&str], effect: Effect) -> Step {
        Step::Run {
            command: Command::new(tool).args(args.iter().copied()),
            effect,
        }
    }

    fn limits(sys: &TestSystem) -> Limits {
        Limits::for_home(sys.root().to_path_buf())
    }

    fn preview_of(
        sys: &TestSystem,
        module: &Scripted,
        requests: &[Request],
        mode: Mode,
    ) -> CleanupPreview {
        let held = Held::new([(module as &dyn Module, module.items.as_slice())]);
        preview(requests, &held, &limits(sys), sys, mode)
    }

    fn execute_of(
        sys: &TestSystem,
        module: &Scripted,
        requests: &[Request],
        mode: Mode,
    ) -> CleanupOutcome {
        let held = Held::new([(module as &dyn Module, module.items.as_slice())]);
        execute(requests, &held, &limits(sys), sys, mode, &mut |_| {})
    }

    fn status(preview: &CleanupPreview, index: usize) -> EntryStatus {
        preview.entries[index].status
    }

    fn blocked(reason: BlockReason) -> EntryStatus {
        EntryStatus::Blocked(reason)
    }

    // --- The preview -------------------------------------------------------------------

    #[test]
    fn a_ready_entry_lists_its_steps_and_its_size() {
        let sys = TestSystem::new();
        let folder = dir(&sys, "cache");
        let module = Scripted::default().with(
            item("cache", Level::Safe, 40, Some(folder.clone())),
            vec![
                delete_step(&folder, NodeKind::Dir),
                run_step("touch", &["/h/.done"], Effect::Housekeeping),
            ],
            vec![delete_step(&folder, NodeKind::Dir)],
        );
        let trash = preview_of(&sys, &module, &[request("cache")], Mode::Trash);
        let entry = &trash.entries[0];
        assert_eq!(entry.status, EntryStatus::Ready);
        assert_eq!(entry.title, "cache");
        assert_eq!(entry.action, "Go");
        assert_eq!(entry.size, 40);
        assert_eq!(
            entry.steps,
            vec![
                StepView::Trash {
                    path: folder.to_string_lossy().into_owned()
                },
                StepView::Run {
                    command: "touch /h/.done".to_owned(),
                    effect: EffectView::Housekeeping,
                    path: None,
                },
            ]
        );
        assert_eq!(trash.total_bytes, 40);
        let permanent = preview_of(&sys, &module, &[request("cache")], Mode::Permanent);
        assert_eq!(
            permanent.entries[0].steps,
            vec![StepView::Delete {
                path: folder.to_string_lossy().into_owned()
            }]
        );
    }

    #[test]
    fn an_unknown_item_is_missing() {
        let sys = TestSystem::new();
        let module = Scripted::default();
        let preview = preview_of(&sys, &module, &[request("gone")], Mode::Trash);
        assert_eq!(status(&preview, 0), blocked(BlockReason::Missing));
        // Nothing is known about it but what was asked.
        assert_eq!(preview.entries[0].title, "test:gone");
        assert_eq!(preview.entries[0].module, "");
        assert!(preview.entries[0].steps.is_empty());
    }

    #[test]
    fn an_action_the_item_does_not_offer_is_kind_changed() {
        let sys = TestSystem::new();
        let folder = dir(&sys, "a");
        let module = Scripted::default().both(
            item("a", Level::Safe, 1, Some(folder.clone())),
            vec![delete_step(&folder, NodeKind::Dir)],
        );
        let other = Request {
            action: "remove".to_owned(),
            ..request("a")
        };
        let preview = preview_of(&sys, &module, &[other], Mode::Trash);
        assert_eq!(status(&preview, 0), blocked(BlockReason::KindChanged));
        assert_eq!(preview.entries[0].action, "remove");
    }

    #[test]
    fn an_option_the_action_does_not_have_is_kind_changed() {
        let sys = TestSystem::new();
        let folder = dir(&sys, "a");
        let module = Scripted::default().both(
            item("a", Level::Safe, 1, Some(folder.clone())),
            vec![delete_step(&folder, NodeKind::Dir)],
        );
        let odd = Request {
            options: vec!["branch".to_owned()],
            ..request("a")
        };
        let preview = preview_of(&sys, &module, &[odd], Mode::Trash);
        assert_eq!(status(&preview, 0), blocked(BlockReason::KindChanged));
    }

    #[test]
    fn a_keep_item_is_kept_without_its_force_option() {
        let sys = TestSystem::new();
        let folder = dir(&sys, "k");
        let module = Scripted::default().both(
            item("k", Level::Keep, 1, Some(folder.clone())),
            vec![delete_step(&folder, NodeKind::Dir)],
        );
        let preview = preview_of(&sys, &module, &[request("k")], Mode::Trash);
        assert_eq!(status(&preview, 0), blocked(BlockReason::Kept));
        // An option that is on but does not force is not a force option.
        let extra = Request {
            options: vec!["extra".to_owned()],
            ..request("k")
        };
        let preview = preview_of(&sys, &module, &[extra], Mode::Trash);
        assert_eq!(status(&preview, 0), blocked(BlockReason::Kept));
        assert_eq!(preview.total_bytes, 0);
    }

    #[test]
    fn a_keep_item_is_ready_with_its_force_option() {
        let sys = TestSystem::new();
        let folder = dir(&sys, "k");
        let module = Scripted::default().both(
            item("k", Level::Keep, 5, Some(folder.clone())),
            vec![delete_step(&folder, NodeKind::Dir)],
        );
        let preview = preview_of(&sys, &module, &[forced("k")], Mode::Trash);
        assert_eq!(status(&preview, 0), EntryStatus::Ready);
        assert_eq!(preview.total_bytes, 5);
    }

    #[test]
    fn an_empty_plan_is_malformed() {
        let sys = TestSystem::new();
        let module = Scripted::default().both(item("a", Level::Safe, 1, None), Vec::new());
        let preview = preview_of(&sys, &module, &[request("a")], Mode::Trash);
        assert_eq!(status(&preview, 0), blocked(BlockReason::Malformed));
    }

    #[test]
    fn a_target_outside_the_home_folder_is_outside_roots() {
        let sys = TestSystem::new();
        let elsewhere = tempfile::tempdir().unwrap();
        let outside = elsewhere.path().canonicalize().unwrap().join("x");
        fs::write(&outside, b"x").unwrap();
        let module = Scripted::default()
            .both(
                item("deleted", Level::Safe, 1, None),
                vec![delete_step(&outside, NodeKind::File)],
            )
            .both(
                item("removed", Level::Safe, 1, None),
                vec![run_step(
                    "rm",
                    &[outside.to_str().unwrap()],
                    Effect::Removes(Target::new(&outside, NodeKind::File)),
                )],
            );
        let preview = preview_of(
            &sys,
            &module,
            &[request("deleted"), request("removed")],
            Mode::Permanent,
        );
        assert_eq!(status(&preview, 0), blocked(BlockReason::OutsideRoots));
        assert_eq!(status(&preview, 1), blocked(BlockReason::OutsideRoots));
    }

    #[test]
    fn a_target_that_is_gone_is_missing() {
        let sys = TestSystem::new();
        let gone = sys.root().join("gone");
        let module = Scripted::default().both(
            item("a", Level::Safe, 1, Some(gone.clone())),
            vec![delete_step(&gone, NodeKind::Dir)],
        );
        let preview = preview_of(&sys, &module, &[request("a")], Mode::Trash);
        assert_eq!(status(&preview, 0), blocked(BlockReason::Missing));
    }

    #[test]
    fn a_target_of_another_kind_is_kind_changed() {
        let sys = TestSystem::new();
        let path = file(&sys, "was-a-folder");
        let module = Scripted::default().both(
            item("a", Level::Safe, 1, Some(path.clone())),
            vec![delete_step(&path, NodeKind::Dir)],
        );
        let preview = preview_of(&sys, &module, &[request("a")], Mode::Trash);
        assert_eq!(status(&preview, 0), blocked(BlockReason::KindChanged));
    }

    #[test]
    fn a_removes_target_in_another_spelling_is_malformed() {
        let sys = TestSystem::new();
        let real = dir(&sys, "real");
        fs::write(real.join("x.object"), b"x").unwrap();
        symlink(&real, sys.root().join("link")).unwrap();
        let spelled = sys.root().join("link/x.object");
        let module = Scripted::default()
            .both(
                item("run", Level::Safe, 1, None),
                vec![run_step(
                    "rm",
                    &[spelled.to_str().unwrap()],
                    Effect::Removes(Target::new(&spelled, NodeKind::File)),
                )],
            )
            // The port deletes what the guards resolved, so a `Delete` in that spelling is
            // fine: the rule is about commands alone.
            .both(
                item("port", Level::Safe, 1, None),
                vec![delete_step(&spelled, NodeKind::File)],
            );
        let preview = preview_of(
            &sys,
            &module,
            &[request("run"), request("port")],
            Mode::Trash,
        );
        assert_eq!(status(&preview, 0), blocked(BlockReason::Malformed));
        assert_eq!(status(&preview, 1), EntryStatus::Ready);
    }

    #[test]
    fn a_target_inside_another_entrys_target_is_nested() {
        let sys = TestSystem::new();
        let outer = dir(&sys, "outer");
        let inner = dir(&sys, "outer/inner");
        let module = Scripted::default()
            .both(
                item("inner", Level::Safe, 1, None),
                vec![delete_step(&inner, NodeKind::Dir)],
            )
            .both(
                item("outer", Level::Safe, 2, None),
                vec![delete_step(&outer, NodeKind::Dir)],
            );
        let preview = preview_of(
            &sys,
            &module,
            &[request("inner"), request("outer")],
            Mode::Trash,
        );
        assert_eq!(status(&preview, 0), blocked(BlockReason::Nested));
        assert_eq!(status(&preview, 1), EntryStatus::Ready);
        assert_eq!(preview.total_bytes, 2);
    }

    #[test]
    fn two_targets_of_one_entry_do_not_block_it() {
        let sys = TestSystem::new();
        let outer = dir(&sys, "outer");
        let inner = outer.join("inside.bin");
        let module = Scripted::default().both(
            item("a", Level::Safe, 1, None),
            vec![
                run_step(
                    "rm",
                    &[inner.to_str().unwrap()],
                    Effect::Removes(Target::new(&inner, NodeKind::File)),
                ),
                delete_step(&outer, NodeKind::Dir),
            ],
        );
        let preview = preview_of(&sys, &module, &[request("a")], Mode::Trash);
        assert_eq!(status(&preview, 0), EntryStatus::Ready);
    }

    #[test]
    fn a_blocked_entry_cannot_swallow_the_one_below_it() {
        let sys = TestSystem::new();
        let outer = dir(&sys, "outer");
        let inner = dir(&sys, "outer/inner");
        let module = Scripted::default()
            .both(
                item("outer", Level::Keep, 2, None),
                vec![delete_step(&outer, NodeKind::Dir)],
            )
            .both(
                item("inner", Level::Safe, 1, None),
                vec![delete_step(&inner, NodeKind::Dir)],
            );
        let preview = preview_of(
            &sys,
            &module,
            &[request("outer"), request("inner")],
            Mode::Trash,
        );
        assert_eq!(status(&preview, 0), blocked(BlockReason::Kept));
        assert_eq!(status(&preview, 1), EntryStatus::Ready);
    }

    #[test]
    fn reversibility_follows_the_steps_and_the_mode() {
        let folder = Target::new("/h/a", NodeKind::Dir);
        let delete = Step::Delete(folder.clone());
        let housekeeping = run_step("touch", &["/h/x"], Effect::Housekeeping);
        let destroys = run_step("docker", &["image", "rm", "x"], Effect::Destroys);
        let removes = run_step("rm", &["/h/a"], Effect::Removes(folder));
        assert!(reversible(
            &[delete.clone(), housekeeping.clone()],
            Mode::Trash
        ));
        assert!(!reversible(
            &[delete.clone(), housekeeping],
            Mode::Permanent
        ));
        assert!(!reversible(
            &[delete.clone(), destroys.clone()],
            Mode::Trash
        ));
        assert!(!reversible(&[destroys], Mode::Trash));
        assert!(!reversible(&[removes], Mode::Trash));
    }

    #[test]
    fn an_entry_reports_whether_the_trash_can_undo_it() {
        let sys = TestSystem::new();
        let folder = dir(&sys, "folder");
        let object = file(&sys, "a.object");
        let module = Scripted::default()
            .both(
                item("folder", Level::Safe, 1, None),
                vec![delete_step(&folder, NodeKind::Dir)],
            )
            .both(
                item("object", Level::Safe, 1, None),
                vec![run_step(
                    "rm",
                    &[object.to_str().unwrap()],
                    Effect::Removes(Target::new(&object, NodeKind::File)),
                )],
            );
        let requests = [request("folder"), request("object")];
        let trash = preview_of(&sys, &module, &requests, Mode::Trash);
        assert!(trash.entries[0].reversible);
        assert!(!trash.entries[1].reversible);
        let permanent = preview_of(&sys, &module, &requests, Mode::Permanent);
        assert!(!permanent.entries[0].reversible);
    }

    #[test]
    fn the_first_rule_that_applies_decides() {
        let sys = TestSystem::new();
        let gone = sys.root().join("gone");
        // Keep without force, and a target that is not there: rule 3 comes first.
        let module = Scripted::default().both(
            item("k", Level::Keep, 1, None),
            vec![delete_step(&gone, NodeKind::Dir)],
        );
        let preview = preview_of(&sys, &module, &[request("k")], Mode::Trash);
        assert_eq!(status(&preview, 0), blocked(BlockReason::Kept));
    }

    #[test]
    fn the_preview_touches_nothing() {
        let sys = TestSystem::new();
        let folder = dir(&sys, "a");
        sys.install("touch");
        let module = Scripted::default().both(
            item("a", Level::Safe, 1, None),
            vec![
                delete_step(&folder, NodeKind::Dir),
                run_step("touch", &["/h/x"], Effect::Housekeeping),
            ],
        );
        preview_of(&sys, &module, &[request("a")], Mode::Permanent);
        assert!(folder.exists());
        assert!(sys.ran().is_empty());
    }

    // --- Execution ---------------------------------------------------------------------

    #[test]
    fn every_step_runs_in_order() {
        let sys = TestSystem::new();
        let folder = dir(&sys, "cache");
        sys.install("touch");
        let module = Scripted::default().both(
            item("cache", Level::Safe, 40, Some(folder.clone())),
            vec![
                delete_step(&folder, NodeKind::Dir),
                run_step("touch", &["/h/.done"], Effect::Housekeeping),
            ],
        );
        let moved = folder.clone();
        sys.script(
            "touch",
            &["/h/.done"],
            Reply::ok().then(move || assert!(!moved.exists(), "the folder went first")),
        );
        let outcome = execute_of(&sys, &module, &[request("cache")], Mode::Trash);
        let entry = &outcome.entries[0];
        assert_eq!(entry.result, EntryResult::Removed { bytes: 40 });
        assert!(sys.trash_dir().join("cache").is_dir(), "moved to the Trash");
        assert_eq!(
            entry.commands,
            vec![vec!["touch".to_owned(), "/h/.done".to_owned()]]
        );
        assert_eq!(entry.targets, vec![folder.clone()]);
        assert_eq!(entry.path, Some(folder));
        assert_eq!(entry.kind, Some(NodeKind::Dir));
        assert_eq!(entry.mode, Mode::Trash);
        assert_eq!(outcome.freed_bytes, 40);
    }

    #[test]
    fn a_failed_step_ends_its_entry_not_the_batch() {
        let sys = TestSystem::new();
        let first = dir(&sys, "first");
        let second = dir(&sys, "second");
        sys.fail_next(&first);
        let module = Scripted::default()
            .both(
                item("first", Level::Safe, 1, None),
                vec![delete_step(&first, NodeKind::Dir)],
            )
            .both(
                item("second", Level::Safe, 2, None),
                vec![delete_step(&second, NodeKind::Dir)],
            );
        let outcome = execute_of(
            &sys,
            &module,
            &[request("first"), request("second")],
            Mode::Permanent,
        );
        assert!(matches!(
            outcome.entries[0].result,
            EntryResult::Failed { .. }
        ));
        assert_eq!(outcome.entries[1].result, EntryResult::Removed { bytes: 2 });
        assert!(first.exists() && !second.exists());
        assert_eq!(outcome.freed_bytes, 2);
    }

    #[test]
    fn a_refusal_before_the_first_step_is_a_skip() {
        let sys = TestSystem::new();
        let folder = dir(&sys, "a");
        sys.install("touch");
        let module = Scripted::default().both(
            item("a", Level::Safe, 1, None),
            vec![
                delete_step(&folder, NodeKind::Dir),
                run_step("touch", &["/h/x"], Effect::Housekeeping),
            ],
        );
        // Previewed while it was there; gone before the batch reaches it.
        let preview = preview_of(&sys, &module, &[request("a")], Mode::Trash);
        assert_eq!(status(&preview, 0), EntryStatus::Ready);
        fs::remove_dir_all(&folder).unwrap();
        let outcome = execute_of(&sys, &module, &[request("a")], Mode::Trash);
        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::Missing
            }
        );
        assert!(sys.ran().is_empty(), "nothing after the refusal ran");
    }

    #[test]
    fn a_refusal_after_a_step_ran_is_a_failure() {
        let sys = TestSystem::new();
        let first = dir(&sys, "first");
        let second = dir(&sys, "second");
        let module = Scripted::default().both(
            item("a", Level::Safe, 1, None),
            vec![
                delete_step(&first, NodeKind::Dir),
                delete_step(&second, NodeKind::Dir),
            ],
        );
        // The second target goes between the plan and its step; the first step already ran.
        let doomed = second.clone();
        let sys = Meddle::new(&sys, first.clone(), move || {
            fs::remove_dir_all(&doomed).unwrap()
        });
        let outcome = {
            let held = Held::new([(&module as &dyn Module, module.items.as_slice())]);
            execute(
                &[request("a")],
                &held,
                &limits(sys.inner),
                &sys,
                Mode::Permanent,
                &mut |_| {},
            )
        };
        match &outcome.entries[0].result {
            EntryResult::Failed { message } => {
                assert_eq!(message, "step 2 of 2, its target is no longer there");
            }
            other => panic!("got {other:?}"),
        }
        assert_eq!(outcome.freed_bytes, 0, "a lower bound, as in 2a");
    }

    #[test]
    fn a_non_zero_exit_fails_the_entry_with_the_end_of_stderr() {
        let sys = TestSystem::new();
        sys.install("docker");
        let module = Scripted::default().both(
            item("img", Level::Safe, 7, None),
            vec![run_step(
                "docker",
                &["image", "rm", "abc"],
                Effect::Destroys,
            )],
        );
        sys.script(
            "docker",
            &["image", "rm", "abc"],
            Reply::exit(1).stderr("noise\nError: image is in use\n"),
        );
        let outcome = execute_of(&sys, &module, &[request("img")], Mode::Trash);
        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Failed {
                message: "`docker image rm abc`: exited with 1: noise / Error: image is in use"
                    .to_owned()
            }
        );
        assert_eq!(
            outcome.entries[0].commands,
            vec![vec![
                "docker".to_owned(),
                "image".to_owned(),
                "rm".to_owned(),
                "abc".to_owned()
            ]]
        );
    }

    #[test]
    fn a_missing_tool_fails_the_entry_and_runs_nothing() {
        let sys = TestSystem::new();
        let module = Scripted::default().both(
            item("img", Level::Safe, 7, None),
            vec![run_step(
                "docker",
                &["image", "rm", "abc"],
                Effect::Destroys,
            )],
        );
        let outcome = execute_of(&sys, &module, &[request("img")], Mode::Trash);
        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Failed {
                message: "`docker image rm abc`: `docker` is not installed".to_owned()
            }
        );
        assert!(outcome.entries[0].commands.is_empty());
        assert!(sys.ran().is_empty());
    }

    #[test]
    fn a_port_error_fails_the_entry() {
        let sys = TestSystem::new();
        sys.install("docker");
        sys.script("docker", &["image", "rm", "abc"], Reply::timeout());
        let module = Scripted::default().both(
            item("img", Level::Safe, 7, None),
            vec![run_step(
                "docker",
                &["image", "rm", "abc"],
                Effect::Destroys,
            )],
        );
        let outcome = execute_of(&sys, &module, &[request("img")], Mode::Trash);
        match &outcome.entries[0].result {
            EntryResult::Failed { message } => {
                assert!(message.starts_with("`docker image rm abc`: "), "{message}");
                assert!(message.contains("did not finish"), "{message}");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn a_removes_target_is_revalidated_before_the_command() {
        let sys = TestSystem::new();
        sys.install("rm");
        let object = file(&sys, "a.object");
        let module = Scripted::default().both(
            item("a", Level::Safe, 1, None),
            vec![run_step(
                "rm",
                &[object.to_str().unwrap()],
                Effect::Removes(Target::new(&object, NodeKind::File)),
            )],
        );
        let preview = preview_of(&sys, &module, &[request("a")], Mode::Trash);
        assert_eq!(status(&preview, 0), EntryStatus::Ready);
        // A folder where the module saw a file.
        fs::remove_file(&object).unwrap();
        fs::create_dir(&object).unwrap();
        let outcome = execute_of(&sys, &module, &[request("a")], Mode::Trash);
        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::KindChanged
            }
        );
        assert!(sys.ran().is_empty(), "the command never ran");
    }

    #[test]
    fn a_removes_entry_runs_its_command_and_reports_its_target() {
        let sys = TestSystem::new();
        sys.install("rm");
        let object = file(&sys, "a.object");
        let target = object.clone();
        sys.script(
            "rm",
            &[object.as_os_str()],
            Reply::ok().then(move || fs::remove_file(target).unwrap()),
        );
        let module = Scripted::default().both(
            item("a", Level::Safe, 3, Some(object.clone())),
            vec![run_step(
                "rm",
                &[object.to_str().unwrap()],
                Effect::Removes(Target::new(&object, NodeKind::File)),
            )],
        );
        let outcome = execute_of(&sys, &module, &[request("a")], Mode::Trash);
        let entry = &outcome.entries[0];
        assert_eq!(entry.result, EntryResult::Removed { bytes: 3 });
        assert!(!object.exists());
        assert_eq!(entry.targets, vec![object.clone()]);
        assert_eq!(entry.kind, Some(NodeKind::File));
        // Nothing about it went to the Trash, whatever the batch was asked.
        assert_eq!(entry.mode, Mode::Permanent);
        assert_eq!(outcome.mode, Mode::Trash);
    }

    #[test]
    fn blocked_entries_are_skipped_with_their_reason() {
        let sys = TestSystem::new();
        let folder = dir(&sys, "k");
        let module = Scripted::default().both(
            item("k", Level::Keep, 1, Some(folder.clone())),
            vec![delete_step(&folder, NodeKind::Dir)],
        );
        let outcome = execute_of(&sys, &module, &[request("k"), request("gone")], Mode::Trash);
        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::Kept
            }
        );
        assert_eq!(
            outcome.entries[1].result,
            EntryResult::Skipped {
                reason: BlockReason::Missing
            }
        );
        assert!(folder.exists());
        assert_eq!(outcome.entries[1].path, None);
    }

    #[test]
    fn execute_plans_again_from_the_requests() {
        let sys = TestSystem::new();
        let before = dir(&sys, "before");
        let after = dir(&sys, "after");
        let previewed = Scripted::default().both(
            item("a", Level::Safe, 1, None),
            vec![delete_step(&before, NodeKind::Dir)],
        );
        let current = Scripted::default().both(
            item("a", Level::Safe, 1, None),
            vec![delete_step(&after, NodeKind::Dir)],
        );
        preview_of(&sys, &previewed, &[request("a")], Mode::Permanent);
        execute_of(&sys, &current, &[request("a")], Mode::Permanent);
        assert!(before.exists(), "the previewed plan is not what runs");
        assert!(!after.exists(), "the plan of the moment is");
    }

    #[test]
    fn progress_is_reported_before_each_entry_and_at_the_end() {
        let sys = TestSystem::new();
        let a = dir(&sys, "a");
        let b = dir(&sys, "b");
        let module = Scripted::default()
            .both(
                item("a", Level::Safe, 1, None),
                vec![delete_step(&a, NodeKind::Dir)],
            )
            .both(
                item("b", Level::Safe, 1, None),
                vec![delete_step(&b, NodeKind::Dir)],
            );
        let held = Held::new([(&module as &dyn Module, module.items.as_slice())]);
        let mut seen = Vec::new();
        execute(
            &[request("a"), request("b")],
            &held,
            &limits(&sys),
            &sys,
            Mode::Trash,
            &mut |progress| seen.push(progress),
        );
        assert_eq!(
            seen,
            vec![
                Progress {
                    done: 0,
                    total: 2,
                    current: Some("a".to_owned())
                },
                Progress {
                    done: 1,
                    total: 2,
                    current: Some("b".to_owned())
                },
                Progress {
                    done: 2,
                    total: 2,
                    current: None
                },
            ]
        );
    }

    #[test]
    fn the_batch_is_dated_before_anything_is_touched() {
        let sys = TestSystem::new();
        let folder = dir(&sys, "a");
        let module = Scripted::default().both(
            item("a", Level::Safe, 1, None),
            vec![delete_step(&folder, NodeKind::Dir)],
        );
        let at = sys.now();
        let outcome = execute_of(&sys, &module, &[request("a")], Mode::Trash);
        assert_eq!(outcome.at, at);
    }

    // --- The shared cases ----------------------------------------------------------------

    #[derive(serde::Deserialize)]
    struct CleanupCases {
        items: Vec<CaseItem>,
        screen: Vec<ScreenCase>,
        reversible: Vec<ReversibleCase>,
    }

    #[derive(serde::Deserialize)]
    struct CaseItem {
        id: String,
        level: Level,
        options: Vec<CaseOption>,
    }

    #[derive(serde::Deserialize)]
    struct CaseOption {
        id: String,
        force: bool,
    }

    #[derive(serde::Deserialize)]
    struct ScreenCase {
        name: String,
        request: Request,
        expect: String,
    }

    #[derive(serde::Deserialize)]
    struct ReversibleCase {
        name: String,
        steps: Vec<String>,
        trash: bool,
        permanent: bool,
    }

    /// An item of the cases: one action, `go`, with the options the case lists.
    fn case_item(case: &CaseItem) -> Item {
        Item {
            id: case.id.clone(),
            module: "test".to_owned(),
            kind: "thing".to_owned(),
            title: case.id.clone(),
            subtitle: None,
            path: None,
            size: Size::exact(0),
            last_used: None,
            verdict: Verdict::new(case.level, Vec::new()),
            facts: Vec::new(),
            actions: vec![ActionSpec {
                id: "go".to_owned(),
                label: "Go".to_owned(),
                estimated_free: 0,
                options: case
                    .options
                    .iter()
                    .map(|option| ActionOption {
                        id: option.id.clone(),
                        label: option.id.clone(),
                        default: false,
                        force: option.force,
                    })
                    .collect(),
            }],
        }
    }

    /// A step of the kind a case names. The paths are never looked at: `reversible` reads the
    /// kinds of the steps and nothing else, which is why these cases need no disk.
    fn case_step(kind: &str) -> Step {
        let target = Target::new("/h/thing", NodeKind::Dir);
        match kind {
            "delete" => Step::Delete(target),
            "housekeeping" => run_step("git", &["worktree", "prune"], Effect::Housekeeping),
            "destroys" => run_step("docker", &["image", "rm", "x"], Effect::Destroys),
            "removes" => run_step("rm", &["/h/thing"], Effect::Removes(target)),
            other => panic!("unknown step kind {other} in the shared cases"),
        }
    }

    #[test]
    fn the_shared_cleanup_cases_hold() {
        let text = fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/cleanup-cases.json"
        ))
        .expect("the shared cleanup cases are next to this crate");
        let cases: CleanupCases = serde_json::from_str(&text).expect("the shared cases parse");
        assert!(!cases.screen.is_empty() && !cases.reversible.is_empty());

        let module = Scripted {
            items: cases.items.iter().map(case_item).collect(),
            ..Scripted::default()
        };
        let held = Held::new([(&module as &dyn Module, module.items.as_slice())]);
        for case in &cases.screen {
            let answer = match screen(&case.request, &held) {
                Ok(_) => "ok".to_owned(),
                Err(reason) => serde_json::to_value(reason)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned(),
            };
            assert_eq!(answer, case.expect, "{}", case.name);
        }
        for case in &cases.reversible {
            let steps: Vec<Step> = case.steps.iter().map(|kind| case_step(kind)).collect();
            assert_eq!(
                reversible(&steps, Mode::Trash),
                case.trash,
                "{} (trash)",
                case.name
            );
            assert_eq!(
                reversible(&steps, Mode::Permanent),
                case.permanent,
                "{} (permanent)",
                case.name
            );
        }
    }

    /// A [`TestSystem`] that runs `then` right after the port was asked to delete `when`:
    /// the only way to make something change between two steps of one entry.
    struct Meddle<'a, F: Fn() + Send + Sync> {
        inner: &'a TestSystem,
        when: PathBuf,
        then: F,
    }

    impl<'a, F: Fn() + Send + Sync> Meddle<'a, F> {
        fn new(inner: &'a TestSystem, when: PathBuf, then: F) -> Self {
            Self { inner, when, then }
        }
    }

    impl<F: Fn() + Send + Sync> System for Meddle<'_, F> {
        fn symlink_metadata(&self, path: &Path) -> Result<std::fs::Metadata, SystemError> {
            self.inner.symlink_metadata(path)
        }

        fn move_to_trash(&self, path: &Path) -> Result<(), SystemError> {
            let done = self.inner.move_to_trash(path);
            if path == self.when {
                (self.then)();
            }
            done
        }

        fn remove(&self, path: &Path) -> Result<(), SystemError> {
            let done = self.inner.remove(path);
            if path == self.when {
                (self.then)();
            }
            done
        }

        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            self.inner.now()
        }

        fn locate(&self, tool: &str) -> Option<PathBuf> {
            self.inner.locate(tool)
        }

        fn run(&self, invocation: &Invocation) -> Result<crate::system::Output, SystemError> {
            self.inner.run(invocation)
        }
    }
}
