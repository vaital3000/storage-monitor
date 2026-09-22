# Writing a cleanup module

A module knows one kind of developer artifact: where it lives, how big it is, whether it
can go, and how it is removed. It never removes anything itself. It **finds** items,
**judges** them, and **plans** the steps that would remove one; the core runs those steps
through the same guards, preview, confirmation and record as a deletion in the Explorer
([ADR 0008](../adr/0008-modules-describe-the-core-acts.md)).

The demo module, [`crates/modules/demo`](../../crates/modules/demo/src/lib.rs), is the
reference: every part of the contract below runs in it at least once, and its tests are
the pattern to copy. The design behind all of it is
[`docs/plans/2026-09-22-phase-2b-modules-design.md`](../plans/2026-09-22-phase-2b-modules-design.md).

## Where a module lives

- A crate of its own: `crates/modules/<id>/`, package `storage-monitor-module-<id>`,
  with `version` set to the workspace's current one.
- A member of the workspace (`Cargo.toml`, `members` and `[workspace.dependencies]`).
- One line in the registry, `crates/modules/registry/src/lib.rs`. The CLI and the desktop
  app both read that list, so they cannot disagree about what a build can clean.
- Two entries in `release-please-config.json`: its `Cargo.toml` and its `Cargo.lock`
  package, in the `@.name.value` form the others use. A version nobody bumps drifts
  quietly — a rule that matches nothing only warns.

## The contract

```rust
pub trait Module: Send + Sync {
    fn descriptor(&self) -> Descriptor;                  // id, name, description, tools
    fn availability(&self, ctx: &Context) -> Availability;
    fn discover(&self, ctx: &Context) -> Result<Vec<Item>, ModuleError>;
    fn plan(&self, item: &Item, action: &ActionSpec, options: &[String], mode: Mode) -> Vec<Step>;
}
```

It is synchronous. The app runs each module's discovery on a thread of its own, and a
module that panics ends `failed` instead of taking anything down — but a module that loops
for ever leaves its row saying "Discovering…" for ever, so keep discovery bounded.

**`availability`** is asked before every discovery. Look for your tools with
`ctx.sys.locate("tool")`, and answer `Unavailable { reason }` with a sentence a user can
act on: "`docker` is not installed", "the Docker daemon is not running".

**`discover`** reads, and only reads:

- the filesystem with `std::fs`, under the paths of the context (`ctx.home`,
  `ctx.data_dir`) or paths a tool reported;
- tools through `ctx.run(&Command)` or `ctx.run_ok(&Command)`, which locate the tool, give
  it a one-minute deadline and turn a missing tool into an error that names it.

**`plan`** is a plain function of the item, the action, the options and the mode. It gets
no `System` and no clock, so it cannot look at anything: whatever it needs has to be in
the item already. The core has checked that the item offers the action and that every
option belongs to it before it calls you. An empty plan is a bug, and the core refuses the
entry as `malformed` rather than report it done.

## Items

| Field               | What it is                                                                                                             |
| ------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `id`                | `<module>:<native id>`, stable across discoveries: the selection and every request of a batch are keyed by it.         |
| `kind`              | Your own word for it — `worktree`, `image`, `simulator`.                                                               |
| `title`, `subtitle` | What the table shows.                                                                                                  |
| `path`              | Where it lives, if it lives on the filesystem; every item with one offers Reveal in Finder.                            |
| `size`              | Allocated bytes (`st_blocks * 512`), with `estimated: true` when you cannot measure it exactly — it is drawn with `~`. |
| `last_used`         | The one generic date the table shows. Other dates are facts.                                                           |
| `verdict`           | `Safe`, `Review` or `Keep`, and the reasons for it.                                                                    |
| `facts`             | Label and typed value: text, bytes, count, date, flag, path.                                                           |
| `actions`           | What can be done with it; the first is what a tick asks for.                                                           |

A **reason** has a `code` for your tests and a `text` for the user. Assert on codes.

**Keep** is enforced by the core, not by you: a Keep item is refused (`kept`) unless the
request turns on one of the action's options marked `force: true`. An action with no force
option cannot remove a Keep item at all.

**Sizes of one batch must not overlap.** `estimated_free` is what the dialog promises and
what the record says was freed; two items whose sizes count the same blocks make both
numbers lie.

## Steps

```rust
pub enum Step {
    Delete(Target),                                  // to the Trash or for good, by the mode
    Run { command: Command, effect: Effect },        // the same in both modes
}
pub enum Effect { Housekeeping, Destroys, Removes(Target) }
```

