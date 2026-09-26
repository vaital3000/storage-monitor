# Storage Monitor: guide for coding agents

Read this before changing anything. `AGENTS.md` is a symlink to this file.

## What this is

A macOS disk space analyzer (Tauri 2, Rust core, React UI) with cleanup
modules for developer artifacts. The full design is in
`docs/plans/2026-09-17-storage-monitor-design.md`; decisions are in
`docs/adr/`. Implementation plans live in `docs/plans/`.

## Layout

```
crates/core/              Rust library: all logic lives here (scanner, snapshots, modules, actions)
  src/scan/               parallel walker (walker.rs), arena tree (tree.rs), live counters (progress.rs)
  src/snapshot/           persisted snapshots: model.rs (format), store.rs (files), delta.rs (growers)
  src/action/             deleting: model.rs (Plan to Outcome), guards.rs (Limits::check),
                          engine.rs (preview, execute; inspect and delete, shared with cleanup),
                          log.rs (the actions.jsonl record)
  src/module/             the module contract (ADR 0008): Module, Context, items with verdicts
                          and facts (item.rs), steps and their effects (step.rs)
  src/cleanup/            cleaning items: preview and execute over a module's steps, through
                          action's guards; model.rs is the wire, engine.rs the rules
  src/system/             the System port: every deletion, running a program, the clock
                          (real.rs, process.rs, test.rs)
  src/disk.rs             volume usage through statvfs
  src/paths.rs            data dir (STORAGE_MONITOR_DATA_DIR), snapshots dir, actions log, home dir
crates/modules/           Cleanup modules; clippy.toml refuses the std calls that delete or spawn
  demo/                   the demo module: a sandbox in the data dir, debug builds only
  registry/               storage-monitor-modules: the one list the CLI and the desktop read
  <id>/                   one crate per module (docs/modules/README.md)
crates/cli/               `storage-monitor` binary, thin wrapper over core, JSON output
                          (scan_cmd.rs, modules_cmd.rs)
apps/desktop/src-tauri/   Tauri commands and app state, thin wrapper over core
  src/scan_manager.rs     the one scan per window: worker thread, progress events, snapshot
  src/actions.rs          a batch end to end: re-plan, execute, record, patch the tree
  src/module_manager.rs   the modules: a discovery per module on a thread, modules:state
                          events, the items of each module's last discovery that worked
  src/cleanup.rs          a cleanup batch end to end: re-plan, execute with cleanup:progress,
                          record, forget, patch the tree, rediscover
  src/views.rs            camelCase payloads that cross IPC (ScanStatus, NodeView)
  src/commands.rs         Tauri commands over the manager and core
apps/desktop/src/         React UI. Backend calls only through src/lib/ipc.ts
  lib/                    ipc.ts (typed commands and events), format.ts, nodeErrors.ts,
                          blockReasons.ts (a guard verdict in words), pages.ts,
                          batchQuestion.ts (what the dialog asks and reports, for both screens),
                          selection.ts, facts.ts, items.ts, commandLine.ts (Cleanup's helpers)
  hooks/                  useScan: the scan state machine fed by scan:progress and scan:done;
                          useModules: the modules and their items, kept by modules:state
  pages/                  ExplorerPage, CleanupPage, ActivityPage and the placeholder of the
                          later phases
  components/             app shell, NodeTable, Treemap (ECharts), Breadcrumbs, ScanProgress,
                          ItemTable, ItemDetail, ModuleStrip, VerdictBadge,
                          ConfirmDeleteDialog (the one door to a deletion, for both screens)
  mocks/                  IPC mock and the /Users/demo fixture (unit tests, e2e, `just dev-web`)
    ipc.ts                the commands; fixtures.ts the scanned tree
    actions.ts            mirrors core's action/{guards,engine}.rs; actionLog.ts its log.rs
    modules.ts            the demo module's items and the module manager; cleanup.ts mirrors
                          core's cleanup/ and the desktop's cleanup.rs
  test/                   Vitest setup, render helper with a QueryClient, ECharts stand-in
apps/desktop/e2e/         Playwright tests against the mocked UI
  fixtures.ts             the `test` every spec imports: it fails on a CSP violation
docs/adr/                 Architecture decision records
docs/modules/             The module authoring guide
docs/plans/               Designs and implementation plans
docs/images/              Screenshots embedded in README.md
```

