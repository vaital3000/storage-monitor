# Storage Monitor: Product and Architecture Design

- Date: 2026-09-17
- Status: Accepted
- Scope: v1 (core analyzer + three modules) and the SDLC around it

## 1. Problem

Disk space on a developer Mac is consumed by artifacts that a generic file
browser cannot explain or safely remove: git worktrees left behind after PRs
merge, Docker images, volumes and build caches, Xcode DerivedData, simulators
and runtimes. Tools like Storage Analyser or DaisyDisk show where the bytes
are, but they do not know what the bytes mean and cannot run the
domain-specific cleanup commands (`git worktree remove`, `docker image rm`,
`xcrun simctl delete`).

Measured on the author's machine on the design date: 22 GB in DerivedData,
12.6 GB of Docker images (9.3 GB reclaimable), 3.5 GB of simulators, and
worktrees spread over five different locations (`.worktrees/`,
`.claude/worktrees/`, `~/.codex/worktrees/`, `/private/tmp/`,
`~/.config/superpowers/worktrees/`), some of them already prunable.

## 2. Goals

- Answer "what can I safely free right now, and why", not only "where is
  the space".
- Modular: a core disk analyzer plus modules that understand specific
  artifact types and their cleanup commands.
- Safe cleanup from the app: plan, preview, confirm, execute, log. Trash by
  default, permanent deletion as an explicit choice.
- Track growth between scans ("what grew since last time").
- OSS-ready: CI-built downloadable app, contributor docs, a repository that
  an AI agent can work in without a human in the loop.

## 3. Non-goals for v1

- Menu bar presence, background monitoring, notifications.
- Full-disk (system volume) analysis. Scan roots default to the home folder.
- Signed and notarized distribution (requires a Developer ID; deferred).
- Homebrew tap (deferred until the first GitHub releases exist).
- Out-of-process plugins. Modules are compiled in.
- Windows and Linux builds.

## 4. Decisions

| Topic | Decision | Rationale |
|---|---|---|
| Stack | Tauri v2: Rust core, React + TypeScript UI | Fast parallel scanner in Rust; rich dashboard (treemap, tables) on the web stack; Rust and frontend tests run on Linux runners; `.app`/`.dmg` from CI; the same core is exposed as a CLI with JSON output. |
| Modules in v1 | git worktrees, Docker, Xcode | Largest offenders on real machines. Dev caches (npm, cargo, brew, ...) go to v2. |
| Deletion modes | Both. Trash is the default, permanent is opt-in | Reversible by default; permanent mode for immediate reclaim. Docker and simulator objects have no Trash, their native commands run in both modes. |
| Distribution | GitHub Releases: ad-hoc signed `.dmg` and CLI binary | No Developer ID today. Notarization and a Homebrew tap are added later without changing the pipeline shape. |
| Workflow | Branch + PR, squash merge, self-merge on green CI | Audit trail and CI gate without blocking on human review time. |
| UI and docs language | English | OSS convention. Conversation with the maintainer stays in Russian. |
| Minimum macOS | 13.3 Ventura, universal binary | Safari 16.4 WebKit, required by Tailwind v4; covers Intel and Apple Silicon. |

## 5. Architecture

Monorepo: a Cargo workspace for Rust and a pnpm workspace for the UI.
The core knows nothing about Tauri; the desktop app and the CLI are thin
adapters over it. Dependency direction is strictly downwards.

```
crates/
  core/               # scanner, data model, snapshots, action engine, Module trait, System port
  modules/
    git-worktrees/
    docker/
    xcode/
  cli/                # storage-monitor scan | modules | clean  (JSON output)
apps/
  desktop/
    src-tauri/        # Tauri v2 commands and events, app state, settings
    src/              # React UI
docs/
  adr/                # architecture decision records
  plans/              # designs and implementation plans
  modules/            # module authoring guide, per-module notes
```

