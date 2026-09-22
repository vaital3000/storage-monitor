# Phase 2b: Cleanup Modules Implementation Plan

**Goal:** Ship the module framework — process execution in the `System` port, the module
contract and registry, the cleanup engine, a demo module in debug builds — and the Cleanup
screen that cleans what the modules find, in both modes, through the guards, the dialog and
the record of phase 2a.

**Architecture:** `storage-monitor-core` gains `System::locate` and `System::run`, a
`module` module (the contract: descriptor, availability, discovery, pure planning into
steps) and a `cleanup` module (preview and execute over steps, reusing the guards and the
path deletion of `action`). Modules live under `crates/modules/`, where `clippy.toml`
forbids deleting and spawning; the registry crate lists them for the CLI and the desktop.
The desktop gains a `ModuleManager` (discovery per module on threads, `modules:state`
events) and cleanup commands with `cleanup:progress` events. The UI adds the Cleanup page and
generalizes `ConfirmDeleteDialog` from a path preview to a batch question.

**Tech Stack:** Rust (std process and threads, nix `signal`, chrono, serde_json, thiserror,
tempfile), Tauri 2, React 19, TanStack Query 5, Vitest, Playwright.

**Design reference:** `docs/plans/2026-09-22-phase-2b-modules-design.md`. Decision:
`docs/adr/0008-modules-describe-the-core-acts.md`. Guards and log: phase 2a's design and
`crates/core/src/action/`.

**Decisions already made (do not re-open):**

- 2b is modules and Cleanup; Settings is 2c; `Presentation`, module pages, URL links and
  `storage-monitor clean` are phase 3.
- The demo module is registered behind `cfg!(debug_assertions)`; a release build has an
  empty registry and a Cleanup section marked "soon".
- The module trait is synchronous; `plan` takes no `System`.
- Allowed roots for module deletions: the home folder (`Limits::for_home`).
- A cleanup preview is computed for both modes at once; `cleanup_run` plans again from the
  requests and never takes a preview.
- `ConfirmDeleteDialog` is generalized, not duplicated.

**Conventions for every task:**

- Work in the worktree `.worktrees/phase-2b-modules` on branch `feat/phase-2b-modules`.
  Never commit to `main`.
- TDD: write the failing test, watch it fail, write the minimal implementation, watch it
  pass, commit.
- Conventional commits, one concern each: `feat(core): ...`, `feat(demo): ...`,
  `feat(desktop): ...`, `feat(cli): ...`, `test(desktop): ...`.
- Rust structs crossing IPC carry `#[serde(rename_all = "camelCase")]`; every new command
  gets a wrapper in `src/lib/ipc.ts`, a handler in `src/mocks/ipc.ts`, and a line in
  `every_command!` with an expectation in the registration test.
- After every Rust task: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`.
- The local Homebrew toolchain may lag CI's stable; run `just clippy-ci` (Docker) before
  pushing.

---

### Task 1: Running a program through the port

**Files:**

- Create: `crates/core/src/system/process.rs`
- Modify: `Cargo.toml` (nix features), `crates/core/src/system/mod.rs`,
  `crates/core/src/system/real.rs`, `crates/core/src/system/test.rs` (a stub `locate`/`run`
  that panics until Task 2)

**Step 1: The dependency**

In the root `Cargo.toml`: `nix = { version = "0.31", features = ["fs", "signal"] }`
(`signal` implies `process`, which carries `Pid`). No new crate.

**Step 2: The types, in `system/mod.rs`**

```rust
/// A program to run: the argv, where, with what added to the environment, and for how long.
pub struct Invocation {
    /// Absolute and in normal form, like every path the port takes; nothing is resolved.
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    /// Added to the inherited environment.
    pub env: Vec<(OsString, OsString)>,
    pub timeout: Duration,
}