Dependency direction: `core` ← `modules` ← `cli` / `desktop`. Core never
imports Tauri. The UI never calls `invoke` outside `src/lib/ipc.ts`.

## Commands

Prerequisites: Rust stable (see `rust-toolchain.toml`), Node 22+, pnpm 10, `just` (`brew install just`).

```
just setup      install JS deps and the Playwright browser
just dev        run the desktop app
just dev-web    run the UI in a browser with mocked IPC (fastest UI loop): the Explorer over the fixture
just test       cargo test + vitest
just e2e        Playwright against the mocked UI
just lint       fmt --check, clippy -D warnings, tsc, eslint, prettier --check
just fmt        apply formatters
just ci         lint + test + build-web + e2e; CI adds a macOS `tauri build` smoke
```

CI runs clippy on the latest stable Rust. If the local toolchain is older (Homebrew
Rust ignores `rust-toolchain.toml`), run `just clippy-ci` (Docker) before pushing.

CLI: `cargo run -q -p storage-monitor-cli -- scan [ROOT] --json [--save]`
(defaults: the home folder, `--depth 2`, `--top 20`, `--threshold` 10 MiB);
`... -- modules list [--json]` and `... -- modules run <id> [--json]` (the demo module in
a debug build; a release build of phase 2b ships none).
Rust tests for a single crate: `cargo test -p storage-monitor-core`.
One vitest file: `pnpm --filter @storage-monitor/desktop test src/App.test.tsx`.

## Data

Snapshots live in `~/Library/Application Support/storage-monitor/snapshots/`;
`STORAGE_MONITOR_DATA_DIR` overrides the data dir (tests set it). A completed scan
writes `<stamp>.snap` (postcard + lz4 payload) and `<stamp>.json` (sidecar for
listing): every directory plus files of 10 MiB or more, keyed by absolute path.
The last 10 snapshots are kept across all roots; `SnapshotStore::latest_for(root)`
pairs a scan with the previous snapshot of the same root for the deltas. The
desktop app saves a snapshot after every completed scan, the CLI on `scan --save`;
both use the same store. Cancelled scans are not persisted. Format details:
`docs/adr/0004-snapshot-format.md`.

Deletions are recorded in `actions.jsonl`, beside `snapshots/` in the same data
dir (so `STORAGE_MONITOR_DATA_DIR` moves it too): one JSON object per line, one
line per entry of a batch, newest at the end. The file is created `0o600` —
it names every path the user has ever deleted — and only on creation, so a mode
someone set by hand is kept. A batch is one append: all its lines in a single
`write_all` into an `O_APPEND` handle, so two writers cannot interleave, and a
batch with no entries writes nothing at all. `Ok` from `ActionLog::append` means
the bytes reached the volume (`sync_all`); `Err` means the caller has deleted
something it has not recorded, which is why `action_run` reports it instead of
swallowing it. A write cut short costs its own line and nothing more:
`ActionLog::tail` counts it in `damaged` rather than failing the read, and the
next `append` starts a fresh line. Unlike snapshots, which are pruned to ten,
**nothing prunes this file** — it is the record, and it grows by one line per
entry the app was asked to delete; `tail` reads all of it to return the end.
A cleanup batch writes the same lines with two fields more — `source` (the module
and item ids, the item's title, the action's label) and `commands` (the argv of
every command that started) — and in the mode each entry really left in, which is
`permanent` for anything the Trash cannot undo. `path` and `kind` are optional: a
cleanup request whose item was already gone has neither. Every field added since
2a is `#[serde(default)]` and left out when empty, so an Explorer line keeps its
exact bytes and every old line reads.

## Workflow