Runtime shape: the desktop app holds the last scan result and module
results in memory behind an async lock. The UI asks for slices ("children
of node X", "items of module Y") through Tauri commands and receives
progress through Tauri events. Nothing large crosses the IPC boundary at
once.

## 6. Core

### 6.1 Scanner

- Parallel directory walk (work-stealing), `lstat` only, symlinks are never
  followed, the walk never crosses volume boundaries (`st_dev`).
- Size is allocated size (`st_blocks * 512`), not logical size, so sparse
  files are counted correctly. Hard links are deduplicated by
  `(st_dev, st_ino)` when `st_nlink > 1`. APFS clones cannot be detected
  cheaply and are counted in full, same as `du`.
- Result is an arena of nodes (`Vec<Node>` with parent and children
  indices), each node carrying name, kind, allocated bytes, logical bytes,
  mtime, and a subtree file count. Directory sizes are aggregated bottom-up.
- Progress events: files seen, bytes so far, current directory. Scans are
  cancellable. Permission errors are recorded per node, not fatal (some
  `~/Library` folders need Full Disk Access; the UI explains that).
- Scan roots and exclusion globs are settings. Default root is the home
  folder.

### 6.2 Snapshots and deltas

After a scan, a snapshot is persisted to
`~/Library/Application Support/storage-monitor/snapshots/`: all directory
nodes plus files above a threshold (default 10 MB), serialized compactly and
compressed. The newest snapshot is compared with the previous one by path to
produce per-directory deltas ("top growers"). The store keeps the last N
snapshots (default 10). Full rescans only in v1; incremental updates via
FSEvents are in the backlog.

### 6.3 System port

All access to the outside world goes through one trait, `System`:
filesystem metadata and Trash/delete operations, process execution
(`git`, `gh`, `docker`, `xcrun`), and the clock. Commands are always argv
arrays, never shell strings. Tests use a fake `System` with a temp
filesystem and scripted command outputs, so module tests run on Linux
runners without Docker or Xcode installed.

### 6.4 CLI

`storage-monitor scan [--root PATH]... [--depth N] --json`,
`storage-monitor modules list`, `storage-monitor modules run <id> --json`,
`storage-monitor clean --plan plan.json --mode trash|permanent [--yes]`.
The CLI exists for scripts, for agents in CI, and to reproduce bugs without
the UI.

## 7. Module contract

```rust
pub trait Module: Send + Sync {
    fn descriptor(&self) -> ModuleDescriptor;        // id, name, description, required tools
    fn presentation(&self) -> Presentation;          // declarative UI schema (section 7.2)
    async fn availability(&self, sys: &dyn System) -> Availability; // Available | Unavailable { reason }
    async fn discover(&self, ctx: &DiscoverContext) -> Result<Vec<Item>>;
    async fn execute(&self, req: &ActionRequest, sys: &dyn System) -> ActionResult;
}
```

`DiscoverContext` carries the scan roots, module settings (thresholds),
an optional reference to the last scan tree (so modules can reuse sizes and
marker directories such as `.git` instead of walking again), and the
`System` port.

### 7.1 Item model

```rust
pub struct Item {
    pub id: String,                       // "<module>:<native id>", stable across runs
    pub module: ModuleId,
    pub kind: String,                     // module-defined: "worktree", "image", "simulator", ...
    pub title: String,
    pub subtitle: Option<String>,
    pub path: Option<PathBuf>,
    pub size: Size,                       // bytes + confidence (Exact | Estimated)
    pub created_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub verdict: Verdict,                 // level: Safe | Review | Keep, reasons: Vec<Reason>
    pub facts: Vec<Fact>,                 // key, label, typed value; shown in the detail panel
    pub links: Vec<Link>,                 // PR URL, "reveal in Finder", ...
    pub actions: Vec<ActionSpec>,
}

pub struct ActionSpec {
    pub id: String,                       // "remove-worktree"
    pub label: String,
    pub danger: Danger,                   // Low | Medium | High
    pub supports_trash: bool,             // false for Docker and simulator objects
    pub reversible: bool,
    pub preview: Vec<PreviewLine>,        // exact commands or paths shown before confirmation
    pub estimated_free: u64,
    pub options: Vec<ActionOption>,       // "also delete local branch", "force", ...
}
```

Verdict levels: **Safe** (the module is confident the object can go),
**Review** (probably stale, a human should glance), **Keep** (do not offer
by default; still deletable with an explicit force option where it makes
sense). Every verdict carries human-readable reasons.

### 7.2 Module UI: two extension layers

**Layer 1, declarative.** `Presentation` describes how a module's items
should be shown: extra table columns bound to fact keys with a display kind
(text, bytes, date, badge, link), sections (Docker: images, containers,
volumes, build cache), an optional group-by fact (worktrees grouped by
repository), and overview widgets (a number, a breakdown). A generic
renderer in the UI draws this without module-specific code. All v1 modules
use only this layer. Future out-of-process plugins would be limited to this
layer, which keeps them safe.

**Layer 2, code.** The UI keeps a registry keyed by module id with optional
components: detail panel, row expansion, page header, overview widget. The
generic module page consults the registry first and falls back to the
declarative rendering. Adding a custom UI for a built-in module is one
folder under `apps/desktop/src/modules/<id>/`; the core does not change.

## 8. Modules

### 8.1 git-worktrees

Discovery in two passes. Pass one walks the scan roots looking for `.git`
entries: a directory marks a main repository, a file marks a linked
worktree (its `gitdir:` pointer is read). Pass two runs
`git worktree list --porcelain` in every main repository; this catches
worktrees outside the roots (`/private/tmp`, `~/.codex/worktrees`). Results
are merged by canonical path. Entries whose directory no longer exists are
marked `prunable`; `.git` files whose main repository is gone are marked
`orphaned`.

Facts per worktree: repository, branch or detached HEAD, HEAD commit date,
dirty state (modified and untracked counts), ahead/behind upstream, whether
HEAD is reachable from the default branch, allocated size, and the pull
request found by `gh pr list --head <branch> --state all` (number, state,
merged date, URL). PR lookups are cached and rate-limited. If the remote is
not GitHub or the repository cannot be resolved, the PR fact is "unknown",
never an error.

Verdict rules (age threshold is a setting, default 30 days):

- Keep: uncommitted changes, or unpushed commits not covered by a merged PR,
  or an open PR.
- Safe: prunable or orphaned; clean and PR merged; clean and branch already
  contained in the default branch; clean detached HEAD reachable from the
  default branch.
- Review: clean, no PR, last commit older than the threshold; clean and PR
  closed without merge.

Actions: "Remove worktree". Trash mode moves the directory to the Trash and
runs `git worktree prune`; permanent mode runs `git worktree remove
--force`. Option: also delete the local branch (`git branch -D`). "Prune
stale entries" runs `git worktree prune` per repository.

### 8.2 docker

Runtime detection through the active Docker context (Docker Desktop,
OrbStack, Colima) so the host-side footprint (the VM disk file) can be
shown next to engine-side numbers when its location is known. Data comes
from `docker system df -v --format json`, which provides images (containers
count, created, size, shared and unique size), containers (state, status,
created, size), volumes (links count, size) and build cache entries (in
use, last used, usage count, shared, size). Images have no last-used
timestamp in the engine; the module approximates it from `LastTagTime` and
the creation time of containers using the image, and says so.

Verdict rules (thresholds are settings):

- Image: dangling → Safe; no containers and older than 30 days → Review;
  in use → Keep.
- Container: running → Keep; exited older than 7 days → Review.
- Volume: no links → Review (never Safe: volumes hold databases).
- Build cache: not in use → Safe.

Actions: `docker image rm`, `docker container rm`, `docker volume rm`,
`docker builder prune`, plus `docker image prune` / `docker system prune`
as module-level actions. No Trash mode; the confirmation dialog states that
these cannot be undone.

### 8.3 xcode

Items: DerivedData per project (name from `info.plist` → `WorkspacePath`,
plus the shared `ModuleCache.noindex` and `CompilationCache.noindex`),
simulator devices from `xcrun simctl list devices -j` (`dataPathSize`,
`isAvailable`, `state`, `lastBootedAt` when present), simulator runtimes
from `xcrun simctl runtime list -j` (`sizeBytes`, `lastUsedAt`,
`deletable`), archives under `~/Library/Developer/Xcode/Archives`, device
support folders (`iOS DeviceSupport`, `watchOS DeviceSupport`, ...),
preview caches and `CoreSimulator/Caches`.

Verdict rules:

- DerivedData project folder not modified for 14 days → Safe, otherwise
  Review. Shared caches → Review.
- Simulator device unavailable → Safe; not booted for 60 days → Review;
  booted → Keep.
- Runtime with no devices and not used for 60 days → Review.
- Archive older than 90 days → Review. Device support for versions older
  than the newest installed → Review.

Actions: Trash or delete folders (DerivedData, archives, device support,
caches); `xcrun simctl delete <udid>`, `xcrun simctl delete unavailable`,
`xcrun simctl runtime delete <id>`.

## 9. Action engine

Every cleanup passes four stages:

1. **Plan**: selected `(item, action, options)` entries plus the mode
   (Trash or Permanent).
2. **Preview**: the exact commands or paths per entry, the total estimated
   bytes, and warnings (for example "this worktree has uncommitted changes,
   force is enabled").
3. **Confirm**: one dialog per batch listing everything above. Dangerous
   entries are highlighted and require their explicit option.
4. **Execute**: sequential, with progress events. Each entry is re-validated
   right before execution (the object still exists and has the same kind).
   Failures do not abort the batch unless the user chose so.

Every entry is appended to an action log
(`~/Library/Application Support/storage-monitor/actions.jsonl`): timestamp,
module, item, action, mode, command, result, bytes freed. Trash entries link
to "show in Trash".

Safety guards, enforced in the core regardless of module:

- Paths are canonicalized; anything outside the allowed roots (home folder
  and user-added roots) or inside a denylist (`/`, `/System`, `/usr`,
  `/Library`, `~/Library` except paths a module declares as managed) is
  refused.
- No shell: commands are argv arrays.
- Symlinks are never followed when deleting.
- The permanent mode is a per-run choice and a setting; the default is
  Trash.

## 10. UI

Screens:

- **Overview**: disk used and free, "reclaimable now" with a per-module
  breakdown, top growers since the previous snapshot, scan status and
  timestamp, "Clean everything safe".
- **Explorer**: treemap with drill-down plus a directory table with
  breadcrumbs, size, delta versus the previous snapshot, "Reveal in Finder",
  "Move to Trash" for arbitrary folders.
- **Cleanup**: one table of candidates across modules with filters by verdict
  and module, multi-select, a detail panel with facts, verdict reasons and
  actions, batch actions with the confirmation flow.
- **Module pages**: the generic renderer driven by the module's
  `Presentation`, or a registered custom component.
- **Activity**: the action log. **Settings**: roots, exclusions, per-module
  thresholds, deletion mode, PR lookup on/off.

Stack: React, TypeScript, Vite, Tailwind, Radix primitives, TanStack Table
and Query, ECharts behind a local `Treemap` component so it can be swapped.
Light and dark themes, system font, transparent title bar for a native
feel. English UI.

## 11. Testing strategy

- Rust unit tests for the data model, verdict rules and the action engine
  (pure functions, no I/O).
- Scanner integration tests on generated temp trees (hard links, sparse
  files, symlink loops, permission errors).
- Worktree module tests on real git repositories created in temp dirs
  (git is available on all CI runners).
- `gh`, `docker` and `simctl` outputs are recorded from a real machine into
  fixtures (`just fixtures record`, with paths anonymized) and replayed
  through the fake `System`.
- Frontend: vitest + Testing Library for components; Playwright against the
  Vite dev server with mocked IPC (`@tauri-apps/api/mocks`) and fixture
  data, so every screen is reproducible and screenshots are attached to
  PRs.
- The desktop crate has smoke tests for commands; the full app is built on a
  macOS runner in CI.

## 12. CI and release

- `ci.yml` on every PR: `cargo fmt --check`, `cargo clippy -D warnings`,
  `cargo test` (ubuntu); `pnpm typecheck`, `pnpm lint`, `pnpm test`,
  Playwright (ubuntu); `tauri build` smoke on macOS.
- `release.yml` on every push to `main`: release-please maintains the release
  PR; merging it creates the tag and the GitHub Release, and a job chained in
  the same run builds the universal `.dmg` and the CLI, then uploads them with
  `gh release upload`. No personal access token is needed. The release PR does
  not trigger CI (it is opened by the workflow token); its diff is versions and
  changelog only.
- Versioning and changelog via release-please from conventional commits;
  PR titles are linted; merges are squash-only. Dependabot for cargo, npm
  and actions. release-please runs the `simple` strategy with `extra-files`
  for every version location, because its Rust strategy cannot handle a
  virtual workspace root.
- Ad-hoc code signing so the app runs on Apple Silicon; README documents
  the Gatekeeper right-click step until notarization exists.

## 13. AI-native SDLC

The repository must be operable by an agent without a human explaining it:

- `CLAUDE.md` (with `AGENTS.md` pointing at it): architecture map, `just`
  commands, conventions, Definition of Done, how to add a module, how to
  record fixtures.
- `docs/adr/` for decisions, `docs/plans/` for designs and implementation
  plans, `docs/modules/` for the module authoring guide.
- Issue templates (bug, module request), a PR template with a checklist,
  `CONTRIBUTING.md`, `SECURITY.md`, `CODEOWNERS`.
- Feature flow: spec → plan → TDD implementation → PR → green CI → squash
  merge. Small PRs, one concern each.
- Optional: Claude GitHub Action for PR review and `@claude` on issues
  (needs an API key secret; decided in phase 0).

## 14. Roadmap

| Phase | Deliverable | Exit criteria |
|---|---|---|
| 0 | Walking skeleton: workspace, empty Tauri window, CI, release pipeline, repo docs | `v0.1.0` release with a downloadable `.dmg` and CLI built by CI |
| 1 | Scanner, snapshots, CLI `scan`, Explorer screen | Scan of the home folder with treemap and deltas. Done 2026-09-18 (v0.2.0). |
| 2 | Module framework, action engine, Cleanup / Activity / Settings screens | A dummy module can be cleaned end-to-end in both modes |
| 3 | git-worktrees module | Verdicts match the rules on the author's machine; PR lookup works |
| 4 | docker module | Works with Docker Desktop and OrbStack |
| 5 | xcode module | DerivedData, simulators, runtimes, archives |
| 6 | Overview with deltas, polish, README with screenshots | `v1.0.0` |

## 15. Backlog (v2)

Menu bar and background monitoring with notifications; dev caches module
(npm, pnpm, yarn, pip, cargo, go, gradle, brew, CocoaPods, `node_modules`
and `target` folders in projects); Homebrew tap; notarization; incremental
scans via FSEvents; out-of-process plugins over the declarative layer;
Time Machine local snapshots and purgeable space.

## 16. Assumptions

- The product keeps the name "Storage Monitor" for now; renaming before
  `v1.0.0` is cheap.
- The maintainer's GitHub account (`gh`) is authenticated; PR lookups run
  only when `gh` is present and authenticated.
- Scanning parts of `~/Library` may require Full Disk Access; the app
  explains the permission instead of prompting for it automatically.