pub struct Output {
    /// `None` when a signal ended the process.
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn success(&self) -> bool { self.status == Some(0) }
}
```

New `SystemError` variants: `Spawn { program, source }`, `Wait { program, source }`,
`TimedOut { program, after }`, `OutputTooLarge { program }`. The trait gains `locate` and
`run` with the doc comments of design section 4.

**Step 3: Failing tests in `system/process.rs` and `system/real.rs`**

Real processes, which exist on macOS and on the Linux runners:

- `run_captures_stdout_and_the_exit_status` — `/bin/echo hello` → `status == Some(0)`,
  stdout `hello\n`.
- `a_non_zero_exit_is_an_output_not_an_error` — `/bin/sh -c 'echo no >&2; exit 3'` →
  `Ok`, `status == Some(3)`, stderr `no\n`. (A test of the port may use `sh`: the rule
  against shell strings is about the app's commands, and this is the simplest way to stage
  an exit code and stderr.)
- `run_passes_arguments_without_a_shell` — `/bin/echo '$HOME' '*'` prints them literally.
- `run_uses_the_working_directory_and_the_environment` — `/bin/pwd` in a temp dir;
  `/usr/bin/env` with `SM_PROBE=1` in `env` lists it.
- `a_timeout_kills_the_process` — `/bin/sleep 10`, timeout 100 ms → `TimedOut` within 2 s.
- `a_timeout_kills_the_children_too` — `/bin/sh -c 'sleep 10; echo late'` (the shell forks
  `sleep` and waits): `TimedOut` within 2 s. Without the group kill the readers would block
  until `sleep` exits; this is the test that proves the group.
- `output_past_the_cap_is_an_error` — with the cap lowered through a `pub(crate)` seam
  (`run_capped(inv, cap)`), `/bin/sh -c 'head -c 4096 /dev/zero'` with a 1 KiB cap →
  `OutputTooLarge`, and the process was not left blocked.
- `a_relative_program_is_rejected` — `program: "echo"` → `Rejected`, nothing ran.
- `a_missing_program_is_a_spawn_error` — `/nonexistent/tool` → `Spawn`.
- `locate_finds_the_first_executable_in_order` — over `locate_in(tool, path_var, known)`, a
  pure seam: two temp dirs, the tool in both, the first wins; a non-executable file is
  skipped; a relative `PATH` entry is ignored; a tool name holding `/` answers `None`; an
  entry like `/a/./b` is normalized, so the answer is in the port's normal form.

**Step 4: Implementation**

`process.rs` holds `run_capped(inv: &Invocation, cap: usize) -> Result<Output, SystemError>`:

1. `std::process::Command::new(&inv.program)`, `.args`, `.envs`, `.current_dir`,
   `stdin(Stdio::null())`, `stdout/stderr(Stdio::piped())`, and
   `std::os::unix::process::CommandExt::process_group(0)`.
2. Two reader threads, each reading 64 KiB chunks until EOF, keeping at most `cap` bytes and
   draining the rest; each answers `(Vec<u8>, overflowed)`.
3. Poll `try_wait` every 10 ms until the deadline. At the deadline:
   `nix::sys::signal::killpg(Pid::from_raw(child.id() as i32), Signal::SIGKILL)`, `wait`,
   give the readers one second, then answer `TimedOut` — detaching a reader that is still
   blocked (a grandchild that left the group can hold the pipe; the thread ends with it).
4. After a normal exit, wait for the readers until the same deadline; if it passes, kill the
   group and answer `TimedOut`.
5. `OutputTooLarge` when either reader overflowed.

`RealSystem::run` calls `check_path` on the program and on `cwd`, then `run_capped` with
the 32 MiB cap. `RealSystem::locate` calls `locate_in(tool, env::var_os("PATH"), KNOWN)`
where `KNOWN` is `/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin`, `/bin`, `/usr/sbin`,
`/sbin`, searched after the absolute entries of `PATH`; an executable is a regular file
(symlinks followed — `docker` is a link into its app bundle) with any `x` bit.

`TestSystem` gets `locate`/`run` that `unimplemented!()` for now; Task 2 replaces them.

**Step 5: Run and commit**

```bash
cargo test -p storage-monitor-core system::
git commit -m "feat(core): run programs through the System port"
```

---

### Task 2: Scripting processes in `TestSystem`

**Files:** Modify `crates/core/src/system/test.rs`, `crates/core/src/system/mod.rs` (export
`Reply`).

**Step 1: Failing tests**

- `an_installed_tool_is_located_in_the_fake_bin` — `install("rm")`; `locate("rm")` is
  `<base>/bin/rm`; `locate("git")` is `None`.
- `a_scripted_run_answers_its_reply_once` — script `rm ["/x"]` → `Reply::ok()`; the run
  answers status 0; a second identical run panics.
- `scripts_match_on_the_file_name_and_the_arguments` — the program's directory is ignored;
  different arguments do not match.
- `replies_for_one_argv_are_used_in_order` — two scripts for the same argv answer in turn.
- `an_unscripted_run_panics_with_its_argv` — `#[should_panic(expected = "rm /y")]`.
- `a_reply_can_act_on_the_temporary_tree` — `Reply::ok().then(move || fs::remove_file(&p).unwrap())`
  removes the file when the run happens, not when it is scripted.
- `a_reply_can_fail_like_the_port` — `Reply::timeout()` answers `Err(TimedOut)`.
- `every_run_is_recorded` — `ran()` returns the argv of each run, program by file name.
- `a_fixture_file_scripts_a_run` — `replay(path)` reads
  `{ "tool", "args", "status", "stdout", "stderr" }` and scripts it.
- `run_still_refuses_a_relative_program` — the port's rule holds in the fake too.

**Step 2: Implementation**

```rust
pub struct Reply { /* status, stdout, stderr, or an error; an optional effect */ }
impl Reply {
    pub fn ok() -> Self;
    pub fn exit(code: i32) -> Self;
    pub fn stdout(self, text: impl Into<Vec<u8>>) -> Self;
    pub fn stderr(self, text: impl Into<Vec<u8>>) -> Self;
    pub fn timeout() -> Self;
    pub fn then(self, effect: impl FnOnce() + Send + 'static) -> Self;
}
impl TestSystem {
    pub fn install(&self, tool: &str);
    pub fn script(&self, tool: &str, args: &[&str], reply: Reply);
    pub fn replay(&self, fixture: &Path);
    pub fn ran(&self) -> Vec<Vec<String>>;
}
```

Scripts live in `Mutex<HashMap<(String, Vec<OsString>), VecDeque<Reply>>>`; the key is the
program's file name and the args. The panic message is the argv joined with spaces.

**Step 3: Commit** — `feat(core): script processes in the test system`

---

### Task 3: The module contract

**Files:**

- Create: `crates/core/src/module/mod.rs`, `crates/core/src/module/item.rs`,
  `crates/core/src/module/step.rs`
- Modify: `crates/core/src/lib.rs` (doc comment, `pub mod module`)