- Never commit to `main`. Branch, open a PR, wait for green CI, squash-merge.
- Conventional commits and PR titles: `feat(core): ...`, `fix(desktop): ...`,
  `docs: ...`, `chore: ...`, `ci: ...`, `test: ...`, `refactor: ...`.
  Full list of types and scopes: `CONTRIBUTING.md`.
  Releases and the changelog are generated from them by release-please.
- TDD: write the failing test first, then the minimal implementation.
- Every module ships with fixtures for external commands (`gh`, `docker`,
  `xcrun`) replayed through the fake `System` (`TestSystem::script` and
  `TestSystem::replay`); tests must pass on Linux. How to write one:
  `docs/modules/README.md`.
- UI changes: add or update a Playwright test and attach the `explorer.png`
  (and `home.png`, and `cleanup.png` for the Cleanup screen) screenshots that
  `just e2e` writes under `apps/desktop/test-results/` to the PR.

## Definition of Done

- `just ci` passes locally and in CI.
- New behavior has tests; verdict rules and action safety have unit tests.
- Docs updated: the module guide (`docs/modules/`, from phase 2) for new modules,
  an ADR for a changed decision, `CLAUDE.md` for new commands or layout changes.
- No shell strings: external commands are argv arrays through `System`.
- Deletion code paths respect the allowed roots and the Trash-by-default rule.

## Conventions

- Rust 2024 edition, `clippy -D warnings`, `rustfmt` defaults.
- TypeScript strict, ESLint + Prettier, Tailwind for styling, no CSS modules.
- `tsc` checks only `src/`. Vite, Vitest and Playwright config files and `e2e/` are validated by running them, not by the type checker.
- English for code, docs, UI strings and commit messages.
- IPC payloads: Rust structs that cross IPC carry
  `#[serde(rename_all = "camelCase")]`; TypeScript interfaces in
  `src/lib/ipc.ts` mirror them in camelCase. Every new command gets a handler
  in `src/mocks/ipc.ts`.
- Tree queries are keyed by the scan generation (`useScan().generation`) with
  `staleTime: Infinity`: a rescan refetches, an older result stays cached until then.
  The Activity query (`['activity']`) is a deliberate exception — `staleTime: 0`,
  so every open re-reads the action log — and nothing invalidates it after a batch,
  because the shell renders one page at a time and that screen is unmounted whenever a
  deletion runs. A screen that deletes _while_ Activity is mounted is what would change
  that; `ExplorerPage`'s `afterBatch` and the page's own `useQuery` both say so. The
  modules' two queries (`['modules']`, `['cleanupItems']`) are the other exception, for
  the same reason: their `modules:state` events are heard only while Cleanup is mounted.
  A cleanup batch marks the Explorer's `['treeNode']` and `['diskUsage']` stale, since its
  splice moved every id in them.
- `StatusEmitter::emit` runs while the manager lock is held: an implementation
  must never call back into `ScanManager`.
- Sizes are allocated bytes (`st_blocks * 512`), formatted 1000-based
  (`formatBytes` in the UI, `human_bytes` in the CLI).
- Hard-linked data is attributed to the lexicographically smallest path; the
  other links report 0 bytes.
- Deleting goes through the `System` port (`crates/core/src/system/`), never
  through `std::fs`: `move_to_trash`, `remove`, `symlink_metadata`, `locate`, `run`
  and the clock are all on it, so tests run against `TestSystem` — a temp tree, a Trash that is
  another folder, a clock a test moves — on any platform, and a deletion outside
  that tree panics instead of wiping a developer's `$HOME`. The port takes
  absolute paths in normal form only and refuses anything else untouched, because
  the kernel reads the path as written: `remove("/a/b/..")` empties `/a/b` and
  `remove("/a/link/")` deletes what the link points at. Two things under
  `action/` do reach `std::fs` on purpose, and neither deletes: `ActionLog`
  writes the record, and `guards::judged_form` resolves a path to decide the
  verdict on it.
- `System::run` takes an absolute program in normal form and an argv — never a shell
  string — closes stdin, caps each stream at 32 MiB and kills the whole process group at
  its deadline. `TestSystem` runs nothing: a test installs tools and scripts each argv, and
  a run nobody scripted panics.
