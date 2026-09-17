# Storage Monitor: guide for coding agents

Read this before changing anything. `AGENTS.md` is a symlink to this file.

## What this is

A macOS disk space analyzer (Tauri 2, Rust core, React UI) with cleanup
modules for developer artifacts. The full design is in
`docs/plans/2026-09-17-storage-monitor-design.md`; decisions are in
`docs/adr/`. Implementation plans live in `docs/plans/`.

## Layout

```
crates/core/              Rust library: all logic lives here (scanner, modules, actions)
crates/modules/<id>/      One crate per cleanup module (from phase 3 on)
crates/cli/               `storage-monitor` binary, thin wrapper over core, JSON output
apps/desktop/src-tauri/   Tauri commands and app state, thin wrapper over core
apps/desktop/src/         React UI. Backend calls only through src/lib/ipc.ts
apps/desktop/src/mocks/   IPC mocks used by unit tests, e2e and `just dev-web`
apps/desktop/e2e/         Playwright tests against the mocked UI
docs/adr/                 Architecture decision records
docs/plans/               Designs and implementation plans
```

Dependency direction: `core` ← `modules` ← `cli` / `desktop`. Core never
imports Tauri. The UI never calls `invoke` outside `src/lib/ipc.ts`.

## Commands

Prerequisites: Rust stable (see `rust-toolchain.toml`), Node 22+, pnpm 10, `just` (`brew install just`).

```
just setup      install JS deps and the Playwright browser
just dev        run the desktop app
just dev-web    run the UI in a browser with mocked IPC (fastest UI loop)
just test       cargo test + vitest
just e2e        Playwright against the mocked UI
just lint       fmt --check, clippy -D warnings, tsc, eslint, prettier --check
just fmt        apply formatters
just ci         lint + test + build-web + e2e; CI adds a macOS `tauri build` smoke
```

Rust tests for a single crate: `cargo test -p storage-monitor-core`.
One vitest file: `pnpm --filter @storage-monitor/desktop test src/App.test.tsx`.

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
- UI changes: add or update a Playwright test and attach the `home.png`
  screenshot that `just e2e` writes under `apps/desktop/test-results/` to the PR.

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
- CLI output: write through a locked `stdout` and treat `BrokenPipe` as a
  quiet exit, so `storage-monitor ... --json | head` never panics.
- Before the first destructive command ships, set a Content Security Policy
  in `tauri.conf.json` (it is `null` in phase 0).
- Do not add dependencies for something the standard library or an existing
  dependency already does.

## Release

Every push to `main` runs release-please, which keeps a `chore(main): release X.Y.Z`
PR up to date. Merging that PR creates the tag `vX.Y.Z` and the GitHub Release; a
chained job in `release.yml` then builds the universal `.dmg` and the CLI tarball and
attaches them. Versions are bumped by that PR in every crate's `Cargo.toml`,
`Cargo.lock` and both `package.json` files; never edit them by hand.

The release PR is opened by the workflow token, so GitHub does not run CI on it. Review
its diff (version bumps and `CHANGELOG.md` only); to force a CI run, close and reopen
the PR. `workflow_dispatch` on `release.yml` rebuilds the assets of an existing tag.
