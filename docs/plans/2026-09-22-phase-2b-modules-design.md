# Phase 2b: cleanup modules

- Date: 2026-09-22
- Status: Accepted
- Scope: process execution in the `System` port, the module contract, the cleanup
  engine, a demo module, the Cleanup screen, `storage-monitor modules`

## 1. Problem

Phase 2a made the app delete, but only what the user picks by hand in the Explorer,
and only by path. The app still cannot say what the bytes are. The modules of phases
3 to 5 — git worktrees, Docker, Xcode — need three things that do not exist yet:

- a contract a module implements: find its items, judge them, say how to remove them;
- a way to run `git`, `docker` and `xcrun` through the `System` port, so that a
  module's tests run on a Linux runner against recorded outputs;
- a screen that lists what every module found, and a batch that removes the chosen
  items through the same guards, dialog and record as a deletion in the Explorer.

## 2. Goal

The roadmap's phase 2b — module framework, process execution, the Cleanup and
Settings screens — is split once more, the way phase 2 was:

- **2b (this design)**: process execution in the port, the module contract and its
  registry, the cleanup engine, a demo module, the Cleanup screen, and
  `storage-monitor modules list|run`.
- **2c**: Settings — roots and a folder picker, exclusions, the default deletion mode,
  per-module settings.
- **Phase 3**, where the first real module gives them a consumer: the declarative
  `Presentation` and the module pages, URL links, `storage-monitor clean`.

The exit criterion is the roadmap's: the demo module is cleaned end to end in both
modes — in a debug build of the app, and in Playwright over the mock.

## 3. Decisions

1. **Modules are synchronous** (ADR 0008). Discovery runs on a thread per module.
2. **Modules describe, the core acts** (ADR 0008). A module never deletes and never
   runs a command that changes anything: it answers with steps, and the core executes
   them through the guards.
3. **Planning is pure.** `plan` takes no `System`, so building a preview cannot touch
   anything, by construction.
4. **The port locates and runs programs**: argv only, an absolute program path, stdin
   closed, a timeout, an output cap.
5. **The demo module exists in debug builds only.** A release build of 2b has an empty
   registry, and its Cleanup section stays "soon" until phase 3.
6. **A module may delete inside the home folder only**, until 2c adds roots.
7. **Discovery runs on demand**, and its results live in memory only.
8. **One confirmation dialog.** `ConfirmDeleteDialog` is generalized from a path
   preview to a batch question, and the Explorer and Cleanup each adapt theirs into it.
9. **A cleanup preview answers for both modes at once**, because a module's steps
   depend on the mode.
10. **Cleanup batches report progress** through events.
11. **The action log gains optional fields**; the lines of 2a keep their exact shape.

## 4. The port runs programs

```rust
pub trait System: Send + Sync {
    // … the four methods of phase 2a …
    /// Where `tool` is installed: the absolute entries of `PATH`, then the known folders.
    fn locate(&self, tool: &str) -> Option<PathBuf>;
    /// Runs to the end or to the timeout. A non-zero exit is an `Output`, not an error.
    fn run(&self, invocation: &Invocation) -> Result<Output, SystemError>;
}

pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(OsString, OsString)>,
    pub timeout: Duration,
}

pub struct Output { pub status: Option<i32>, pub stdout: Vec<u8>, pub stderr: Vec<u8> }
```

**No shell.** The program and its arguments reach `execve` as they are. The program
must be absolute and in normal form, which is the rule the port already holds every
other path to: `run` refuses a bare `rm` as `Rejected` before anything starts, so no
name is ever resolved against whatever the working directory happens to be.

**Locating.** A Mac app started from Finder inherits launchd's `PATH`
(`/usr/bin:/bin:/usr/sbin:/sbin`), which is not where Homebrew puts `gh`, nor where
Docker Desktop and OrbStack put `docker`. `locate` walks the absolute entries of the
process's `PATH` — a relative entry would resolve against the working directory — and
then `/opt/homebrew/bin` and `/usr/local/bin`, and answers the first executable regular
file. A tool that lives somewhere else is a module's business, and the list grows when
a module needs it to.

One hazard is recorded here for phase 3 rather than solved: `/usr/bin/git` and
`/usr/bin/xcrun` exist on every Mac, but on one without the Command Line Tools they are
shims that open an installation dialog when run. `locate` finds them either way; the
git module has to ask `xcode-select -p` before it runs one.