**Step 1: Failing tests** — the wire shapes, as `action/model.rs` pins its own:

- `an_item_crosses_the_wire_in_camel_case` — every field, `lastUsed` as RFC 3339,
  `size: { bytes, estimated }`, `verdict: { level: "keep", reasons: [{ code, text }] }`.
- `a_fact_value_is_tagged_by_type` — `{ "type": "bytes", "value": 10 }`, and one per
  variant, round-tripping.
- `an_action_option_says_whether_it_forces` — `force: true` serializes.
- `a_command_line_reads_as_one_argument_per_word` — `command_line("rm", ["/a b/c"])` is
  `rm '/a b/c'`; a quote inside is escaped; an empty argument is `''`; a plain word stays
  bare.
- `context_run_locates_the_tool_and_uses_the_discovery_timeout` — over `TestSystem`:
  `ctx.run(&Command::new("git").arg("status"))` runs `<base>/bin/git status`; an
  uninstalled tool is `ModuleError::ToolMissing("git")` and nothing ran.
- `context_run_ok_turns_a_non_zero_exit_into_an_error` — naming the tool, the code and the
  end of stderr.

**Step 2: Implementation** — the types of design sections 5 to 7:

- `module/mod.rs`: `Module`, `Descriptor`, `Availability`, `Context { sys, home, data_dir }`
  with `run` and `run_ok`, `ModuleError` (`ToolMissing`, `Failed { tool, status, stderr }`,
  `System(#[from] SystemError)`, `Io { path, source }`, `Other(String)`),
  `DISCOVERY_TIMEOUT = 60 s`.
- `module/item.rs`: `Item`, `Size`, `Level`, `Verdict`, `Reason`, `Fact`, `FactValue`
  (`#[serde(tag = "type", content = "value", rename_all = "camelCase")]`), `ActionSpec`,
  `ActionOption`. All `Serialize + Deserialize`, camelCase.
- `module/step.rs`: `Step`, `Target`, `Command` (a builder: `new(tool)`, `arg`, `args`,
  `cwd`, `env`), `Effect`, and `command_line(tool, args) -> String`, the display form of
  design section 7. Quote an argument unless it is made only of `[A-Za-z0-9_./:=@%+,-]`;
  quote with `'`, writing a `'` inside as `'\''`.

**Step 3: Commit** — `feat(core): the module contract`

---

### Task 4: The cleanup preview

**Files:**

- Create: `crates/core/src/cleanup/mod.rs`, `crates/core/src/cleanup/model.rs`,
  `crates/core/src/cleanup/engine.rs`
- Modify: `crates/core/src/action/model.rs` (`BlockReason::Kept`),
  `crates/core/src/action/guards.rs` (`Limits::for_home`),
  `crates/core/src/action/engine.rs` (share the target check), `crates/core/src/lib.rs`

**Step 1: Share what 2a already does for one path**