- Modules describe, the core acts (ADR 0008): a module finds and judges items and plans
  steps — `Delete` a path, or `Run` a tool whose effect it declares — and never deletes or
  spawns anything itself; `crates/modules/clippy.toml` refuses the std calls that would.
  Every path a step deletes passes `Limits::check` (the home folder is the only allowed
  root until phase 2c), a `Removes` target must be spelled the way the guards resolve it,
  `plan` takes no `System`, and a Keep item is refused unless a `force` option is on.
  `cleanup_run` takes the requests and plans again, never a preview: a preview carries
  commands.
- CLI output: write through a locked `stdout` and treat `BrokenPipe` as a
  quiet exit, so `storage-monitor ... --json | head` never panics.
- The Content Security Policy in `tauri.conf.json` stays closed: no
  `'unsafe-eval'` or `'unsafe-inline'` in `script-src`, no source anywhere that
  is not a keyword, Tauri's `ipc:` or a `localhost` host, and no directive
  beyond the ten that are there — `script-src-elem` replaces `script-src` for
  `<script>` elements rather than adding to it, so an eleventh directive can
  defeat the tenth. `the_content_security_policy_stays_closed`
  (`src-tauri/src/lib.rs`) holds all three, and the reasoning is in
  `docs/adr/0006-content-security-policy.md`. Loosening the policy means
  changing that test, deliberately.
- `tauri.conf.json` takes no comments: it is plain JSON, and an extra key —
  `"_csp"` beside `"csp"`, say — makes `tauri-build` reject the config outright,
  so the config cannot carry its own reasons. That is why they live in the ADR
  and, for the three that are not guessable from the line, in the test.
- Nothing runs the app under that policy by itself. `just dev` cannot: Tauri
  attaches the header in the `tauri://localhost` handler for the embedded
  frontend, and a `devUrl` document is served by Vite and never passes through
  it — `devCsp` does not help either, it is the same handler. The nearest
  substitute is `just e2e`, which serves the same header from the mock server
  (see Testing). To check a change for real, run the binary that
  `pnpm tauri build --debug --no-bundle` leaves behind, always with a positive
  control: plant an `eval`, confirm it is blocked, and only then believe "no
  violations".
- Do not add dependencies for something the standard library or an existing
  dependency already does.

## Testing

- Mock mode: `vite --mode mock` loads `.env.mock` (`VITE_MOCK_IPC=1`) and
  `main.tsx` calls `installIpcMock()`, which answers every command from the
  fixture rooted at `/Users/demo` and simulates a scan through the same events as
  the real manager. Unit tests call `installIpcMock()` themselves and run the
  simulated scan with `setMockScanDelay(0)` (`src/test/setup.ts`); Playwright
  keeps the browser pace and reaches the mock through
  `window.__STORAGE_MONITOR_MOCK__`.
- Vitest runs in a pinned zone (`env: { TZ: 'America/New_York' }` in
  `vitest.config.ts`), not in the machine's. The app formats instants into local
  time, CI runners are UTC, and in UTC `getUTCHours` for `getHours` is the same
  function — so the tests that exist for that mix-up cannot see it there.
  `the_suite_runs_outside_utc` in `src/lib/format.test.ts` fails if the pin goes.
- Playwright serves the mock on port 1430 and writes `explorer.png`,
  `explorer-dark.png`, `explorer-selection.png`, `cleanup.png`, `cleanup-dialog.png`,
  `activity.png` and `home.png` under `apps/desktop/test-results/`, which it wipes on
  every run.