**The timeout.** The standard library has no `wait_timeout`. The child runs in a
process group of its own, the port polls `try_wait`, and at the deadline it kills the
whole group rather than the child alone — a tool that forked leaves the grandchild
holding the pipes, and the readers would wait for it forever. Two threads read stdout
and stderr, so that neither pipe fills up and stalls the child, and the same deadline
covers them.

**The cap.** 32 MiB per stream. Past it the port keeps draining, since a child blocked
on a full pipe never exits, and answers `SystemError::OutputTooLarge`: a JSON document
cut in half parses as garbage, and garbage is worse than an error. `Output::status` is
`None` when a signal ended the process. What a non-zero exit means for an entry is the
engine's business (section 8).

**`TestSystem` runs nothing.** A test tells it which tools exist and what each
invocation answers:

```rust
sys.install("rm");                                  // `locate("rm")` finds <base>/bin/rm
sys.script("rm", &["/…/old.object"], Reply::ok());  // the next such run answers this
sys.ran();                                          // every invocation, oldest first
```

A run nobody scripted panics with its argv, for the reason the confinement panic of 2a
gives: a test of a module whose plan went wrong must stop, not quietly run
`docker image rm` on the developer's machine. Scripts match on the program's file name
and the arguments, and a reply is used once. A reply can also carry an effect on the
temporary tree — a scripted `rm` that really removes the file — for the end-to-end tests
of the desktop, where the next discovery has to find the object gone. A reply loads from
a fixture file as well, `{ "tool", "args", "status", "stdout", "stderr" }`; recording
those from a real machine arrives with the first module that has a tool worth recording.

## 5. The module contract

```rust
pub trait Module: Send + Sync {
    fn descriptor(&self) -> Descriptor;
    fn availability(&self, ctx: &Context) -> Availability;
    fn discover(&self, ctx: &Context) -> Result<Vec<Item>, ModuleError>;
    fn plan(&self, item: &Item, action: &ActionSpec, options: &[String], mode: Mode) -> Vec<Step>;
}

pub struct Descriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// What `availability` looks for; listed by `storage-monitor modules list`.
    pub tools: &'static [&'static str],
}

pub enum Availability { Available, Unavailable { reason: String } }

pub struct Context<'a> { pub sys: &'a dyn System, pub home: &'a Path, pub data_dir: &'a Path }
```

`Context::run(&Command)` is the one way a module runs anything during discovery: it
locates the tool, runs it with the discovery timeout (60 s), and turns a missing tool
into an error that names it.

Against design section 7:

- **Synchronous.** The core is synchronous throughout — the walker runs on rayon, the
  engine is a loop, the desktop hands both to `spawn_blocking` — and the CLI has no
  runtime at all. An `async fn` in a trait object needs boxing or a macro crate, and an
  executor for the CLI, and buys nothing a thread does not: a module's discovery is a few
  commands in sequence, and modules run in parallel with each other on threads of their
  own.
- **`execute` is gone; `plan` replaces it** (section 7).
- **`presentation` waits for phase 3**, where worktrees grouped by repository and the
  four sections of Docker give the schema something real to describe. Designed against a
  demo it would be designed in the dark. Until then one generic renderer draws every item
  (section 11).
- **`DiscoverContext` is `Context`, and loses two fields for now.** Module settings
  arrive with 2c, and the reference to the last scan tree with its first consumer, the
  sizes of DerivedData in phase 5. A field added to a struct the core builds breaks no
  module.

`home` and `data_dir` are in the context rather than read from `paths::`, so that a test
hands a temporary tree to both. A module reads the filesystem with `std::fs` — the port
exists for deleting, running and the clock — but only under paths it got from the
context or from a tool.

**The registry** is the crate `storage-monitor-modules` (`crates/modules/registry`) and
one function, `registry() -> Vec<Box<dyn Module>>`, which the CLI and the desktop both
call. Adding a module is a crate under `crates/modules/<id>` and one line there. In 2b
the list holds the demo module behind `cfg!(debug_assertions)`.

**The rule is enforced, not asked for.** `crates/modules/clippy.toml` disallows
`std::process::Command::new`, `std::fs::remove_file`, `remove_dir`, `remove_dir_all` and
`rename`. Clippy reads the nearest `clippy.toml` walking up from a crate's manifest, so
the file covers every crate under `crates/modules/` and nothing else — measured: the same
call in a sibling crate outside the folder stays legal. `RealSystem`, the one place those
calls are meant to live, is not under that folder.

## 6. Items