Extract from `action::engine::check_entry` a `pub(crate) fn inspect(path, limits, sys) ->
Result<Inspected, BlockReason>` (`Inspected { path, judged, kind }`: the guards, then the
port's `symlink_metadata`, with `reason_for` mapping its errors) and from `run_entry` a
`pub(crate) fn delete(path, mode, sys) -> Result<(), SystemError>`. `check_entry` and
`run_entry` call them. No behavior changes: the whole 2a suite passes untouched, and that is
this step's test.

**Step 2: Failing tests** in `cleanup/engine.rs`, over `TestSystem` (home = its root) and a
table-driven test module (`struct Scripted` in the test module: items and plans given
directly):

- `a_ready_entry_lists_its_steps_and_its_size` — `Delete` → a `trash` step view in Trash
  mode and a `delete` view in Permanent; `size` is the action's `estimated_free`.
- `an_unknown_item_is_missing` — rule 1.
- `an_action_the_item_does_not_offer_is_kind_changed` and
  `an_option_the_action_does_not_have_is_kind_changed` — rule 2.
- `a_keep_item_is_kept_without_its_force_option` and
  `a_keep_item_is_ready_with_its_force_option` — rule 3; an option not marked `force` does
  not count.
- `an_empty_plan_is_malformed` — rule 4.
- `a_target_outside_the_home_folder_is_outside_roots` — rule 5, for `Delete` and for
  `Removes` alike; `a_missing_target_is_missing`; `a_target_of_another_kind_is_kind_changed`.
- `a_target_inside_another_entrys_target_is_nested` — rule 6; two targets of one entry
  nested in each other do not block it.
- `reversibility_follows_the_steps_and_the_mode` — Delete + Housekeeping in Trash:
  reversible; the same in Permanent: not; a `Destroys` or `Removes` step: not, in either.
- `the_total_counts_ready_entries_only`.
- `the_first_rule_that_applies_decides` — a missing item that would also be kept is
  `missing`.

**Step 3: Implementation**

`cleanup/model.rs` (wire types, camelCase, `Serialize` only — nothing here is ever read back
from the UI):

```rust
pub struct Request { pub item: String, pub action: String, pub options: Vec<String> } // + Deserialize
pub enum StepView { Trash { path }, Delete { path }, Run { command, effect: EffectView, path: Option<String> } }
pub struct CleanupEntry {
    pub item: String, pub module: String, pub title: String, pub action: String,
    pub steps: Vec<StepView>, pub size: u64, pub status: EntryStatus, pub reversible: bool,
}
pub struct CleanupPreview { pub entries: Vec<CleanupEntry>, pub total_bytes: u64, pub mode: Mode }
```

`cleanup/engine.rs`:

```rust
/// The latest discovery results by item id, with the module each came from.
pub struct Held<'a> { /* HashMap<&str, (&dyn Module, &Item)> */ }
impl<'a> Held<'a> { pub fn new(results: impl IntoIterator<Item = (&'a dyn Module, &'a [Item])>) -> Self; }

pub fn preview(requests: &[Request], held: &Held, limits: &Limits, sys: &dyn System, mode: Mode) -> CleanupPreview;
pub fn reversible(steps: &[Step], mode: Mode) -> bool;
/// Rules 1 to 3, disk-free: the shared cases of Task 7 call it directly.
pub fn screen<'a>(request: &Request, held: &Held<'a>) -> Result<(&'a dyn Module, &'a Item, &'a ActionSpec), BlockReason>;
```

A private `plan_all` returns `Planned { request index, item, action, steps, status, judged targets }`
per request; `preview` maps it to the wire, and Task 5's `execute` runs it. Nested is a
cross-entry pass with the predicate of `drop_nested` (a strict ancestor wins; between
equals, the earlier entry), comparing only targets of different entries.

`Limits::for_home(home)` is `with_home(home.clone(), Some(home))`.

**Step 4: Commit** — `feat(core): preview a cleanup batch`

---

### Task 5: Executing a cleanup batch

**Files:** Modify `crates/core/src/cleanup/engine.rs`, `crates/core/src/cleanup/model.rs`.

**Step 1: Failing tests**

- `every_step_runs_in_order` — Trash-mode folder: the folder is in the fake Trash, then the
  scripted `touch` ran; `Removed { bytes }`.
- `a_failed_step_ends_its_entry_not_the_batch` — a second entry still runs.
- `a_refusal_before_the_first_step_is_a_skip` — target gone between preview and execution:
  `Skipped { missing }`; nothing ran.
- `a_refusal_after_a_step_ran_is_a_failure` — step 1 deletes, step 2's target is gone:
  `Failed`, the message says "step 2 of 2".
- `a_non_zero_exit_fails_the_entry_with_the_end_of_stderr` — status 1, three lines of
  stderr: the message holds the last ones and the command line.
- `a_missing_tool_fails_the_entry_and_runs_nothing`.
- `a_port_error_fails_the_entry` — `Reply::timeout()`.
- `a_removes_target_is_revalidated_before_the_command` — kind changed on disk →
  `Skipped { kindChanged }`, the command never ran (`ran()` is empty).
- `blocked_entries_are_skipped_with_their_reason`.
- `execute_plans_again_from_the_requests` — the steps that run are the module's, whatever
  the caller previewed before.
- `progress_is_reported_before_each_entry_and_at_the_end`.
- `an_irreversible_entry_reports_the_permanent_mode` — a `Destroys` entry in a Trash batch
  has `mode: permanent` in its outcome; a reversible one keeps `trash`.
- `the_outcome_lists_the_commands_that_started_and_the_targets`.
- `freed_bytes_is_the_sum_of_removed_entries`.

**Step 2: Implementation**

```rust
pub struct Progress { pub done: usize, pub total: usize, pub current: Option<String> }
pub struct CleanupEntryOutcome {
    pub item: String, pub module: String, pub title: String, pub action: String,
    pub path: Option<PathBuf>, pub targets: Vec<PathBuf>, pub mode: Mode,
    pub commands: Vec<Vec<String>>, pub result: EntryResult,
}
pub struct CleanupOutcome { pub entries: Vec<CleanupEntryOutcome>, pub freed_bytes: u64, pub at: DateTime<Utc>, pub mode: Mode }

pub fn execute(requests: &[Request], held: &Held, limits: &Limits, sys: &dyn System, mode: Mode,
               progress: &mut dyn FnMut(Progress)) -> CleanupOutcome;
```

Per step: a `Delete` or `Removes` target goes through `inspect` and the kind comparison; a
`Delete` then through `delete(path, mode, sys)`; a `Run` locates its tool and runs with
`STEP_TIMEOUT` (10 minutes). The message of a failed step is
`"step {i} of {n}, `{line}`: {what}"`, without the step prefix when the entry has one step;
`what` is `exited with {code}: {last three non-empty lines of stderr, 500 chars at most}`,
`was killed by a signal`, `{tool} is not installed`, or the port's error. `targets` holds
the normalized path of every `Delete`/`Removes` step that was attempted.

**Step 3: Commit** — `feat(core): execute a cleanup batch`

---

### Task 6: Recording a cleanup batch

**Files:** Modify `crates/core/src/action/log.rs`, `crates/core/src/action/mod.rs`, the
callers of `LogEntry::path` in `apps/desktop/src-tauri/src/actions.rs` tests.

**Step 1: Failing tests**

- `an_explorer_line_keeps_its_exact_bytes` — the JSON of a 2a line is unchanged: no
  `source`, no `commands`, `path` and `kind` present.
- `a_cleanup_line_carries_its_source_and_commands` —
  `source: { module, item, title, action }`, `commands: [["rm", "/…"]]`, the entry's own mode.
- `a_cleanup_line_without_a_path_leaves_it_out` — and reads back as `None`.
- `a_line_written_by_phase_2a_still_reads` — a literal line of 2a parses; `source` is `None`.
- `a_cleanup_batch_is_one_write` — `append_cleanup` shares `append`'s single `write_all`
  and its repair of a torn end (the existing tests, run through the new entry point).

**Step 2: Implementation** — `LogEntry.path: Option<String>`, `kind: Option<NodeKind>`,
both `#[serde(default, skip_serializing_if = "Option::is_none")]`; new
`source: Option<Source>` and `commands: Vec<Vec<String>>` with the same attributes
(`Vec::is_empty` for the second). `ActionLog::append_cleanup(&CleanupOutcome)`; the body of
`append` after building the lines becomes a private `write_lines`. For a cleanup line,
`kind` is the kind of the target whose path is the item's path, when the plan had one.

**Step 3: Commit** — `feat(core): record cleanup batches in the action log`

---

### Task 7: The shared cleanup cases

**Files:**

- Create: `crates/core/tests/fixtures/cleanup-cases.json`
- Modify: `crates/core/src/cleanup/engine.rs` (the runner)

The cases pin what the engine decides without a disk: `screen` (rules 1 to 3) and
`reversible`. Items and requests are literal; plans are lists of step kinds (`delete`,
`housekeeping`, `destroys`, `removes`). Around a dozen cases: missing item, unknown action,
unknown option, keep without force, keep with a non-force option, keep with force, review
and safe, each step mix in both modes. `the_shared_cleanup_cases_hold` runs them; Task 13
runs the same file in Vitest.

**Commit** — `test(core): pin the cleanup rules in shared cases`

---

### Task 8: The demo module

**Files:**

- Create: `crates/modules/clippy.toml`, `crates/modules/demo/Cargo.toml`,
  `crates/modules/demo/src/lib.rs`, `crates/modules/demo/src/seed.rs`
- Modify: `Cargo.toml` (members, `[workspace.dependencies]`), `release-please-config.json`

**Step 1: `clippy.toml`, with a positive control**

```toml
# Modules describe; the core acts (ADR 0008). Every crate under this folder is a module or
# the registry of them, and none of them deletes or starts a process by itself: they plan
# steps, and `System` is the only door.
disallowed-methods = [
  { path = "std::process::Command::new", reason = "run through Context::run, or plan a Step::Run" },
  { path = "std::fs::remove_file", reason = "plan a Step::Delete; the core deletes" },
  { path = "std::fs::remove_dir", reason = "plan a Step::Delete; the core deletes" },
  { path = "std::fs::remove_dir_all", reason = "plan a Step::Delete; the core deletes" },
  { path = "std::fs::rename", reason = "plan a Step::Delete; the core deletes" },
]
```

Plant `std::process::Command::new("true")` in the demo crate, run clippy, see it fail, remove
it. Record that in the commit message: the rule is only worth something if it was seen to
fire.

**Step 2: Failing tests** over `TestSystem` (home and data dir inside its root):

- `the_first_discovery_seeds_the_sandbox` — five entries with the sizes and ages of design
  section 9, dated from `sys.now()`.
- `a_later_discovery_does_not_seed_again` — after removing an item, it stays removed.
- `verdicts_follow_age_and_the_keep_marker` — codes `stale`, `recent`, `marked-keep`, and
  sentences with the number of days.
- `folders_report_files_modified_and_path_facts`; `objects_report_an_estimated_size`.
- `a_keep_folder_offers_a_force_option`.
- `a_folder_plans_two_steps_in_trash_mode_and_one_in_permanent`.
- `an_object_plans_rm_in_both_modes` — `Removes(Target { path, File })`.
- `available_when_rm_and_touch_are_installed`; `unavailable_names_the_missing_tool`.
- `items_are_sorted_largest_first` — not required by the contract, but it keeps the CLI
  output stable.

**Step 3: Implementation** — as design section 9. The seed writes real bytes (sparse files
would report no allocated size), sets `mtime` with `File::set_modified` on files and then on
their folders, children first. Sizes are allocated bytes (`st_blocks * 512`), like the
scanner's.

**Step 4: Versions** — the crate carries `version = "0.4.0"`, and `release-please-config.json`
gains its `Cargo.toml` and `Cargo.lock` entries, in the `@.name.value` form CLAUDE.md
describes.

**Step 5: Commit** — `feat(demo): a demo module over a sandbox in the data dir`

---

### Task 9: The registry

**Files:** Create `crates/modules/registry/Cargo.toml`, `crates/modules/registry/src/lib.rs`;
modify `Cargo.toml`, `release-please-config.json`.

**Tests:** `the_debug_registry_holds_the_demo`, `module_ids_are_unique`,
`every_module_describes_its_tools`.

```rust
/// Every module this build ships. The demo module is a debug build's only (design 2b,
/// section 9); a release build of 2b has none, and its Cleanup section says "soon".
pub fn registry() -> Vec<Box<dyn Module>> {
    let mut modules: Vec<Box<dyn Module>> = Vec::new();
    if cfg!(debug_assertions) {
        modules.push(Box::new(storage_monitor_module_demo::Demo));
    }
    modules
}
```

**Commit** — `feat(core): a registry of the compiled-in modules`

---

### Task 10: `storage-monitor modules`

**Files:** Create `crates/cli/src/modules_cmd.rs`, `crates/cli/tests/modules.rs`; modify
`crates/cli/src/main.rs`, `crates/cli/Cargo.toml`.

**Tests** (the binary, with `STORAGE_MONITOR_DATA_DIR` in a temp dir):

- `modules_list_names_the_demo_module_as_available` (text and `--json`).
- `modules_run_demo_prints_its_items_with_verdicts` — `--json` holds five items; the sandbox
  now exists in the temp data dir.
- `modules_run_an_unknown_id_fails_with_the_known_ids`.
- `modules_output_survives_a_closed_pipe` — `| head -1`, as `scan` does.

Text output of `run`: one line per item — verdict, size (`human_bytes`, `~` when
estimated), title — then its reasons indented.

**Commit** — `feat(cli): list and run the cleanup modules`

---

### Task 11: `ModuleManager`

**Files:** Create `apps/desktop/src-tauri/src/module_manager.rs`; modify
`apps/desktop/src-tauri/src/views.rs`, `lib.rs`, `Cargo.toml`.

**Step 1: Failing tests** (a recorder emitter; `TestSystem` in an `Arc`; small test modules
beside the demo):

- `a_module_is_idle_until_refreshed`.
- `refresh_discovers_and_emits_each_state` — `discovering`, then `ready`, with the totals.
- `an_unavailable_module_says_why_and_holds_no_items`.
- `a_module_that_fails_keeps_the_items_of_its_last_success`.
- `a_panicking_module_ends_failed` — the message names the panic.
- `an_older_discovery_cannot_overwrite_a_newer_one` — a refresh during a refresh.
- `items_are_largest_first_and_capped`.
- `forget_drops_items_until_the_next_discovery`.
- `the_emitter_is_called_under_the_lock_and_never_re_enters` — as `StatusEmitter`'s docs
  require.

**Step 2: Implementation**

```rust
pub trait ModuleEmitter: Send + Sync + 'static { fn emit(&self, view: &ModuleView); }
pub struct ModuleManager { /* Arc<Mutex<Inner>>, registry, Arc<dyn System>, home, data_dir */ }
impl ModuleManager {
    pub fn new(registry: Vec<Box<dyn Module>>, sys: Arc<dyn System>, home: PathBuf, data_dir: PathBuf) -> Self;
    pub fn list(&self) -> Vec<ModuleView>;
    pub fn refresh(&self, emitter: Arc<dyn ModuleEmitter>, ids: &[String]) -> Vec<ModuleView>;
    pub fn items(&self, limit: usize) -> Vec<Item>;
    pub fn with_held<T>(&self, f: impl FnOnce(&Held) -> T) -> T;
    pub fn forget(&self, item_ids: &[String]);
    pub fn home(&self) -> &Path;
    pub fn system(&self) -> &dyn System;
}
```

`ModuleView { id, name, description, status, reason, itemCount, totalBytes, safeBytes,
discoveredAt }` in `views.rs`, camelCase. The event name is `modules:state`. The Tauri
`AppHandle<R>` implements `ModuleEmitter` like it implements `StatusEmitter`.

**Commit** — `feat(desktop): discover modules on threads and report their states`

---

### Task 12: The cleanup commands

**Files:** Create `apps/desktop/src-tauri/src/cleanup.rs`; modify `commands.rs`, `lib.rs`
(`every_command!`, `configure`), `views.rs`.

**Step 1: Failing tests**

In `cleanup.rs`, end to end over `TestSystem` and the demo module (home and data dir in its
root, `rm` and `touch` installed, the replies acting on the tree):

- `preview_answers_both_modes_aligned` — the folder has two steps in `trash`, one in
  `permanent`; the object is irreversible in both.
- `a_batch_cleans_records_forgets_and_rediscovers` — Trash batch of a folder and an object:
  the folder is in the fake Trash, the object is gone, two log lines with `source`, the
  items are gone from `items()` at once, and a `modules:state` for `demo` follows.
- `a_batch_patches_the_explorer_tree` — with a scan of the home held: the folder's row is
  gone and its ancestors shrank.
- `a_target_spelled_otherwise_is_moved_into_the_scan_spelling` — a scan rooted through a
  symlink; the patch still lands.
- `a_target_the_scan_never_saw_is_not_stale` — a scan taken before the sandbox was seeded:
  `treeStale` is false.
- `progress_events_are_emitted_per_entry`.
- `cleanup_and_explorer_batches_share_the_queue` — `BatchLock` held during `run_cleanup`.
- `an_unrecorded_batch_still_reports_its_outcome` — log in an unwritable dir:
  `recorded: false`.

In `lib.rs`: the registration test gains an expectation for `modules_list`,
`modules_refresh`, `cleanup_items`, `cleanup_preview` and `cleanup_run`.

**Step 2: Implementation** — the commands of design section 10. `cleanup_run` is
`async` with its body in `spawn_blocking`, like `action_run`; the progress callback emits
`cleanup:progress` through the handle. `configure` manages a `ModuleManager` built with
`storage_monitor_modules::registry()`, `Arc::new(RealSystem)`, `paths::home_dir()` and
`paths::data_dir()`; it takes the manager as a parameter, so the registration test passes a
temporary one.

The spelling of a target for the patch: if it starts with the root as scanned, as is;
otherwise its parent canonicalized plus its name, and if that starts with the canonical
root, the root as scanned joined with the remainder; otherwise it is outside the scan and
nothing is patched.

**Step 3: Commit** — `feat(desktop): cleanup commands with progress and patching`

---

### Task 13: The IPC surface and the mock

**Files:**

- Modify: `apps/desktop/src/lib/ipc.ts`, `src/lib/ipc.test.ts`, `src/mocks/ipc.ts`,
  `src/mocks/actionLog.ts`, `src/lib/blockReasons.ts`
- Create: `src/mocks/modules.ts`, `src/mocks/cleanup.ts`, `src/mocks/cleanup.test.ts`,
  `src/mocks/modules.test.ts`

**Step 1: Types and commands** in `ipc.ts`: `VERDICT_LEVELS`, `ModuleView`, `Item`,
`FactValue`, `ActionSpec`, `ActionOption`, `CleanupRequest`, `StepView`, `CleanupEntry`,
`CleanupPreview`, `CleanupPreviews`, `CleanupEntryOutcome`, `CleanupOutcome`,
`CleanupResult`, `CleanupProgress`; `modulesList`, `modulesRefresh`, `cleanupItems`,
`cleanupPreview`, `cleanupRun`, `onModulesState`, `onCleanupProgress`. `BLOCK_REASONS`
gains `kept`. `ActivityEntry.path` and `kind` become nullable; `source` and `commands` are
optional.

**Step 2: Failing tests**

- `mocks/cleanup.test.ts`: `cleanup-cases.json` answered by the mock's `screen` and
  `reversible` (imported as JSON, the way `actions.test.ts` imports the guard cases); a
  preview of the demo items in both modes; a batch removes, logs with `source`, emits
  progress, forgets the items and emits `modules:state`; a Keep folder without force is
  `kept`.
- `mocks/modules.test.ts`: refresh emits `discovering` then `ready`; `setMockModules([])`
  empties the registry; `setMockModuleFailure('demo', '…')` makes the next refresh fail
  and keeps the items.
- `mocks/actionLog.test.ts`: a cleanup line reads back with its `source` and commands; a line
  without `path` reads; a 2a line still reads.
- `blockReasons.test.ts`: `kept` has words; the two scan-worded reasons have home-worded
  twins (`describeBlock(reason, 'home')`).

**Step 3: Implementation** — the mock mirrors the engine rules through
`checkPath(limitsFor(FIXTURE_ROOT), target)` for placement and the mock sandbox for
existence; a batch patches the fixture tree only for targets the fixture knows (none of
the demo's, which matches the real "the scan never saw it"). Test hooks on
`window.__STORAGE_MONITOR_MOCK__`: `setMockModules`, `setMockModuleFailure`,
`resetMockModules`.

**Commit** — `feat(desktop): the IPC surface of cleanup, and its mock`

---

### Task 14: One dialog for both kinds of batch

**Files:**

- Create: `apps/desktop/src/lib/batchQuestion.ts`, `src/lib/batchQuestion.test.ts`
- Modify: `src/components/ConfirmDeleteDialog.tsx`,
  `src/components/ConfirmDeleteDialog.test.tsx`, `src/pages/ExplorerPage.tsx`

**Step 1: The adapters, test first**

```ts
export interface BatchRow { title: string; context?: string; steps?: readonly StepView[];
  size: number; status: EntryStatus; irreversible: boolean }
export interface BatchQuestion { rows: readonly BatchRow[]; totalBytes: number }
export interface BatchQuestions { trash: BatchQuestion; permanent: BatchQuestion;
  wording: 'delete' | 'clean'; scope: 'scan' | 'home' }
export interface ReportRow { title: string; mode: DeletionMode; result: EntryResult }
export interface BatchReport { rows: readonly ReportRow[]; freedBytes: number; mode: DeletionMode;
  recorded: boolean; treeStale: boolean; wording: 'delete' | 'clean'; scope: 'scan' | 'home' }

export function questionsFromPreview(preview: Omit<Preview, 'mode'>): BatchQuestions;
export function reportFromBatch(result: BatchResult): BatchReport;
export function questionsFromCleanup(previews: CleanupPreviews, moduleNames: ReadonlyMap<string, string>): BatchQuestions;
export function reportFromCleanup(result: CleanupResult): BatchReport;
```

Tests: the Explorer's rows are the same in both modes and irreversible only in
`permanent`; the cleanup rows keep their order and carry the context and steps; reports
keep each row's own mode.

**Step 2: The dialog**

Props become `questions`, `status` (`running` may carry `progress`; `done` carries a
`BatchReport`), `initialMode`, `onConfirm`, `onClose`. The rules:

- the acknowledgement appears when any ready row of the chosen mode is irreversible. Its
  sentence is today's when all of them are ("I understand that this cannot be undone.")
  and "I understand that N items cannot be undone." otherwise; when only some rows are
  irreversible, those rows carry a "Cannot be undone" mark;
- the wording `clean` titles the dialog "Clean N items?" and labels the button "Clean" /
  "Clean for good", and runs as "Cleaning…" or, with progress, "Cleaning 2 of 5 · logs";
  the wording `delete` keeps every string of today;
- step lines under a row: "Move to the Trash", "Delete" or "Run", then the path or the
  command line; housekeeping in the muted tone;
- the report's headline counts removed rows by their own mode: all Trash is today's
  "Moved N items to the Trash · X", all permanent "Deleted N items · X", and a mix
  "Deleted N and moved M to the Trash · X"; the lists show titles.

Every existing test of the dialog keeps its assertions and renders through
`questionsFromPreview` and `reportFromBatch`; new tests cover the cleanup rules above.
`ExplorerPage` adapts at the call site; its tests do not change.

**Step 3: Commit** — `refactor(desktop): one confirmation dialog for every kind of batch`

---

### Task 15: The Cleanup page

**Files:**

- Create: `src/pages/CleanupPage.tsx`, `src/pages/CleanupPage.test.tsx`,
  `src/components/ItemTable.tsx`, `src/components/ItemTable.test.tsx`,
  `src/components/ItemDetail.tsx`, `src/components/ItemDetail.test.tsx`,
  `src/components/ModuleStrip.tsx`, `src/components/VerdictBadge.tsx`,
  `src/components/SelectionBar.tsx`, `src/hooks/useModules.ts`, `src/lib/facts.ts`,
  `src/lib/selection.ts` (+ tests)
- Modify: `src/pages/ExplorerPage.tsx` (its `SelectionBar` moves to `components/`)

**Step 1: Failing tests**

- `useModules`: refreshes the idle modules on mount; a `modules:state` event refetches
  `['modules']` and `['cleanupItems']`.
- `ItemTable`: rows largest first; the verdict badge; `~` for an estimated size; Space,
  shift-click and the header box, as `NodeTable` behaves; the focused row is reported.
- `ItemDetail`: reasons, facts by type (`formatBytes`, `formatDate`, a count, a flag, a
  path with Reveal), the action choice, the options; a force option in the danger style.
- `CleanupPage`: Keep is filtered out by default; turning a filter off prunes the selection
  to what is visible; the bar sums the ticked `estimated_free`; "Clean…" previews both modes
  and opens the dialog in Trash mode; a confirmed batch clears the selection, invalidates
  the Explorer's `treeNode` and `diskUsage` queries, and the items refetch; a Keep item
  forced in the detail panel goes through; a failed module shows its message and Retry;
  "Discovering…" while a refresh runs.
- `selection.ts`: `defaultChoice(item)`, `prune(selection, visibleIds)`, `requestsOf(selection)`.

**Step 2: Implementation** — as design section 11. The page's deletion state machine
follows the Explorer's `Deletion` (previewing, previewFailed, confirming with a
`BatchStatus`), without the scope/generation part: item ids do not move when the items are
rediscovered.

**Step 3: Commit** — `feat(desktop): the Cleanup page`

---

### Task 16: The sidebar and Activity

**Files:** Modify `src/App.tsx`, `src/App.test.tsx`, `src/components/Sidebar.tsx`,
`src/lib/pages.ts`, `src/pages/ActivityPage.tsx`, `src/pages/ActivityPage.test.tsx`.

**Tests:**

- `App`: Cleanup is enabled when `modules_list` answers the demo, and "soon" when it
  answers none; clicking it opens the page.
- `ActivityPage`: a cleanup line shows its title, "Demo · Remove object" and the command
  line; a line without a path shows its title; an `outsideRoots` cleanup line is worded for
  the home folder.

**Commit** — `feat(desktop): Cleanup in the sidebar, cleanup lines in Activity`

---

### Task 17: End to end

**Files:** Create `apps/desktop/e2e/cleanup.spec.ts`; modify `CLAUDE.md` (the list of
screenshots).

- Open Cleanup: the demo module's strip says 5 items; Keep is hidden.
- Tick `build-cache` and `old.object`, "Clean…": the dialog lists the two steps of the
  folder and `rm` for the object, asks for the acknowledgement in Trash mode because of the
  object, runs with progress, reports; both rows are gone; Activity shows two lines with
  "Demo".
- A Permanent batch of `logs` behind the acknowledgement.
- Show Keep, tick `keepsake`: blocked as kept; turn on its force option in the detail
  panel; it goes through.
- Screenshot `cleanup.png` (the page with the detail panel open) and
  `cleanup-dialog.png`.

**Commit** — `test(desktop): clean the demo module end to end`

---

### Task 18: Documentation and the pull request

**Files:**

- Create: `docs/modules/README.md` — the module authoring guide: the contract, steps and
  effects, what `plan` may not do, `Context::run`, the clippy rule and why, `TestSystem`
  scripts and fixtures, adding the crate and the registry line, the demo as the reference.
- Modify: `CLAUDE.md` (layout: `crates/modules/`, `core/src/module`, `core/src/cleanup`,
  `module_manager.rs`, `cleanup.rs`, the new UI files; the log's new fields in Data; the
  conventions for modules; the cleanup cases in Testing), `README.md` (Cleanup, in debug
  builds only for now), `CONTRIBUTING.md` (the `demo` scope and module ids).

**Manual verification** — the exit criterion, on the real app:

1. `pnpm tauri build --debug --no-bundle`, run the binary from a terminal.
2. Cleanup: the demo module discovers and seeds; clean a folder and an object in Trash
   mode — the folder is in the Trash, the object is gone, Activity has both lines.
3. A Permanent batch; the Keep folder refused, then forced.
4. The Explorer after a scan that includes the sandbox: the cleaned rows are gone.
5. Delete the sandbox from the Explorer; Refresh in Cleanup seeds it again.

**The pull request**

```bash
git push -u origin feat/phase-2b-modules
gh pr create --title "feat: cleanup modules and the Cleanup screen (phase 2b)" --body "..."
```

The body carries the test plan, the manual verification, and the screenshots
`cleanup.png`, `cleanup-dialog.png`, `explorer.png`, `home.png`. Wait for green CI,
self-review the whole diff, squash-merge. release-please then opens `0.5.0`; close and
reopen it so CI runs, check that its diff is versions and changelog only — including the
two new crates — merge it, and check the assets.