- That server is the mock one, sealed (`STORAGE_MONITOR_E2E=1`, set by
  `playwright.config.ts`): it serves the window's Content Security Policy read
  from `tauri.conf.json` itself, and every spec runs under it through the `test`
  of `e2e/fixtures.ts`, which fails on any `securitypolicyviolation`. Fast
  refresh is off there because its preamble is an inline script the policy
  blocks — the page then renders nothing, which is also what `just dev` would do
  if the header were set on the ordinary dev server. `e2e/csp.spec.ts` is the
  positive control: it checks the header arrived, and plants four violations
  through the page's own loader — an inline `<script>`, a `blob:` worker, a
  `data:` font and a `setTimeout` handed a **string**, which is the eval class.
  Never a bare `eval('…')` inside `page.evaluate`: that compiles through the
  debugger, which the policy does not cover, so it runs and proves nothing.
  A third test, marked `test.fail()`, plants a violation and does not declare
  it — the only thing that can fail it is the watcher itself, which is how the
  alarm is kept honest. Deleting the teardown assertion or making the fixture
  non-`auto` leaves every other spec green.
- The fixture's ids move when a batch does. `patchTree(gone, touched)` in
  `src/mocks/fixtures.ts` is the only way to change that tree: it drops the
  subtrees that went and then re-numbers what is left, breadth first and
  siblings largest first — `patch_paths` and `install_patches` in one, so the
  two halves cannot come apart. `touched` is every entry **removed or failed**,
  the predicate `ExplorerPage`'s `afterBatch` re-anchors on, so a batch that
  only failed still moves the arena. The consequences a UI test leans on:
  `fixtureNodes` is a live binding, a node read before a batch is a snapshot of
  the old arena, and `fixtureNodeView(id)` answers for whichever node holds that
  slot now rather than throwing. `fixtures.test.ts` checks the numbering
  invariants over both arenas — the built one and the patched one — because a
  renumbering that sorted by the wrong field passed every other test in the
  suite.
- The deletion guards are pinned by cases both implementations answer:
  `crates/core/tests/fixtures/guard-cases.json`, run by
  `the_shared_guard_cases_hold` in `crates/core/src/action/guards.rs` over a temp
  tree and by `apps/desktop/src/mocks/actions.test.ts` over the fixture. Changing
  a placement rule means changing both sides, which is the point: the mock is the
  oracle every UI test of the deletion is written against. The file covers the six
  numbered rules of `Limits::check` and the `drop_nested` tie-break — everything
  the guards decide without a disk — and stops where a `System` would be needed:
  the stat behind `missing`, and `kindChanged`.
- The cleanup engine's rules that need no disk — a missing item, an unknown action or
  option, `kept` and its force option, and which plans the Trash can undo — are pinned the
  same way: `crates/core/tests/fixtures/cleanup-cases.json`, run by
  `the_shared_cleanup_cases_hold` in `crates/core/src/cleanup/engine.rs` and by
  `apps/desktop/src/mocks/cleanup.test.ts`. The mock's demo module reports the items the
  Rust one does, sizes on APFS included, so a screen over the mock is the app's screen.
- Rust walker tests (`crates/core/tests/walker.rs`) build temp trees: hard
  links, sparse files, permission-denied and partially readable directories,
  symlinks, deep nesting.
- Desktop manager tests use `ScanManager::with_snapshots_dir(tempdir)` and the
  CLI tests set `STORAGE_MONITOR_DATA_DIR`, so no test touches the real store.

## Release

Every push to `main` runs release-please, which keeps a `chore(main): release X.Y.Z`
PR up to date. Merging that PR creates the tag `vX.Y.Z` and the GitHub Release; a
chained job in `release.yml` then builds the universal `.dmg` and the CLI tarball and
attaches them. Versions are bumped by that PR in every crate's `Cargo.toml`,
`Cargo.lock` and both `package.json` files; never edit them by hand. In
`release-please-config.json` the TOML rules compare `@.name.value` (the parser
wraps scalars); a rule that matches nothing only warns, so keep that form.

The release PR is opened by the workflow token, so GitHub does not run CI on it. Review
its diff (version bumps and `CHANGELOG.md` only); to force a CI run, close and reopen
the PR. `workflow_dispatch` on `release.yml` rebuilds the assets of an existing tag.

release-please opens its PR with the workflow token, so the repository setting
"Allow GitHub Actions to create and approve pull requests" (Settings > Actions >
General) must stay enabled; without it the `Release` run fails at that step.