```rust
pub struct Item {
    pub id: String,                  // "<module>:<native id>", stable across discoveries
    pub module: String,
    pub kind: String,                // module-defined: "folder", "object", "worktree", "image"
    pub title: String,
    pub subtitle: Option<String>,
    pub path: Option<PathBuf>,
    pub size: Size,                  // { bytes, estimated }
    pub last_used: Option<DateTime<Utc>>,
    pub verdict: Verdict,            // { level: Safe | Review | Keep, reasons: Vec<Reason> }
    pub facts: Vec<Fact>,            // { key, label, value: FactValue }
    pub actions: Vec<ActionSpec>,    // { id, label, estimated_free, options }
}

pub struct ActionOption { pub id: String, pub label: String, pub default: bool, pub force: bool }
pub struct Reason { pub code: String, pub text: String }
pub enum FactValue { Text(String), Bytes(u64), Count(u64), Date(DateTime<Utc>), Flag(bool), Path(PathBuf) }
```

Against design section 7.1:

- **`ActionSpec` keeps `id`, `label`, `estimated_free` and `options`.** `danger`,
  `supports_trash`, `reversible` and `preview` are no longer fields: all four are
  properties of the steps an action plans in a given mode, and the core computes them
  from those (section 7), where a module cannot get them wrong.
- **`links` waits for phase 3**, together with opening a URL, which is a capability
  grant of its own. "Reveal in Finder" needs no link: every item with a path offers it.
- **`created_at` becomes a fact**, and `last_used_at` is `last_used`: the table has one
  generic date column, and it is that one.
- **A reason has a `code`** beside its sentence: tests assert on codes, the screen shows
  sentences.
- **`size.estimated`** is the design's `Confidence`. A size the module could not measure
  exactly — the shared layers of a Docker image — is drawn with a `~`.

**A Keep verdict carries its own gate.** The core refuses a Keep item, with a new block
reason `kept`, unless the request turns on one of the action's options marked `force`.
An action with no force option cannot remove a Keep item at all. That is what "do not
offer by default; still deletable with an explicit force option where it makes sense"
becomes once it is enforced rather than hoped for.

`estimated_free` is what the dialog promises and what the record says was freed. The
property `Preview::total_bytes` asks of any source of sizes still holds for a module: two
items of one batch must not count the same blocks.

## 7. Steps: modules describe, the core acts

```rust
pub enum Step {
    /// Moves the path to the Trash or deletes it, whichever the batch's mode says.
    Delete(Target),
    /// Runs a program, the same way in both modes.
    Run { command: Command, effect: Effect },
}

pub struct Target { pub path: PathBuf, pub kind: NodeKind }

pub struct Command {
    pub tool: String,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(OsString, OsString)>,
}

pub enum Effect {
    /// Bookkeeping nobody could miss: `git worktree prune` after the folder is gone.
    Housekeeping,
    /// Destroys something outside the filesystem the guards see: an image, a simulator.
    Destroys,
    /// Deletes this path, by other means than the port.
    Removes(Target),
}
```

The v1 modules, to check that the shape holds:

- a DerivedData folder: `[Delete(folder)]` in both modes;
- a Docker image: `[Run(docker image rm <id>, Destroys)]` in both modes;
- a worktree: `[Delete(worktree), Run(git worktree prune, Housekeeping)]` in Trash
  mode, `[Run(git worktree remove --force <path>, Removes(worktree))]` in Permanent
  mode, and the option "also delete the local branch" appends
  `Run(git branch -D <branch>, Destroys)`;
- the module-level actions (`docker image prune`, `simctl delete unavailable`) are items
  too — an item that stands for "dangling images", with one action — so the model needs
  no second kind of action.

What the shape buys:

- **The guards see every path.** A `Delete` target and a `Removes` target are judged by
  the same `Limits::check` as a row of the Explorer, in the preview and again right
  before the step. A module cannot reach a path the guards refuse, because the only thing
  it can do with a path is hand it back.
- **Reversibility is computed.** An entry is reversible in a mode when that mode is Trash
  and its steps are `Delete` and `Housekeeping` only. The dialog asks for its
  acknowledgement whenever a ready entry is irreversible in the chosen mode: the rule of
  2a for Permanent, now per entry. A Docker image asks for it in Trash mode too, because
  nothing about it goes to the Trash (ADR 0003).
- **The preview is exact.** It lists the steps themselves — the paths and the command
  lines — which is what design section 9 promised: "the exact commands or paths per
  entry".
- **`plan` is a plain function** of the item, the action, the options and the mode. It
  cannot see the disk or the clock, and a module's plans are unit-tested as such.

