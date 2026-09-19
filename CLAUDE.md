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
  src/disk.rs             volume usage through statvfs
  src/paths.rs            data dir (STORAGE_MONITOR_DATA_DIR), snapshots dir, home dir
crates/modules/<id>/      One crate per cleanup module (from phase 3 on)
crates/cli/               `storage-monitor` binary, thin wrapper over core, JSON output
apps/desktop/src-tauri/   Tauri commands and app state, thin wrapper over core
  src/scan_manager.rs     the one scan per window: worker thread, progress events, snapshot
  src/views.rs            camelCase payloads that cross IPC (ScanStatus, NodeView)
  src/commands.rs         Tauri commands over the manager and core
apps/desktop/src/         React UI. Backend calls only through src/lib/ipc.ts
  lib/                    ipc.ts (typed commands and events), format.ts, nodeErrors.ts, pages.ts
  hooks/                  useScan: the scan state machine fed by scan:progress and scan:done
  pages/                  ExplorerPage and the placeholder of the sections of later phases
  components/             app shell, NodeTable, Treemap (ECharts), Breadcrumbs, ScanProgress
  mocks/                  IPC mock and the /Users/demo fixture (unit tests, e2e, `just dev-web`)
  test/                   Vitest setup, render helper with a QueryClient, ECharts stand-in
apps/desktop/e2e/         Playwright tests against the mocked UI
docs/adr/                 Architecture decision records
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
(defaults: the home folder, `--depth 2`, `--top 20`, `--threshold` 10 MiB).
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

## Workflow

- Never commit to `main`. Branch, open a PR, wait for green CI, squash-merge.
- Conventional commits and PR titles: `feat(core): ...`, `fix(desktop): ...`,
  `docs: ...`, `chore: ...`, `ci: ...`, `test: ...`, `refactor: ...`.
  Full list of types and scopes: `CONTRIBUTING.md`.
  Releases and the changelog are generated from them by release-please.
- TDD: write the failing test first, then the minimal implementation.
- Every module ships with fixtures for external commands (`gh`, `docker`,
  `xcrun`) replayed through the fake `System` (design section 6.3, arrives
  with the module framework in phase 2); tests must pass on Linux.
- UI changes: add or update a Playwright test and attach the `explorer.png`
  (and `home.png`) screenshots that `just e2e` writes under
  `apps/desktop/test-results/` to the PR.

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
- `StatusEmitter::emit` runs while the manager lock is held: an implementation
  must never call back into `ScanManager`.
- Sizes are allocated bytes (`st_blocks * 512`), formatted 1000-based
  (`formatBytes` in the UI, `human_bytes` in the CLI).
- Hard-linked data is attributed to the lexicographically smallest path; the
  other links report 0 bytes.
- CLI output: write through a locked `stdout` and treat `BrokenPipe` as a
  quiet exit, so `storage-monitor ... --json | head` never panics.
- The Content Security Policy in `tauri.conf.json` stays closed: no
  `'unsafe-eval'` or `'unsafe-inline'` in `script-src`, no remote origin in any
  directive. `the_content_security_policy_stays_closed` (`src-tauri/src/lib.rs`)
  holds that and carries the reason for every grant — including why `style-src`
  keeps `'unsafe-inline'`, which is inline style _attributes_ and not Tailwind.
  Loosening a directive means changing that test, deliberately.
- Nothing runs the app under that policy by itself. `just dev` cannot: Tauri
  attaches the header in the `tauri://localhost` handler for the embedded
  frontend, and a `devUrl` document is served by Vite and never passes through
  it — `devCsp` does not help either, it is the same handler. For the dev
  window, set the header in Vite's `server.headers`. To check a change for
  real, `pnpm tauri build --debug --no-bundle` and run the binary, always with
  a positive control: plant an `eval`, confirm it is blocked, and only then
  believe "no violations".
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
- Playwright serves the mock on port 1430 and writes `explorer.png`,
  `explorer-dark.png` and `home.png` under `apps/desktop/test-results/`, which
  it wipes on every run.
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