- **`Delete`** hands a path to the core, which deletes it through the port: to the Trash
  in Trash mode, for good in Permanent mode.
- **`Run`** runs a tool, by name. The core locates it when the step runs and gives it ten
  minutes. Declare what it changes:
  - `Housekeeping` — bookkeeping nobody could miss (`git worktree prune` after the folder
    went to the Trash);
  - `Destroys` — something outside the filesystem the guards see (an image, a device);
  - `Removes(target)` — a path. The guards judge it like a `Delete`, before the preview
    and again right before the command.

The rules the core holds you to:

- **Every path a step deletes passes `Limits::check`.** Only the home folder is an allowed
  root until Settings adds roots in phase 2c; the denylist and the shield of ADR 0007
  apply. A module cannot reach a path the guards refuse, because the only thing it can do
  with a path is hand it back.
- **A `Removes` target must be spelled the way the guards resolve it** — its parent
  canonicalized. The port deletes the resolved form of a `Delete`, whatever you wrote; a
  command gets your spelling and resolves it itself, later. A target that resolves to
  another spelling is refused as `malformed`. Report paths in resolved form and this never
  comes up.
- **The kind you saw must still be there**: a `Target` carries the kind of entry you found,
  and a file where you saw a folder refuses the entry.
- **Reversibility is derived from your steps**: an entry is undone by the Trash only when
  the mode is Trash and its steps are `Delete` and `Housekeeping` alone. Anything else asks
  the user for an explicit acknowledgement, in either mode.
- **Steps run in order and stop at the first failure.** Refused before its first step, an
  entry is skipped; refused or failed after one ran, it failed.

The module-level actions of a tool (`docker image prune`, `simctl delete unavailable`) are
items too: an item that stands for "dangling images", with one action.

## What a module may not do

`crates/modules/clippy.toml` refuses `std::process::Command::new`, `std::fs::remove_file`,
`remove_dir`, `remove_dir_all` and `rename` in every crate under that folder. Deleting is a
`Step`, and running a tool is `ctx.run` or a `Step::Run`. A test that has to stage a
disappearance goes through the port too — `TestSystem::remove`.

Do not write outside your own data. The demo module seeds a sandbox in the data dir, which
is the one exception and the reason it ships in debug builds only.

`/usr/bin/git` and `/usr/bin/xcrun` exist on every Mac, but without the Command Line Tools
they are shims that open an installation dialog when run. `locate` finds them either way:
ask `xcode-select -p` before you run one.

## Testing

Everything runs against `TestSystem` (the `testing` feature of `storage-monitor-core`), on
any platform:

- a temporary tree — pass `sys.root()` (or a folder in it) as `home` and `data_dir`;
- tools: `sys.install("docker")` makes `locate` find it, and
  `sys.script("docker", &["system", "df", "--format", "json"], Reply::ok().stdout(json))`
  answers one run. A run nobody scripted panics with its argv, so a plan that went wrong
  stops the test instead of running something;
- `Reply::then(effect)` gives a scripted tool the consequence the real one has — a scripted
  `rm` that removes the file, through the port;
- `sys.replay(path)` scripts a run recorded in a fixture file:
  `{ "tool", "args", "status", "stdout", "stderr" }`, with `status` null for a process a
  signal ended;
- `sys.ran()` says what ran, in order;
- the clock is fixed and moves with `sys.advance`, so ages in verdicts are exact.

Test the verdict rules one by one, the plans as plain functions in both modes, and one
batch end to end through `cleanup::execute` with `Limits::for_home(sys.root())` — the
demo's `the_demo_cleans_end_to_end_through_the_engine` is the template.

## Checklist for a new module

- [ ] The crate, the workspace entries, the registry line, the release-please entries.
- [ ] `descriptor`: an id without `:`, a name, one sentence, the tools by name.
- [ ] `availability` with a sentence for every way it can be missing.
- [ ] Item ids stable across discoveries; paths in resolved form; sizes that do not
      overlap.
- [ ] Verdict rules with reasons, each rule tested by its code.
- [ ] Plans for both modes, `Effect` declared honestly, tested as plain functions.
- [ ] Recorded fixtures for every tool you run, replayed through `TestSystem`; the tests pass
      on Linux.
- [ ] A section below describing the module, and the design's module section updated.

## Modules

- **demo** — sample folders and objects in `<data dir>/demo`, seeded on the first
  discovery; debug builds only. Folders plan `Delete` (plus a housekeeping `touch` in Trash
  mode); objects plan `rm`, which removes a path. A folder holding a `KEEP` file is Keep,
  with a force option.