The tool is located when its step runs, not when it is planned: a `Command` names a tool,
the core runs whatever `locate` answers at that moment, and a tool that went away fails
its entry with a sentence that names it. The preview shows the tool by name.

A command line is shown, and recorded, as text: the argv joined with quoting, so that a
path with a space in it reads as one argument. The quoting is for reading only. Nothing
ever parses it back, and the process receives the argv.

## 8. The cleanup engine

```rust
pub struct Request { pub item: String, pub action: String, pub options: Vec<String> }

pub fn preview(requests: &[Request], held: &Held, limits: &Limits, sys: &dyn System, mode: Mode)
    -> CleanupPreview;
pub fn execute(requests: &[Request], held: &Held, limits: &Limits, sys: &dyn System, mode: Mode,
               progress: &mut dyn FnMut(Progress)) -> CleanupOutcome;
```

`Held` is the latest discovery results, by item id, each with the module it came from.

`execute` takes the requests, not a preview, and plans again before it runs anything. In
2a a `Preview` was re-checked because it has public fields and could be forged; here a
preview carries commands, and re-checking a command proves nothing about it. So there is
no path from a preview to a process at all: the only steps that ever run are the ones a
module planned in the same call.

**The preview**, per request, in order; the first rule that applies decides:

1. The item is not among the held results: `missing`.
2. The item does not offer the action, or the action has no such option: `kindChanged` —
   the item is no longer what the screen showed.
3. A Keep item without a `force` option turned on: `kept`.
4. The module plans the steps. An empty plan is a bug in the module and blocks the entry
   as `malformed`.
5. Every `Delete` and `Removes` target passes `Limits::check`, and its kind is the kind
   on the disk — every reason of 2a, unchanged.
6. Across the batch, an entry with a target inside another ready entry's target is
   `nested`: `drop_nested` over the judged forms, as in 2a.

**Execution**, per ready entry, step by step. A target is judged and stat'd again right
before its step — the re-validation of 2a, with the same `kindChanged` when the disk no
longer agrees. A `Run` locates its tool and runs it; exit 0 goes on to the next step, and
anything else ends the entry. The entry's result:

- `Removed { bytes: estimated_free }` when every step succeeded;
- `Skipped { reason }` when it was refused before its first step ran. Nothing was
  touched, which includes the race of 2a: a target that vanished just before (`missing`);
- `Failed { message }` when a step failed, or was refused after an earlier one had run.
  Something may have changed, so it is not a skip. The message names the step —
  "step 2 of 2, `touch …/.last-cleanup`: exited with 1: …" — with the end of stderr,
  capped. Its bytes are 0, the lower bound of 2a.

Entries are independent: a failure ends its entry, never the batch. Design section 9's
"unless the user chose so" waits with the rest of batch control; a Stop button needs a
module whose batches run for minutes, which phase 4 is.

`progress` is called before every entry and once at the end, with how many are done, how
many there are, and the title of the one starting.

Each entry reports the mode it really left in: the batch's mode when the entry is
reversible, and `permanent` otherwise. A line of the log that said "removed, Trash"
about a Docker image would promise a recovery that does not exist.

**The record.** Every entry is one line of `actions.jsonl`, and `LogEntry` gains two
fields, each `#[serde(default)]` and skipped when empty, as its own documentation
requires:

- `source`: `{ module, item, title, action }` — the ids of the module and the item, the
  item's title and the action's label, so that the line reads on its own long after the
  module has changed;
- `commands`: the argv of every `Run` that started, in order.

`path` and `kind` become optional (`#[serde(default)]`, skipped when absent). A line of
2a keeps both and its exact bytes; a cleanup line carries the item's path and the kind of
its target, and a request whose item was already gone has neither to give — it is
recorded anyway, because a row the user asked for and did not get is what the Activity
screen is for.

**The limits.** `Limits::for_home(home)`: the home folder as the only root, with the
denylist and the shield of ADR 0007 — `for_scan_root(home)`, with the home the context
carries rather than the one `paths::` reports, so a test can pass a temporary one. 2c
replaces the single root with the roots of Settings.

## 9. The demo module

`crates/modules/demo`, id `demo`, registered in debug builds only. Its world is a sandbox
it seeds on its first discovery, `<data dir>/demo`, and never writes to again. It is the
only module allowed to write, and only there, because a demo with nothing to clean
demonstrates nothing. Deleting the sandbox — from the Explorer, say — makes the next
discovery seed it again.

The seed is chosen so that every part of the contract the v1 modules need runs at least
once:

| Entry | Item | Verdict | Trash mode | Permanent mode |
|---|---|---|---|---|
| `build-cache/`, 2 MB, 45 days old | folder | Safe: untouched for 30 days | `Delete`, then `touch .last-cleanup` | `Delete` |
| `logs/`, 1 MB, 3 days old | folder | Review: modified recently | the same | the same |
| `keepsake/`, holding a `KEEP` file | folder | Keep, with a force option | the same, once forced | the same |
| `old.object`, 1 MB, 60 days old | object | Safe | `rm old.object`, `Removes` | the same |
| `fresh.object`, 3 MB, 1 day old | object | Review, size estimated | the same | the same |

What each row is for: the folders plan differently in the two modes, the way a worktree
will — two steps against one, the second one housekeeping that can fail on its own. The
objects run the same command in both modes and ask for the acknowledgement in Trash mode
too, the way a Docker image will. The Keep folder exercises the force rule, and the
estimated size the `~`. Availability rests on `locate` finding `rm` and `touch`.

The demo is also the reference for everything a real module does — `std::fs` reads under
a path from the context, typed facts, reasons with codes, plans as pure functions with
unit tests — and `docs/modules/README.md` points the author of a new module at it.

## 10. Desktop

`ModuleManager` (`src-tauri/src/module_manager.rs`) holds the registry, a `System`, the
home and the data dir, and one state per module — `idle`, `discovering`, `ready`,
`unavailable` or `failed` — beside the items of the last discovery that succeeded.

- `modules_refresh(ids)` starts a discovery of the given modules (every module when
  there are none) on a thread each: availability, then discovery, caught like the scan
  worker, so that a module that panics ends `failed` rather than `discovering` for ever.
  Every change of state emits `modules:state` with that module's view. The items of the
  previous discovery stay readable while a refresh runs, and stay when it fails — a
  refresh that failed does not empty the screen, it says so. A module that became
  unavailable drops them, since nothing can act on them.
- `modules_list()` answers the descriptors, the states and the totals;
  `cleanup_items()` every held item, largest first, at most 2000.
- `cleanup_preview(requests)` answers `{ trash, permanent }`: the engine's preview in
  each mode, the entries aligned by index. The dialog switches between the two without a
  round trip. That keeps the rule 2a wrote into the dialog — it must never show one
  mode's steps above a button armed for the other — a property of the data rather than
  of timing.
- `cleanup_run(requests, mode)` takes the `BatchLock` of 2a, because both kinds of batch
  patch the one tree. It plans again and executes, emitting `cleanup:progress`, appends
  the record, forgets the removed items, patches the Explorer's tree, and starts a
  rediscovery of the modules involved. As with `action_run`, `Err` means the batch did not
  run, and `recorded` and `treeStale` come back beside the outcome.

**Patching the Explorer.** Every target of an entry that was removed or failed is patched
out of the scan tree, as 2a does with its own paths. A module does not spell paths the
way the scan does — `git` answers `/private/tmp` for a root scanned as `/tmp` — and
`Tree::find` matches the scan's spelling byte for byte, so each path is first moved into
it: its parent resolved, the resolved root's prefix swapped for the root as scanned. The
remainder then carries the names the disk has, which are the names the walker read, so
a path the tree still does not know is one the scan never saw — the demo seeds its
sandbox after a scan, for one — and there is no row to patch. Reporting `treeStale` for
it would be a false alarm in the most ordinary run of the demo.

The commands are generic over the runtime like the others, and join `every_command!`, so
the registration test covers them.

## 11. UI

**The Cleanup page** reads from top to bottom:

- a strip with each module's state and totals — "Demo · 5 items · 7.2 MB",
  "Discovering…", "Unavailable: `docker` is not installed", "Failed: … · Retry" — and one
  Refresh. Opening the page discovers the modules that never ran;
- filters by verdict — Safe and Review on, Keep off, which is the design's "do not offer
  by default" — and by module;
- the item table: a tick box, the title and subtitle, the module, the verdict, the size
  (with `~` when estimated) and the last use; largest first, and ticked the way the
  Explorer's table is (Space, shift-click, the header box over what is visible);
- a detail panel for the focused row: the path with Reveal in Finder, the verdict and its
  reasons, the facts drawn by their type, and the action with its options — a force option
  in the danger style, saying what it overrides — and "Clean…" for this item alone;
- the Explorer's action bar: "3 selected · 4.3 MB" and "Clean…".

The selection maps an item id to the action and the options chosen for it, the defaults
until the detail panel changes them. It is pruned to what is visible whenever the filters
change, so a batch is always exactly the ticked rows on screen, and cleared after a
batch. Ids are stable across discoveries, so a refresh keeps the ticks of the items that
are still there.

The sidebar marks Cleanup "soon" while `modules_list` answers no module, which is every
release build of 2b.

**One dialog.** `ConfirmDeleteDialog` stays the one door to a deletion, over a question
that no longer assumes paths:

```ts
interface BatchRow {
  title: string;
  context?: string; // "Demo · Delete folder"
  steps?: StepView[];
  size: number;
  status: EntryStatus;
  irreversible: boolean;
}
type BatchQuestions = Record<DeletionMode, { rows: readonly BatchRow[]; totalBytes: number }>;
```

The Explorer adapts its path preview — the same rows in both modes, irreversible in
Permanent — and renders exactly what it renders today; its tests are the regression net
of the refactor. Cleanup adapts its pair of previews. The one rule that changes shape is
the acknowledgement: the dialog asks for it when any ready row is irreversible in the
chosen mode, which for the Explorer is exactly "Permanent", and for Cleanup is also "any
command that destroys". While a cleanup batch runs the dialog shows its progress
("Cleaning 2 of 5 · logs"), and the report lists titles where the Explorer's lists
paths.

Two block reasons are worded for the Explorer: `outsideRoots` says "outside the folder
that was scanned", and `isRoot` "the scanned folder itself". A cleanup row names the home
folder instead, which is the root its guards were built from.

**Activity** draws a cleanup line with the item's title, the module and the action, and
the commands that ran.

**The mock.** `src/mocks/modules.ts` holds the demo module's seeded items and
`src/mocks/cleanup.ts` mirrors the engine: the rules of section 8 over the mock guards of
2a, the same events, the same log lines, the forgetting of removed items, the patch of the
fixture tree. `crates/core/tests/fixtures/cleanup-cases.json` pins the rules the engine
decides without a disk — a missing item, an unknown action or option, `kept` and its force
option, an empty plan, reversibility per mode, the totals — and both sides answer it, as
they answer the guard cases.

## 12. CLI

`storage-monitor modules list [--json]` prints every registered module, the tools it
looks for and whether it is available; `storage-monitor modules run <id> [--json]`
discovers and prints the items with their verdicts and reasons. Both only read — apart
from the demo's first discovery, which seeds its sandbox (section 9). A release build
lists nothing. `clean --plan` waits for phase 3.

## 13. Testing

- **The port.** `RealSystem::run` against real programs: output, a non-zero exit, a
  timeout that has to kill a child which forked — the reason the whole group is killed —
  and the cap. `locate` over a temporary `PATH`; `run` refusing a relative program.
  `TestSystem`: scripts, their order, the panic on a run nobody scripted.
- **The engine.** Each rule of section 8 on its own; steps in order, stopping at the
  first failure; a refusal before the first step against one after it; a vanished target
  as a skip; a `Removes` target guarded like a `Delete`; reversibility and the reported
  mode; the progress calls; the new fields of the record, and old lines still read.
- **The demo.** Discovery over a seeded temporary data dir — verdicts, reasons, facts,
  sizes — its plans in both modes, availability with and without `rm`.
- **The desktop.** The states and events of `ModuleManager`, a module that panics,
  items kept through a failed refresh; `cleanup_run` end to end over `TestSystem`, with a
  scripted `rm` and `touch` that act on the temporary tree — record, forget, patch,
  rediscover; the registration test.
- **The CLI.** `modules list`, and `modules run demo --json` with
  `STORAGE_MONITOR_DATA_DIR` in a temporary folder.
- **Vitest.** The Cleanup page — filters, the pruning of the selection, the options of
  the detail panel; the generalized dialog — the Explorer's cases unchanged in substance, a
  command row asking for the acknowledgement in Trash mode, a Keep row blocked until
  forced, the progress; the mock engine against `cleanup-cases.json`; Activity's cleanup
  lines.
- **Playwright**, `cleanup.spec.ts`: two items cleaned in Trash mode, gone from the table
  and present in Activity; a Permanent batch behind the acknowledgement; the Keep row
  refused, then forced. The PR carries a new `cleanup.png`.

## 14. Out of scope

Settings (2c). `Presentation`, module pages and URL links (phase 3).
`storage-monitor clean` (phase 3). Stopping a batch that runs, and stopping at the first
failure (phase 4). Recording fixtures from a real machine (phase 3). Keeping discovery
results across launches.
