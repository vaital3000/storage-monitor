# Phase 2a: Deleting From the App Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Ship deletion from the Explorer — multi-select, a confirmation dialog with both Trash and Permanent modes, a patched tree, and an action log with its own screen.

**Architecture:** `storage-monitor-core` gains a `system` port (filesystem and clock behind a trait) and an `action` module (plan → preview → execute, safety guards, JSONL log). The scanner gains `rescan_path` and the arena gains `replace_subtrees`, so a finished batch patches the tree in place of a full rescan. The desktop crate wires three thin commands over the core and re-reads disk usage. The UI adds selection to `NodeTable`, a confirmation dialog and the Activity page.

**Tech Stack:** Rust (trash 5.2.9, chrono, serde_json, thiserror, tempfile), Tauri 2, React 19, TanStack Query 5, Vitest, Playwright.

**Design reference:** `docs/plans/2026-09-18-phase-2a-deletion-design.md`. Policy: `docs/adr/0003-trash-first-dual-deletion.md`. Engine stages and guards: design section 9 of `docs/plans/2026-09-17-storage-monitor-design.md`.

**Decisions already made (do not re-open):**

- The `System` port covers filesystem and clock only. Process execution arrives in 2b.
- Both modes ship; the mode is chosen per batch and is not remembered until Settings exists.
- `trash` 5.2.9 with `set_delete_method(DeleteMethod::NsFileManager)`. The `Finder` default shells out to `osascript` and needs an Automation grant that an ad-hoc signed build loses on every rebuild. The cost is that "Put Back" may be missing, so the UI says "Show in Trash" and never promises Put Back.
- After a batch only the deleted paths are rescanned, never their parents.
- The arena is rebuilt to splice the results in; children live in contiguous id ranges.

**Conventions for every task:**

- Work in the worktree `.worktrees/phase-2a-deletion` on branch `feat/phase-2a-deletion`. Never commit to `main`.
- TDD: write the failing test, watch it fail, write the minimal implementation, watch it pass, commit.
- Conventional commits, one concern each: `feat(core): ...`, `feat(desktop): ...`, `test(desktop): ...`.
- Rust structs crossing IPC carry `#[serde(rename_all = "camelCase")]`; every new command gets a wrapper in `src/lib/ipc.ts` and a handler in `src/mocks/ipc.ts`.
- After every Rust task: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`.
- The local Homebrew toolchain is older than CI's stable; run `just clippy-ci` (Docker) before pushing.

---

### Task 1: The `System` port

**Files:**

- Create: `crates/core/src/system/mod.rs`, `crates/core/src/system/real.rs`, `crates/core/src/system/test.rs`
- Modify: `Cargo.toml` (workspace deps), `crates/core/Cargo.toml`, `crates/core/src/lib.rs`

**Step 1: Add the dependency**

In the root `Cargo.toml` under `[workspace.dependencies]`:

```toml
trash = "5.2.9"
```

In `crates/core/Cargo.toml` under `[dependencies]`: `trash = { workspace = true }`.

**Step 2: Write the failing tests in `crates/core/src/system/test.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn trash_moves_the_entry_into_the_trash_dir() {
        let sys = TestSystem::new();
        let file = sys.root().join("a.bin");
        fs::write(&file, b"xxx").unwrap();
        sys.move_to_trash(&file).unwrap();
        assert!(!file.exists());
        assert_eq!(fs::read(sys.trash_dir().join("a.bin")).unwrap(), b"xxx");
    }

    #[test]
    fn trash_keeps_both_entries_when_the_name_repeats() {
        let sys = TestSystem::new();
        for content in [b"one", b"two"] {
            let file = sys.root().join("same.bin");
            fs::write(&file, content).unwrap();
            sys.move_to_trash(&file).unwrap();
        }
        let names: Vec<_> = fs::read_dir(sys.trash_dir()).unwrap().count().into();
        assert_eq!(names, 2, "the second entry must not overwrite the first");
    }

    #[test]
    fn remove_deletes_a_directory_tree() {
        let sys = TestSystem::new();
        let dir = sys.root().join("tree/inner");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("f.bin"), b"x").unwrap();
        sys.remove(&sys.root().join("tree")).unwrap();
        assert!(!sys.root().join("tree").exists());
    }

    #[test]
    fn remove_deletes_a_symlink_without_touching_its_target() {
        let sys = TestSystem::new();
        let target = sys.root().join("target.bin");
        fs::write(&target, b"keep").unwrap();
        let link = sys.root().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        sys.remove(&link).unwrap();
        assert!(!link.exists());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
    }

    #[test]
    fn trash_moves_a_symlink_without_following_it() {
        let sys = TestSystem::new();
        let target = sys.root().join("target.bin");
        fs::write(&target, b"keep").unwrap();
        let link = sys.root().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        sys.move_to_trash(&link).unwrap();
        assert!(fs::symlink_metadata(sys.trash_dir().join("link")).unwrap().is_symlink());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
    }

    #[test]
    fn symlink_metadata_reports_the_link_not_the_target() {
        let sys = TestSystem::new();
        let dir = sys.root().join("dir");
        fs::create_dir(&dir).unwrap();
        let link = sys.root().join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(sys.symlink_metadata(&link).unwrap().is_symlink());
    }

    #[test]
    fn missing_path_is_reported_as_missing() {
        let sys = TestSystem::new();
        let err = sys.symlink_metadata(&sys.root().join("nope")).unwrap_err();
        assert!(matches!(err, SystemError::Missing(_)), "got {err:?}");
    }

    #[test]
    fn the_clock_is_fixed_and_can_be_moved() {
        let sys = TestSystem::new();
        let first = sys.now();
        assert_eq!(sys.now(), first, "the test clock does not drift");
        sys.advance(chrono::Duration::seconds(5));
        assert_eq!(sys.now(), first + chrono::Duration::seconds(5));
    }
}
```

Fix the second test while writing it — `fs::read_dir(..).count().into()` does not compile. Write it as:

```rust
        let names = fs::read_dir(sys.trash_dir()).unwrap().count();
        assert_eq!(names, 2, "the second entry must not overwrite the first");
```

**Step 3: Run to verify failure**

Run: `cargo test -p storage-monitor-core system`
Expected: compile errors, `TestSystem` and `SystemError` are missing.

**Step 4: Implement `crates/core/src/system/mod.rs`**

```rust
//! The one door to the outside world: filesystem operations that delete things, and the
//! clock. Everything the action engine touches goes through this trait, so tests run
//! against a temporary filesystem on any platform. Process execution joins it in phase 2b.

mod real;
#[cfg(any(test, feature = "testing"))]
mod test;

use std::fs::Metadata;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

pub use real::RealSystem;
#[cfg(any(test, feature = "testing"))]
pub use test::TestSystem;

#[derive(Debug, thiserror::Error)]
pub enum SystemError {
    #[error("{} no longer exists", .0.display())]
    Missing(PathBuf),
    #[error("cannot read {}: {source}", path.display())]
    Metadata { path: PathBuf, source: std::io::Error },
    #[error("cannot move {} to the Trash: {message}", path.display())]
    Trash { path: PathBuf, message: String },
    #[error("cannot delete {}: {source}", path.display())]
    Remove { path: PathBuf, source: std::io::Error },
}

pub trait System: Send + Sync {
    /// Metadata that does not follow symlinks.
    fn symlink_metadata(&self, path: &Path) -> Result<Metadata, SystemError>;
    /// Moves the entry to the Trash. A symlink is moved as a link.
    fn move_to_trash(&self, path: &Path) -> Result<(), SystemError>;
    /// Deletes a file, a symlink or a whole directory tree, permanently.
    fn remove(&self, path: &Path) -> Result<(), SystemError>;
    fn now(&self) -> DateTime<Utc>;
}
```

`crates/core/src/system/real.rs`:

```rust
use std::fs::{self, Metadata};
use std::path::Path;

use chrono::{DateTime, Utc};
use trash::TrashContext;
use trash::macos::{DeleteMethod, TrashContextExtMacos};

use super::{System, SystemError};

/// The real machine.
#[derive(Debug, Clone, Default)]
pub struct RealSystem;

impl System for RealSystem {
    fn symlink_metadata(&self, path: &Path) -> Result<Metadata, SystemError> {
        fs::symlink_metadata(path).map_err(|source| match source.kind() {
            std::io::ErrorKind::NotFound => SystemError::Missing(path.to_path_buf()),
            _ => SystemError::Metadata { path: path.to_path_buf(), source },
        })
    }

    fn move_to_trash(&self, path: &Path) -> Result<(), SystemError> {
        let mut ctx = TrashContext::default();
        // `Finder` is the crate's default and the only method that produces a reliable
        // "Put Back", but it drives Finder through `osascript` and needs an Automation
        // grant that an ad-hoc signed build loses on every rebuild. See the design, §11.
        #[cfg(target_os = "macos")]
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        ctx.delete(path).map_err(|err| SystemError::Trash {
            path: path.to_path_buf(),
            message: err.to_string(),
        })
    }

    fn remove(&self, path: &Path) -> Result<(), SystemError> {
        let meta = self.symlink_metadata(path)?;
        let result = if meta.is_dir() {
            // Not a symlink: `symlink_metadata` reports links as links, and std removes
            // the tree without following any link inside it.
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        result.map_err(|source| SystemError::Remove { path: path.to_path_buf(), source })
    }

    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
```

The `trash::macos` import and the `set_delete_method` call both need the `#[cfg(target_os = "macos")]` gate, otherwise the Linux build fails on the missing module. Put the `use` behind the same gate.

`crates/core/src/system/test.rs` holds `TestSystem`: a `TempDir` with `root/` and `trash/` inside, a `Mutex<DateTime<Utc>>` clock with `advance`, real `fs` calls for `symlink_metadata` and `remove`, and a `move_to_trash` that renames into `trash/`, appending ` 2`, ` 3`, ... to the file name while the target exists (the macOS Trash does the same). `fs::rename` moves a symlink as a link, which is what the test above asserts.

**Step 5: Register the module**

`crates/core/src/lib.rs`: add `pub mod system;` and extend the crate doc comment with one sentence about the port. In `crates/core/Cargo.toml` add a `testing` feature so the desktop crate can use `TestSystem` in its own tests later:

```toml
[features]
testing = ["dep:tempfile"]
```

and add `tempfile` as an optional `[dependencies]` entry. Keep the `[dev-dependencies]` one as
well: `crates/core/tests/walker.rs` needs it with the feature off, and dropping it breaks
`cargo test` with `unresolved import 'tempfile'`.

**Step 6: Run the tests**

Run: `cargo test -p storage-monitor-core system`
Expected: 8 passed.

**Step 7: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add Cargo.toml Cargo.lock crates/core
git commit -m "feat(core): add the System port with a test implementation"
```

---

### Task 2: The action model and the safety guards

**Files:**

- Create: `crates/core/src/action/mod.rs`, `crates/core/src/action/model.rs`, `crates/core/src/action/guards.rs`
- Modify: `crates/core/src/lib.rs`

**Step 1: Write the failing tests in `guards.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn limits(root: &std::path::Path) -> Limits {
        Limits::new(root.to_path_buf(), vec![root.join("Library")])
    }

    #[test]
    fn a_path_inside_the_root_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("keep/a.bin");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"x").unwrap();
        assert_eq!(limits(dir.path()).check(&file).unwrap(), fs::canonicalize(&file).unwrap());
    }

    #[test]
    fn the_scan_root_itself_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(limits(dir.path()).check(dir.path()), Err(BlockReason::IsRoot));
    }

    #[test]
    fn an_ancestor_of_the_root_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("home/user");
        fs::create_dir_all(&root).unwrap();
        assert_eq!(limits(&root).check(&dir.path().join("home")), Err(BlockReason::IsRoot));
    }

    #[test]
    fn a_path_outside_the_root_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("home");
        fs::create_dir_all(&root).unwrap();
        let outside = dir.path().join("elsewhere");
        fs::create_dir(&outside).unwrap();
        assert_eq!(limits(&root).check(&outside), Err(BlockReason::OutsideRoots));
    }

    #[test]
    fn a_denylisted_subtree_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let inside = dir.path().join("Library/Caches/app");
        fs::create_dir_all(&inside).unwrap();
        assert_eq!(limits(dir.path()).check(&inside), Err(BlockReason::Denylisted));
    }

    #[test]
    fn a_missing_path_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(limits(dir.path()).check(&dir.path().join("gone/a.bin")), Err(BlockReason::Missing));
    }

    #[test]
    fn a_symlink_is_kept_as_a_link_and_not_resolved() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let root = dir.path().join("home");
        fs::create_dir(&root).unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        // The link lives inside the root, so it may be deleted; the target must not be
        // what the guard returns, or we would delete outside the root.
        assert_eq!(limits(&root).check(&link).unwrap(), fs::canonicalize(&root).unwrap().join("link"));
    }

    #[test]
    fn dot_dot_cannot_climb_out_of_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("home");
        fs::create_dir_all(root.join("sub")).unwrap();
        let sneaky = root.join("sub/../../outside");
        fs::create_dir(dir.path().join("outside")).unwrap();
        assert_eq!(limits(&root).check(&sneaky), Err(BlockReason::OutsideRoots));
    }

    #[test]
    fn the_filesystem_root_is_refused() {
        // The port accepts "/": it is a well-formed absolute path and the port is not a
        // policy layer. Rule 1 is the only thing between a batch and `remove_dir_all("/")`
        // — the denylist never gets a say, because `Limits::new` drops an entry that
        // contains the root and "/" contains every root. Hence its own test.
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(limits(dir.path()).check(std::path::Path::new("/")), Err(BlockReason::Malformed));
    }

    #[test]
    fn nested_entries_collapse_to_their_ancestor() {
        let paths = vec![
            std::path::PathBuf::from("/h/a"),
            std::path::PathBuf::from("/h/a/b"),
            std::path::PathBuf::from("/h/c"),
            std::path::PathBuf::from("/h/ab"),
        ];
        let kept = drop_nested(&paths);
        assert_eq!(kept, vec![false, true, false, false].into_iter().map(|n| !n).collect::<Vec<_>>());
    }
}
```

Write the last test as a straightforward assertion instead of the `map` gymnastics:

```rust
        assert_eq!(drop_nested(&paths), vec![true, false, true, true]);
```

`drop_nested` returns, per entry and in input order, whether it survives. `/h/ab` must survive: a prefix check on strings would swallow it, so compare path components, not bytes.

**Step 2: Run to verify failure**

Run: `cargo test -p storage-monitor-core action::guards`
Expected: compile errors, `Limits` and `check` are missing.

**Step 3: Implement `model.rs`**

The types from design section 5: `Mode`, `PlanEntry`, `Plan`, `BlockReason`, `EntryStatus`, `PreviewEntry`, `Preview`, `EntryResult`, `EntryOutcome`, `Outcome`. All of them `#[serde(rename_all = "camelCase")]`, `BlockReason` and `Mode` also `#[derive(PartialEq, Eq)]` so tests can compare them. `Preview::total_bytes` counts `Ready` entries only.

`EntryResult::Failed` and `EntryResult::Skipped` are struct variants (`Failed { message }`, `Skipped { reason }`), not newtypes. Serde's internal tagging — the `tag = "result"` these types need to cross IPC — cannot serialize a newtype variant whose content is not a map, and it fails at runtime, not at compile time. `EntryStatus::Blocked(BlockReason)` stays a tuple variant; adjacent tagging handles that one.

**Step 4: Implement `guards.rs`**

```rust
/// Where deletion is allowed and where it is never allowed.
#[derive(Debug, Clone)]
pub struct Limits {
    root: PathBuf,
    denied: Vec<PathBuf>,
}

impl Limits {
    /// `root` is canonicalized here. Each denied entry is kept in two forms — fully
    /// canonicalized, and with only its parent canonicalized — so a denied symlink such
    /// as `/etc` refuses both the link itself and `/private/etc/...` below it. An entry
    /// that does not exist on this machine is kept as given and simply never matches.
    pub fn new(root: PathBuf, denied: Vec<PathBuf>) -> Self { ... }

    /// The root plus the standard denylist of design section 9. `new` drops any denied
    /// entry that contains the root, or rule 7 would refuse every path in the tree: `/`
    /// contains every root, and the home folder contains the default one.
    ///
    /// This means a root *inside* a denied entry opens that entry up — scanning
    /// `~/Library` makes its contents deletable. That is the policy: a root is something
    /// the user pointed at deliberately. The root picker of phase 2b is where a warning
    /// belongs, not here.
    pub fn for_scan_root(root: PathBuf) -> Self {
        let mut denied = vec![
            PathBuf::from("/"), PathBuf::from("/System"), PathBuf::from("/usr"),
            PathBuf::from("/bin"), PathBuf::from("/sbin"), PathBuf::from("/Library"),
            PathBuf::from("/etc"), PathBuf::from("/var"), PathBuf::from("/tmp"),
            PathBuf::from("/private"), PathBuf::from("/Applications"),
            PathBuf::from("/Users"), PathBuf::from("/Volumes"),
            PathBuf::from("/opt"), PathBuf::from("/cores"),
        ];
        if let Some(home) = crate::paths::home_dir() {
            denied.push(home.join("Library"));
            denied.push(home);
        }
        Self::new(root, denied)
    }
}

impl Limits {
    /// Normalizes `path` and applies every rule.
    pub fn check(&self, path: &Path) -> Result<Checked, BlockReason> { ... }
}

/// A path that passed the rules, in the two forms the engine needs.
pub struct Checked {
    /// What gets deleted: the parent resolved, the last component exactly as written, so a
    /// symlink is removed as a link.
    pub path: PathBuf,
    /// What the rules judged: fully resolved when the entry exists and is not a symlink.
    /// Comparisons between entries belong here — two spellings of one directory (`Data` and
    /// `data` on a case-insensitive volume, `café` in NFC and NFD) share it and differ in
    /// `path`.
    pub judged: PathBuf,
}

/// Per entry, in input order: false when another entry in the list contains it.
/// Of several copies of one path the first survives — read literally, the rule would
/// drop every copy, since equal paths contain each other.
pub fn drop_nested(paths: &[PathBuf]) -> Vec<bool> { ... }
```

`check` in order:

1. `path.file_name()` is `None` → `Malformed` (this is `/` or a trailing `..`).
2. Canonicalize the parent. `ErrorKind::NotFound` → `Missing`; any other error → `Unreadable`. They are different things: a `chmod 000` parent is not a vanished one, and this app has a whole design about Full Disk Access — "nothing is there any more" would send the user chasing a ghost.
3. `normalized = parent.join(file_name)`. Canonicalizing `path` itself would resolve a symlink to its target and delete the wrong thing.
4. Pick what the rules below judge: `lstat` the entry, and when it exists and is **not** a symlink, judge `normalized.canonicalize()`; otherwise judge `normalized`. Rules 5–7 then compare a path whose every component carries its on-disk spelling, while step 3's value is still what gets deleted. Without this the last component keeps whatever the caller wrote, and on a case-insensitive filesystem `library` walks past a denylist that names `Library`.
5. judged `== root` or `root.starts_with(&judged)` → `IsRoot`.
6. `!judged.starts_with(&root)` → `OutsideRoots`.
7. Any denied `d` with `judged == d || judged.starts_with(d)` → `Denylisted`.

`starts_with` on `Path` compares whole components, so `/h/ab` does not start with `/h/a`. Use it everywhere; never compare strings.

Two constraints the `System` port places on this step, both verified in its review:

- The port refuses any path that is not absolute and in normal form. Step 3 above is what produces that form, so `parent.canonicalize().join(file_name)` carries two loads at once: it defends against a symlink in the middle of the path, and it makes the result absolute even when the scan root was relative (`Tree::path` keeps whatever the root was; only the CLI normalizes it). Do not "optimize" the canonicalization away for paths that already look absolute.
- The port accepts `/` — it is a well-formed path and the port is not a policy layer. Rules 1 and 4 are therefore the only thing standing between a batch and `remove_dir_all("/")`, hence the dedicated test above.

**Step 5: Run the tests**

Run: `cargo test -p storage-monitor-core action::guards`
Expected: 9 passed.

**Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add crates/core
git commit -m "feat(core): add the action model and the deletion safety guards"
```

---

### Task 3: Preview

**Files:**

- Create: `crates/core/src/action/engine.rs`
- Modify: `crates/core/src/action/mod.rs`

**Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::NodeKind;
    use crate::system::TestSystem;
    use std::fs;

    fn plan(sys: &TestSystem, names: &[&str], mode: Mode) -> Plan {
        Plan {
            entries: names
                .iter()
                .enumerate()
                .map(|(i, n)| PlanEntry {
                    path: sys.root().join(n),
                    kind: NodeKind::File,
                    // 10, 20, 30 ... so a total names exactly one entry: with equal sizes,
                    // `freed_bytes == 10` over two entries is true even if the wrong one
                    // was counted.
                    size: (i as u64 + 1) * 10,
                })
                .collect(),
            mode,
        }
    }

    #[test]
    fn ready_entries_are_totalled_and_blocked_ones_are_not() {
        let sys = TestSystem::new();
        fs::write(sys.root().join("a.bin"), b"x").unwrap();
        let mut p = plan(&sys, &["a.bin", "gone.bin"], Mode::Trash);
        p.entries[1].size = 999;
        let checked = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        assert_eq!(checked.entries[0].status, EntryStatus::Ready);
        assert_eq!(checked.entries[1].status, EntryStatus::Blocked(BlockReason::Missing));
        assert_eq!(checked.total_bytes, 10, "a blocked entry contributes nothing");
        assert_eq!(checked.mode, Mode::Trash);
    }

    #[test]
    fn a_descendant_of_another_entry_is_blocked_as_nested() {
        let sys = TestSystem::new();
        fs::create_dir_all(sys.root().join("dir/inner")).unwrap();
        let p = Plan {
            entries: vec![
                PlanEntry { path: sys.root().join("dir"), kind: NodeKind::Dir, size: 100 },
                PlanEntry { path: sys.root().join("dir/inner"), kind: NodeKind::Dir, size: 40 },
            ],
            mode: Mode::Permanent,
        };
        let checked = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        assert_eq!(checked.entries[1].status, EntryStatus::Blocked(BlockReason::Nested));
        assert_eq!(checked.total_bytes, 100, "the child's bytes are not counted twice");
    }

    #[test]
    fn preview_touches_nothing_on_disk() {
        let sys = TestSystem::new();
        let file = sys.root().join("a.bin");
        fs::write(&file, b"x").unwrap();
        let p = plan(&sys, &["a.bin"], Mode::Permanent);
        preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        // Not `exists`, which follows a symlink — the same idiom `system/test.rs` uses.
        assert!(fs::symlink_metadata(&file).is_ok());
        assert_eq!(fs::read_dir(sys.trash_dir()).unwrap().count(), 0);
    }

    #[test]
    fn the_kind_recorded_in_the_preview_comes_from_the_disk() {
        let sys = TestSystem::new();
        fs::create_dir(sys.root().join("a.bin")).unwrap();
        let p = plan(&sys, &["a.bin"], Mode::Trash); // planned as a File
        let checked = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        assert_eq!(checked.entries[0].kind, NodeKind::Dir);
    }
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p storage-monitor-core action::engine`
Expected: compile errors, `preview` is missing.

**Step 3: Implement**

```rust
/// Checks a plan without touching anything. Every entry keeps its place in the list, so
/// the UI can show blocked ones with their reason.
pub fn preview(plan: &Plan, limits: &Limits, sys: &dyn System) -> Preview
```

For each entry: run `limits.check(path)`, which already reports `Missing`, `Unreadable` and `Malformed` as well as the three placement reasons; then `sys.symlink_metadata` on what it returned (a `Missing` error becomes `Blocked(Missing)`) and record the kind the disk reports, through `NodeKind::from_metadata` — the same classification the walker uses, so a socket or a fifo is `Other` here as well. Rolling a private `if is_dir { Dir } else { File }` instead would make every such entry fail re-validation with `KindChanged` forever. Then apply `drop_nested` over the **judged** paths of the entries that are still `Ready`, blocking the descendants with `Nested`. Judged, not normalized: the normalized form keeps the caller's spelling of the last component, and `drop_nested` compares byte-exact per component, so `Data` and `data` on a case-insensitive volume — or `café` in NFC and NFD, which needs no user error at all — would both stay ready and `total_bytes` would promise the same megabytes twice. Sum `size` over `Ready` entries into `total_bytes`.

**Step 4: Run the tests**

Run: `cargo test -p storage-monitor-core action::engine`
Expected: 4 passed.

**Step 5: Commit**

```bash
git add crates/core && git commit -m "feat(core): preview a deletion plan against the guards"
```

---

### Task 4: Execute

**Files:**

- Modify: `crates/core/src/action/engine.rs`

**Step 1: Write the failing tests**

```rust
    #[test]
    fn trash_mode_moves_the_entries_to_the_trash() {
        let sys = TestSystem::new();
        fs::write(sys.root().join("a.bin"), b"xxx").unwrap();
        let p = plan(&sys, &["a.bin"], Mode::Trash);
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let outcome = execute(&preview(&p, &limits, &sys), &limits, &sys);
        assert_eq!(outcome.freed_bytes, 10);
        assert!(matches!(outcome.entries[0].result, EntryResult::Removed { bytes: 10 }));
        assert!(fs::symlink_metadata(sys.trash_dir().join("a.bin")).is_ok());
        assert_eq!(outcome.at, sys.now());
    }

    #[test]
    fn permanent_mode_deletes_without_the_trash() {
        let sys = TestSystem::new();
        fs::write(sys.root().join("a.bin"), b"xxx").unwrap();
        let p = plan(&sys, &["a.bin"], Mode::Permanent);
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        execute(&preview(&p, &limits, &sys), &limits, &sys);
        assert!(fs::symlink_metadata(sys.root().join("a.bin")).is_err());
        assert_eq!(fs::read_dir(sys.trash_dir()).unwrap().count(), 0);
    }

    #[test]
    fn an_entry_that_vanished_between_the_stages_does_not_stop_the_batch() {
        let sys = TestSystem::new();
        fs::write(sys.root().join("a.bin"), b"x").unwrap();
        fs::write(sys.root().join("b.bin"), b"x").unwrap();
        let p = plan(&sys, &["a.bin", "b.bin"], Mode::Permanent);
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let checked = preview(&p, &limits, &sys);
        fs::remove_file(sys.root().join("a.bin")).unwrap(); // vanishes between the stages
        let outcome = execute(&checked, &limits, &sys);
        assert!(matches!(outcome.entries[0].result, EntryResult::Skipped { reason: BlockReason::Missing }));
        assert!(matches!(outcome.entries[1].result, EntryResult::Removed { .. }));
        assert_eq!(outcome.freed_bytes, 20, "only what was really deleted");
    }

    #[test]
    fn an_entry_whose_kind_changed_is_skipped() {
        let sys = TestSystem::new();
        fs::write(sys.root().join("a.bin"), b"x").unwrap();
        let p = plan(&sys, &["a.bin"], Mode::Permanent);
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let checked = preview(&p, &limits, &sys);
        fs::remove_file(sys.root().join("a.bin")).unwrap();
        fs::create_dir(sys.root().join("a.bin")).unwrap(); // same name, now a directory
        let outcome = execute(&checked, &limits, &sys);
        assert!(matches!(outcome.entries[0].result, EntryResult::Skipped { reason: BlockReason::KindChanged }));
        assert!(sys.root().join("a.bin").is_dir(), "the replacement is left alone");
    }

    #[test]
    fn a_failing_deletion_is_reported_as_failed_not_skipped() {
        let sys = TestSystem::new();
        fs::write(sys.root().join("a.bin"), b"x").unwrap();
        fs::write(sys.root().join("b.bin"), b"x").unwrap();
        let p = plan(&sys, &["a.bin", "b.bin"], Mode::Trash);
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let checked = preview(&p, &limits, &sys);
        sys.fail_next(&sys.root().join("a.bin"));
        let outcome = execute(&checked, &limits, &sys);
        assert!(matches!(outcome.entries[0].result, EntryResult::Failed { .. }));
        assert!(matches!(outcome.entries[1].result, EntryResult::Removed { .. }));
        assert!(fs::symlink_metadata(sys.root().join("a.bin")).is_ok(), "a failure leaves the entry alone");
        assert_eq!(outcome.freed_bytes, 20, "b.bin only — the second entry is the one that got through");
    }

    #[test]
    fn blocked_entries_are_reported_but_never_touched() {
        let sys = TestSystem::new();
        let p = plan(&sys, &["gone.bin"], Mode::Trash);
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let outcome = execute(&preview(&p, &limits, &sys), &limits, &sys);
        assert_eq!(outcome.entries.len(), 1);
        assert!(matches!(outcome.entries[0].result, EntryResult::Skipped { reason: BlockReason::Missing }));
        assert_eq!(outcome.freed_bytes, 0);
    }
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p storage-monitor-core action::engine`
Expected: compile error, `execute` is missing.

**Step 3: Implement**

```rust
/// Runs a checked preview. Blocked entries are reported as skipped; a failure does not
/// stop the rest of the batch.
pub fn execute(preview: &Preview, limits: &Limits, sys: &dyn System) -> Outcome
```

`execute` takes the limits and re-runs `check` on every ready entry, not only the kind comparison. `Preview` has public fields and derives `Deserialize`, so one can be built without ever passing the guards; re-checking here means a forged preview buys nothing, and the deletion is guarded at the point where it happens rather than by a promise made upstream. A path that now fails becomes `Skipped` with the reason the guards gave.

Per entry: `Blocked(reason)` → `Skipped { reason }`. `Ready` → re-read `symlink_metadata` (missing → `Skipped { reason: Missing }`), compare `NodeKind::from_metadata` with the preview's kind (different → `Skipped { reason: KindChanged }`), then `move_to_trash` or `remove` by mode; an error becomes `Failed { message }`. `freed_bytes` sums the `Removed` entries. `at` comes from `sys.now()`.

`remove` is not atomic: a tree can be part-deleted and then fail, which lands as `Failed` with 0 bytes even though gigabytes are gone. That is a deliberate lower bound — the tree still ends up correct, because Task 8 rescans the path either way and splices in whatever remains.

**Step 4: Run the tests**

Run: `cargo test -p storage-monitor-core action`
Expected: every action test, tasks 2 and 3 included — around 70 by this point, not only the ones this task adds.

**Step 5: Commit**

```bash
git add crates/core && git commit -m "feat(core): execute a previewed deletion batch"
```

---

### Task 5: The action log

**Files:**

- Create: `crates/core/src/action/log.rs`
- Modify: `crates/core/src/action/mod.rs`, `crates/core/src/paths.rs`

**Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::NodeKind;

    fn outcome(path: &str, bytes: u64) -> EntryOutcome {
        EntryOutcome {
            path: path.into(),
            kind: NodeKind::Dir,
            result: EntryResult::Removed { bytes },
        }
    }

    #[test]
    fn entries_are_appended_and_read_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let log = ActionLog::new(dir.path().join("actions.jsonl"));
        let at = chrono::Utc::now();
        log.append(&batch(Mode::Trash, at, vec![outcome("/h/a", 10)])).unwrap();
        log.append(&batch(Mode::Permanent, at, vec![outcome("/h/b", 20), outcome("/h/c", 30)])).unwrap();
        let entries = log.tail(10).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, "/h/c");
        assert_eq!(entries[0].mode, Mode::Permanent);
        assert_eq!(entries[2].path, "/h/a");
        assert_eq!(entries[2].mode, Mode::Trash);
    }

    #[test]
    fn tail_returns_at_most_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let log = ActionLog::new(dir.path().join("actions.jsonl"));
        for i in 0..5 {
            log.append(&batch(Mode::Trash, chrono::Utc::now(), vec![outcome(&format!("/h/{i}"), 1)])).unwrap();
        }
        let entries = log.tail(2).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "/h/4");
    }

    #[test]
    fn a_damaged_line_is_skipped_instead_of_failing_the_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actions.jsonl");
        let log = ActionLog::new(path.clone());
        log.append(&batch(Mode::Trash, chrono::Utc::now(), vec![outcome("/h/a", 1)])).unwrap();
        std::fs::write(&path, format!("{{ truncated\n{}", std::fs::read_to_string(&path).unwrap())).unwrap();
        assert_eq!(log.tail(10).unwrap().len(), 1);
    }

    #[test]
    fn a_missing_log_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let log = ActionLog::new(dir.path().join("nothing.jsonl"));
        assert!(log.tail(10).unwrap().is_empty());
    }

    #[test]
    fn failures_are_logged_too() {
        let dir = tempfile::tempdir().unwrap();
        let log = ActionLog::new(dir.path().join("actions.jsonl"));
        let entry = EntryOutcome {
            path: "/h/x".into(),
            kind: NodeKind::File,
            result: EntryResult::Failed { message: "permission denied".into() },
        };
        log.append(&batch(Mode::Trash, chrono::Utc::now(), vec![entry])).unwrap();
        let read = &log.tail(1).unwrap()[0];
        assert_eq!(read.bytes, 0);
        assert!(matches!(read.result, LogResult::Failed));
        assert_eq!(read.detail.as_deref(), Some("permission denied"));
    }
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p storage-monitor-core action::log`
Expected: compile errors.

**Step 3: Implement**

`paths.rs` gains:

```rust
/// The append-only log of everything the app deleted.
pub fn actions_log() -> PathBuf {
    data_dir().join("actions.jsonl")
}
```

`log.rs`: `LogEntry { at, path: String, kind: NodeKind, mode: Mode, result: LogResult, detail: Option<String>, bytes: u64 }` with `LogResult { Removed, Failed, Skipped }`, all camelCase. `ActionLog::append` creates the parent directory, opens with `OpenOptions::new().create(true).append(true)`, and writes all lines of the batch in one `write_all` so a batch never interleaves with another writer. `tail` reads the file as **bytes** and decodes with `String::from_utf8_lossy`, then iterates lines from the end, `serde_json::from_str` per line, skipping the errors, until it has `limit` of them. Not `read_to_string`: that fails the whole read with `InvalidData` when a write was torn in the middle of a multi-byte character, which is exactly the racing-`tail` case this has to survive. Reading from the end also parses at most `limit` lines instead of all of them.

Several things follow from `Outcome` being the only input:

- Every line of a batch carries the same `at`, read before the first deletion. "Newest first" therefore rests on **file order**, which is what `tail` reverses — do not "improve" it into a sort by `at`, or two batches inside one clock tick will come back shuffled.
- `EntryOutcome::path` is a `PathBuf` and `LogEntry::path` is a `String`. Convert with `to_string_lossy()`, not an `unwrap`, and point the comment at the paragraph in `model.rs` that explains why a lossy conversion cannot lose anything here.
- A `Failed` message already embeds the absolute path, because it comes from `SystemError`'s `Display`. `detail` will therefore repeat `path`; harmless in the file, but the Activity screen must not render both.
- `bytes` is 0 for everything that is not `Removed`. That is deliberate — nothing was freed — but the Activity screen should not read as though a skipped row was worth nothing.
- `detail` carries the failure message for `Failed` **and the block reason for `Skipped`**, as the wire name `BlockReason` serializes to (`"denylisted"`, `"outsideRoots"`, …). Without it a skipped row reaches Activity with no reason at all. It is the same vocabulary `ipc.ts` already mirrors, so Task 15 maps it the way `nodeErrors.ts` maps scan errors.
- An empty batch writes nothing and creates nothing — no file, no directory. A no-op leaving an empty `actions.jsonl` behind is worse than no file.
- A filename may contain a newline, so lines are written through `serde_json`, never formatted by hand: an unescaped name could otherwise forge a log line.
- **`append` prefixes a newline when the file does not already end with one.** A write torn by a full volume — the most likely failure this app will ever meet, since its users are people whose disk is full — leaves the file mid-line; without the check, the remainder and the first object of the next batch glue into one unparseable line and `tail` drops **both**, silently losing a record of a deletion that really happened.
- `append` calls `sync_all` before returning, so `Ok` means the bytes reached the disk rather than the kernel. One batch is one user gesture, so the cost is imperceptible, and it turns a full volume from "loses a record" into "reports an error".
- The file is created `0o600`: it names every path the user has ever deleted.
- **Every field added to `LogEntry` from here on carries `#[serde(default)]`.** The derived `Deserialize` requires every field, so adding one without it makes the whole existing history vanish from the Activity screen — no error, no count, just an empty list over a file full of records.
- `tail` returns a `LogTail` — the entries **and how many lines it had to drop** — so the screen can say "3 damaged entries hidden" instead of quietly showing fewer rows than the user remembers deleting.
- There is no batch identity on a line, deliberately: nothing in 2a groups rows, and the `serde(default)` rule above is what makes adding one later cheap rather than destructive. The line is the unit.

**Step 4: Run the tests**

Run: `cargo test -p storage-monitor-core action::log`
Expected: 5 passed.

**Step 5: Commit**

```bash
git add crates/core && git commit -m "feat(core): append deletions to an action log"
```

---

### Task 6: `rescan_path`

**Files:**

- Modify: `crates/core/src/scan/walker.rs`, `crates/core/src/scan/mod.rs`
- Test: `crates/core/tests/walker.rs`

**Step 1: Write the failing tests in `crates/core/tests/walker.rs`**

```rust
#[test]
fn rescan_of_a_deleted_path_reports_nothing() {
    let dir = fixture();
    let gone = dir.path().join("docs");
    fs::remove_dir_all(&gone).unwrap();
    let options = ScanOptions::new(dir.path().to_path_buf());
    assert!(rescan_path(&gone, &options).unwrap().is_none());
}

#[test]
fn rescan_of_a_directory_returns_its_current_contents() {
    let dir = fixture();
    let docs = dir.path().join("docs");
    fs::remove_file(docs.join("report.txt")).unwrap();
    let options = ScanOptions::new(dir.path().to_path_buf());
    let tree = rescan_path(&docs, &options).unwrap().unwrap();
    assert_eq!(tree.root().file_count, 2, "only the two notes are left");
    assert_eq!(tree.root().logical_size, 300);
}

#[test]
fn rescan_of_a_single_file_returns_one_node() {
    let dir = fixture();
    let file = dir.path().join("big.bin");
    let options = ScanOptions::new(dir.path().to_path_buf());
    let tree = rescan_path(&file, &options).unwrap().unwrap();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree.root().kind, NodeKind::File);
    assert_eq!(tree.root().logical_size, 40_000);
}

#[test]
fn rescan_of_a_symlink_does_not_follow_it() {
    let dir = fixture();
    let link = dir.path().join("docs-link");
    std::os::unix::fs::symlink(dir.path().join("docs"), &link).unwrap();
    let options = ScanOptions::new(dir.path().to_path_buf());
    let tree = rescan_path(&link, &options).unwrap().unwrap();
    assert_eq!(tree.root().kind, NodeKind::Symlink);
    assert_eq!(tree.len(), 1);
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p storage-monitor-core --test walker rescan`
Expected: compile errors, `rescan_path` is missing.

**Step 3: Implement in `walker.rs`**

```rust
/// A fresh [`Tree`] rooted at `path`, or `None` when the path is gone. `options` supplies
/// the excludes and the same-volume rule of the scan this patch belongs to; its `root` is
/// ignored. The root node carries `path` as its name, exactly like [`scan`].
pub fn rescan_path(path: &Path, options: &ScanOptions) -> Result<Option<Tree>, ScanError>
```

`symlink_metadata` on the path: a `NotFound` error is `Ok(None)`, another error is `ScanError::Root`. A directory delegates to `scan` with `ScanOptions { root: path.to_path_buf(), ..options.clone() }` and returns its tree. Anything else becomes a single node through `Subtree::new(node).flatten().0`, reusing the same `node_from_metadata` helper the walker already has.

Export it from `crates/core/src/scan/mod.rs`.

**Step 4: Run the tests**

Run: `cargo test -p storage-monitor-core --test walker`
Expected: the whole walker file, the 18 tests phase 1 left included — not only the ones this task adds.

**Step 5: Commit**

```bash
git add crates/core && git commit -m "feat(core): rescan a single path after a deletion"
```

---

### Task 7: Patching the arena

**Files:**

- Modify: `crates/core/src/scan/tree.rs`

**Step 1: Write the failing tests**

```rust
    #[test]
    fn find_locates_a_node_by_its_absolute_path() {
        let tree = sample(); // /root with a/{x.bin,y.bin} and b.bin
        let a = tree.find(std::path::Path::new("/root/a")).unwrap();
        assert_eq!(tree.get(a).unwrap().name.as_ref(), "a");
        assert_eq!(tree.find(std::path::Path::new("/root")), Some(Tree::ROOT));
        assert_eq!(tree.find(std::path::Path::new("/root/nope")), None);
        assert_eq!(tree.find(std::path::Path::new("/elsewhere")), None);
    }

    #[test]
    fn dropping_a_node_shrinks_its_ancestors() {
        let tree = sample();
        let a = tree.find(std::path::Path::new("/root/a")).unwrap();
        let patched = replace_subtrees(&tree, vec![(a, None)]);
        assert_eq!(patched.find(std::path::Path::new("/root/a")), None);
        assert_eq!(patched.root().size, 10, "only b.bin is left");
        assert_eq!(patched.root().file_count, 1);
        assert_eq!(patched.children(Tree::ROOT).len(), 1);
    }

    #[test]
    fn a_replacement_subtree_takes_the_place_of_the_old_one() {
        let tree = sample();
        let a = tree.find(std::path::Path::new("/root/a")).unwrap();
        let remainder = Subtree::with_children(
            Node::new("whatever", NodeKind::Dir, 0, 0, 0, 7),
            vec![Subtree::new(Node::new("y.bin", NodeKind::File, 5, 5, 1, 7))],
        );
        let (replacement, _) = remainder.flatten();
        let patched = replace_subtrees(&tree, vec![(a, Some(replacement))]);
        let a = patched.find(std::path::Path::new("/root/a")).unwrap();
        assert_eq!(patched.get(a).unwrap().name.as_ref(), "a", "the old name is kept");
        assert_eq!(patched.get(a).unwrap().size, 5);
        assert_eq!(patched.root().size, 15);
        assert_eq!(patched.child_count(a), 1);
    }

    #[test]
    fn children_of_a_changed_group_are_sorted_again() {
        let tree = sample(); // children of /root: b.bin (10), a (35)
        let a = tree.find(std::path::Path::new("/root/a")).unwrap();
        let (small, _) = Subtree::new(Node::new("a", NodeKind::Dir, 1, 1, 0, 7)).flatten();
        let patched = replace_subtrees(&tree, vec![(a, Some(small))]);
        let names: Vec<&str> = patched
            .children(Tree::ROOT)
            .map(|id| patched.get(id).unwrap().name.as_ref())
            .collect();
        assert_eq!(names, vec!["b.bin", "a"], "largest first still holds");
    }

    #[test]
    fn errors_of_surviving_nodes_are_kept_and_of_dropped_ones_are_not() {
        let mut root = Subtree::with_children(
            Node::new("/root", NodeKind::Dir, 0, 0, 0, 7),
            vec![
                Subtree::new(Node::new("locked", NodeKind::Dir, 0, 0, 0, 7)),
                Subtree::new(Node::new("kept", NodeKind::Dir, 0, 0, 0, 7)),
            ],
        );
        root.children[0].error = Some("permission denied".into());
        root.children[1].error = Some("partially read".into());
        let (tree, _) = root.flatten();
        let locked = tree.find(std::path::Path::new("/root/locked")).unwrap();
        let patched = replace_subtrees(&tree, vec![(locked, None)]);
        let kept = patched.find(std::path::Path::new("/root/kept")).unwrap();
        assert_eq!(patched.error(kept), Some("partially read"));
        assert_eq!(patched.errors().len(), 1);
    }

    #[test]
    fn patching_nothing_returns_an_equal_tree() {
        let tree = sample();
        let patched = replace_subtrees(&tree, vec![]);
        assert_eq!(patched.len(), tree.len());
        assert_eq!(patched.root().size, tree.root().size);
    }
```

**Step 2: Run to verify failure**

Run: `cargo test -p storage-monitor-core scan::tree`
Expected: compile errors, `find` and `replace_subtrees` are missing.

**Step 3: Implement**

```rust
impl Tree {
    /// The node at an absolute path, or `None` when the path is not in this tree.
    pub fn find(&self, path: &Path) -> Option<NodeId> { ... }
}

/// A new arena with the listed nodes replaced by the given trees, or dropped when the
/// replacement is `None`. Ancestors are re-aggregated and the sibling groups that changed
/// are sorted again; everything else keeps its order. The root cannot be patched.
pub fn replace_subtrees(tree: &Tree, patches: Vec<(NodeId, Option<Tree>)>) -> Tree { ... }
```

`find` strips the root node's name (an absolute path) from `path` and walks the remaining components, matching each against the names in `children(current)`. Names are unique within a directory, so no backtracking is needed.

`replace_subtrees` rebuilds with the same breadth-first placement `Subtree::flatten` uses:

1. Build a map `NodeId -> Option<Tree>` from the patches and a set of the ancestors of every patched node.
2. Walk the old tree from the root. For a node that is patched: skip it entirely when the replacement is `None`; otherwise place the replacement's nodes, keeping the *old* node's `name` for the replacement's root (the rescanned tree's root carries an absolute path).
3. Place every other node as it is, with its error if it has one.
4. Aggregates come **first**, over the old tree, before a single node is placed: collect every ancestor of every patch, order them deepest first (a child's id always exceeds its parent's) and recompute each from its children, taking a patched child's weight from its replacement. Only the ancestors need it; every other node already carries its own subtree totals.
5. Then place, sorting a group with `by_size_then_name` when its parent is in the ancestor set; other groups keep the order the walker gave them.

   The order of these two matters and the obvious one does not work: a group cannot be sorted before its members' new sizes are known, and a member that is itself an ancestor of a deeper patch only learns its new size from that deeper aggregation. Sorting during placement with stale sizes puts a shrunken directory in the wrong place — `/root`'s children must come back `["b.bin", "a"]` once `a` drops from 100 to 70, and a placement-time sort would still see 100.

Note for the implementer: a node's `size` is the subtree total, and the walker adds the directory's own allocated blocks to it. When recomputing an ancestor, start from `own_size = old_size - sum(old children sizes)` so the directory's own blocks survive the patch. Compute `own_size` from the *old* tree before rebuilding.

Four things Task 6 established about the input, all pinned by its tests:

- **The names differ on purpose.** A patch root carries the absolute path; the node it replaces carries the file name. Step 2's "keep the old node's name" depends on exactly that, and `crates/core/tests/walker.rs` asserts both sides.
- **Sibling order already matches a scan's**, so a group that did not change needs no re-sorting.
- **Verify a splice at least as strictly as Task 6 verified its input.** `assert_same_subtree` and `subtree_size` in `crates/core/tests/walker.rs` compare kind, sizes, `file_count`, mtime, child names, child counts and the error side table, recursively. Promote them or copy them — do not check a splice with weaker assertions than the thing being spliced.
- **The hard-link limitation test is a tripwire, not a bug report.** If `replace_subtrees` ever re-aggregates hard links, `rescan_re_attributes_hard_links_inside_the_branch` fails. That failure means a design decision is being re-opened, not that the test needs updating.

**Step 4: Run the tests**

Run: `cargo test -p storage-monitor-core scan::tree`
Expected: 10 passed.

**Step 5: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add crates/core
git commit -m "feat(core): patch the scan tree with rescanned subtrees"
```

---

### Task 8: Deletion commands in the desktop crate

**Files:**

- Create: `apps/desktop/src-tauri/src/actions.rs`
- Modify: `apps/desktop/src-tauri/src/scan_manager.rs`, `apps/desktop/src-tauri/src/commands.rs`, `apps/desktop/src-tauri/src/lib.rs`

**Wire shapes.** These types cross IPC now, so pin their serde representation in `crates/core/src/action/model.rs` before writing the commands — TypeScript needs discriminated unions, not serde's default enum shape:

```rust
#[serde(rename_all = "camelCase")]                                  // Mode, BlockReason
#[serde(tag = "state", content = "reason", rename_all = "camelCase")] // EntryStatus
#[serde(tag = "result", rename_all = "camelCase")]                    // EntryResult
```

They produce `"trash"`, `"kindChanged"`, `{"state":"blocked","reason":"missing"}` and `{"result":"removed","bytes":10}`.

**Step 1: Write the failing tests in `apps/desktop/src-tauri/src/actions.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A manager holding a finished scan of `dir`, with its snapshots in a temp directory.
    fn scanned(dir: &std::path::Path) -> (ScanManager, tempfile::TempDir) { ... }

    #[test]
    fn a_trashed_directory_leaves_the_tree_and_shrinks_its_parent() {
        let fixture = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(fixture.path().join("cache/inner")).unwrap();
        std::fs::write(fixture.path().join("cache/inner/blob.bin"), vec![b'x'; 8_192]).unwrap();
        std::fs::write(fixture.path().join("keep.bin"), vec![b'x'; 4_096]).unwrap();
        let (manager, _snaps) = scanned(fixture.path());
        let before = manager.node_view(None, 100).unwrap().size;

        let sys = TestSystem::at(fixture.path());
        let outcome = run_batch(
            &manager,
            &sys,
            &ActionLog::new(fixture.path().join("actions.jsonl")),
            vec![fixture.path().join("cache")],
            Mode::Trash,
        );

        assert_eq!(outcome.entries.len(), 1);
        let view = manager.node_view(None, 100).unwrap();
        assert!(view.children.iter().all(|c| c.name != "cache"), "the row is gone");
        assert!(view.size < before, "the parent shrank");
    }

    #[test]
    fn a_partially_deleted_directory_keeps_what_is_left() { ... }

    #[test]
    fn a_blocked_entry_changes_nothing() { ... }

    #[test]
    fn every_entry_of_the_batch_reaches_the_log() { ... }
}
```

Fill the three sketched tests out: the second deletes a directory whose child cannot be removed (make the child read-only through its parent's permissions, as `crates/core/tests/walker.rs` already does) and asserts the remaining node is in the tree with the smaller size; the third passes a path outside the scan root and asserts the tree is untouched and the outcome says `Skipped { reason: OutsideRoots }`; the fourth reads the log back through `ActionLog::tail`.

**Step 2: Run to verify failure**

Run: `cargo test -p storage-monitor-desktop actions`
Expected: compile errors, `run_batch` is missing.

**Step 3: Implement**

`ScanManager` gains one method, and it must not hold the lock while scanning:

```rust
/// Rescans every path of a finished batch and patches the tree. Paths the tree does not
/// know are ignored. Does nothing when no scan result is held.
pub fn patch_paths(&self, paths: &[PathBuf]) {
    // 1. under the lock: clone the Arc of the result, resolve the paths to ids
    // 2. off the lock: rescan_path for each id, build the patches
    // 3. under the lock: rebuild the tree, but only if the generation did not change
}
```

Step 3 is the important one: a rescan started meanwhile invalidates the patch, so compare the generation captured in step 1 and drop the patch when it moved. Reuse the existing `Inner.generation` field.

Two rules about calling into the tree, both measured during Task 7's review:

- **Resolve the ids from the paths the UI sent, before the guards touch them.** `Tree::find` only matches the spelling the scan itself recorded: a canonicalized path, a case-different spelling on a case-insensitive volume, and NFD where the disk holds NFC all return `None`. `Limits::check` deliberately manufactures two other spellings — `judged` is fully resolved, and `path` carries the caller's own last component — so neither is a key `find` is guaranteed to accept. Hand `find` the path the UI sent, keep the `NodeId` through the batch, and patch by id. Getting this wrong is silent: no patch, and the Explorer goes on showing a directory that is gone.
- **One call, not one per path.** `replace_subtrees` rebuilds the whole arena, so its cost is per call: 100 patches in one call measured 120 ms on a 3.7M-node tree, while 100 separate calls would cost ten seconds. Collect every patch of the batch and splice once. Peak memory is roughly double the tree while both arenas exist.
- **Bump the generation on a successful patch, not only compare it.** Every id in the arena changes on a splice, `NodeView` carries `NodeId` across IPC, and the UI caches tree queries keyed by `useScan().generation` with `staleTime: Infinity`. A patch that leaves the generation alone therefore leaves the Explorer holding ids from an arena that no longer exists — the same wrong-tree symptom the splice exists to prevent, arriving from the other side.
- **Splice where a panic is already caught.** The rebuild's ceiling is a live `assert!` in release builds. On the worker side `ScanManager` already recovers from a poisoned lock and turns a worker panic into a failure message, so it degrades to a failed scan rather than a dead window. Never on a `StatusEmitter::emit` path — `CLAUDE.md`'s rule that an emitter must not call back into `ScanManager` points the same way.
- **A patch updates the tree facts of `ScanStatus` and leaves the run facts alone.** `bytes`, `files` and `dirs` describe the tree as it stands — `ScanStats::bytes`'s own doc says it equals the root node's `size` — so recompute them: the first two off the root node, `dirs` with one pass over the new arena, which is noise next to the rebuild. `errors`, `hardlinks_skipped`, `duration_ms` and `started_at` describe the walk that ran, and a patch has nothing to say about them.

  This is not cosmetic. `status.bytes` feeds `DiskUsageBar scanned={status.bytes}` as well as the summary line, and `disk_usage` is re-read after every batch — so stale stats mean the user watches free space grow while the scanned segment of the bar stays put, on the one screen meant to show what they just did.

`actions.rs` holds the glue:

```rust
pub fn run_batch(
    manager: &ScanManager,
    sys: &dyn System,
    log: &ActionLog,
    paths: Vec<PathBuf>,
    mode: Mode,
) -> Outcome
```

It builds `Limits::for_scan_root(manager.root())`, turns the paths into a `Plan` (the kind and size come from the tree when it knows the path, otherwise `NodeKind::Other` and 0), runs `preview`, then `execute`, appends the outcome to the log, patches the tree, and returns the outcome.

Patch the paths of every entry that was `Removed` **or** `Failed`, not just the removed ones: a failed `remove` may have emptied most of a tree before it stopped, and the rescan is what makes the Explorer agree with the disk again. A log failure is printed to stderr and does not fail the batch — the files are already gone.

`commands.rs`:

```rust
#[tauri::command]
pub async fn action_preview(manager: State<'_, ScanManager>, paths: Vec<String>, mode: Mode) -> Result<Preview, String>

#[tauri::command]
pub async fn action_run(manager: State<'_, ScanManager>, paths: Vec<String>, mode: Mode) -> Result<Outcome, String>
```

Both clone what they need and run the body inside `tauri::async_runtime::spawn_blocking`, so the window keeps painting. `action_run` re-plans from the paths and never trusts a preview from the UI. Register both in `lib.rs` next to the scan commands, and add the `RealSystem` and the `ActionLog` to the managed state.

**A rejected `actionRun` and a warning inside a resolved one mean opposite things, and the UI must not render them the same way.** `Err` means the batch **did not run** — the body panicked before doing the work, and the message reads "the batch did not finish". A batch that did run always resolves, and reports afterwards through `recorded: false` (deleted, no record) or `treeStale: true` (deleted, the view is behind). Showing an error for that second pair would tell the user nothing happened at the moment 40 GB went.

That is a **release-build** statement, and a rule rather than a fact: debug assertions in `touched` and in `execute` fire *after* the entries are deleted, so a developer build can reject a batch that ran. Keep it true by keeping it a rule — anything added to `run_batch` after `execute` must not be able to panic out of it, which is why the splice is caught. The first person who adds a step between the deletion and the return breaks the contract the UI is written against, and breaks it silently.

**Step 4: Run the tests**

Run: `cargo test -p storage-monitor-desktop`
Expected: the existing tests plus 4 new ones pass.

**Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add apps/desktop/src-tauri crates/core
git commit -m "feat(desktop): preview and run deletion batches"
```

---

### Task 9: The activity log command

**Files:**

- Modify: `apps/desktop/src-tauri/src/commands.rs`, `apps/desktop/src-tauri/src/lib.rs`

**Step 1: Write the failing test**

In `actions.rs` tests: append two batches through `ActionLog`, call the reading helper with a limit of 1, assert the newest entry comes back and that a missing file yields an empty list rather than an error.

**Step 2: Run to verify failure**

Run: `cargo test -p storage-monitor-desktop activity`

**Step 3: Implement**

```rust
#[tauri::command]
pub fn activity_log(log: State<'_, ActionLog>, limit: Option<usize>) -> Result<LogTail, String>
```

Default limit 100. A **damaged line** is already handled below the command — `tail` skips it and counts it in `LogTail::damaged`. A **read error** is different and must not be flattened into an empty list: "No actions yet" over a log that exists and is full of the user's deletions is the same silent-loss failure `damaged` was added to prevent, one layer up. Surface it, so the screen can say the log could not be read.

**Step 4: Run the tests and commit**

```bash
git add apps/desktop/src-tauri
git commit -m "feat(desktop): expose the action log to the UI"
```

---

### Task 10: Content Security Policy

**Files:**

- Modify: `apps/desktop/src-tauri/tauri.conf.json`

`CLAUDE.md` requires a CSP before the first destructive command ships. Replace `"csp": null` with:

```json
"csp": "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: asset: http://asset.localhost; font-src 'self'; connect-src 'self' ipc: http://ipc.localhost; object-src 'none'; base-uri 'self'; frame-ancestors 'none'"
```

Tailwind injects styles at runtime, hence `'unsafe-inline'` for styles only. `ipc:` and `http://ipc.localhost` are what Tauri 2 uses for its own channel on macOS.

**Verification.** `just dev` **cannot** verify this: Tauri attaches the header only in the `tauri://localhost` handler for the embedded frontend, and with `devUrl` the document comes from Vite and never passes through it. Build the real vehicle instead — `pnpm tauri build --debug --no-bundle`, which is what CI smoke-tests — and run that binary.

And verify with a **positive control**, not with absence: plant an `eval` and confirm it is blocked. Without one, "no violations in the console" is indistinguishable from "no policy at all". Then confirm the treemap really painted (a canvas with opaque pixels, not merely an element), and that a scan crossed IPC under the policy.

**Commit:**

```bash
git add apps/desktop/src-tauri/tauri.conf.json
git commit -m "feat(desktop): set a content security policy"
```

---

### Task 11: The IPC surface and the mock

**Files:**

- Modify: `apps/desktop/src/lib/ipc.ts`, `apps/desktop/src/mocks/ipc.ts`, `apps/desktop/src/mocks/fixtures.ts`
- Test: `apps/desktop/src/mocks/fixtures.test.ts`

**Step 1: Write the failing tests** in `fixtures.test.ts`: previewing a fixture path returns a ready entry with the fixture's size; previewing a path outside `/Users/demo` returns `blocked` with `outsideRoots`; running a batch removes the rows from the fixture tree and shrinks the ancestors; the activity log grows by one entry per batch entry.

**Step 2: Run to verify failure**

Run: `pnpm --filter @storage-monitor/desktop test src/mocks/fixtures.test.ts`

**Step 3: Implement**

`ipc.ts` gains the mirrored types and three wrappers:

```ts
export type Mode = 'trash' | 'permanent';
export type BlockReason =
  | 'outsideRoots'
  | 'denylisted'
  | 'isRoot'
  | 'nested'
  | 'missing'
  | 'unreadable'
  | 'kindChanged'
  | 'malformed';
export type EntryStatus = { state: 'ready' } | { state: 'blocked'; reason: BlockReason };
export type EntryResult =
  | { result: 'removed'; bytes: number }
  | { result: 'failed'; message: string }
  | { result: 'skipped'; reason: BlockReason };

export interface PreviewEntry { path: string; kind: NodeKind; size: number; status: EntryStatus }
export interface Preview { entries: PreviewEntry[]; totalBytes: number; mode: Mode }
export interface EntryOutcome { path: string; kind: NodeKind; result: EntryResult }
export interface Outcome { entries: EntryOutcome[]; freedBytes: number; at: string; mode: Mode }
export interface BatchResult { outcome: Outcome; recorded: boolean; treeStale: boolean }
export interface ActivityEntry { at: string; path: string; kind: NodeKind; mode: Mode; result: 'removed' | 'failed' | 'skipped'; detail: string | null; bytes: number }

export function actionPreview(paths: string[], mode: Mode): Promise<Preview>
export function actionRun(paths: string[], mode: Mode): Promise<BatchResult>
export function activityLog(limit?: number): Promise<{ entries: ActivityEntry[]; damaged: number }>
```

The mock implements the same rules against the fixture, **including the denylist**: `outsideRoots` for anything not under `/Users/demo`, `isRoot` for `/Users/demo` itself, `nested` for a descendant of another entry in the same batch, `missing` for an unknown path, and `denylisted` for `/Users/demo/Library` and everything under it — which is most of the fixture's bulk, DerivedData and Docker.raw included, because the scan root is the home folder and `Limits` keeps `~/Library` denied.

That last one is not a detail. A mock that allows what the backend refuses makes every UI test after this one green against a fiction, and the first real deletion of a DerivedData row comes back refused.

**What the mock can and cannot stage, for Tasks 12–16.**

- `Library` is denied and `Downloads` is ready, and they are the root listing's two largest rows — that pairing is Task 14's "a batch where one entry is blocked still deletes the other". Any test wanting a clean batch picks rows outside `Library`.
- `.Trash` is ready although it carries a scan error, so Task 12's "a locked directory is still selectable" holds.
- The mock cannot produce `failed`, `unreadable`, `kindChanged`, `recorded: false`, `treeStale: true`, or a rejected `activityLog`. `mockActionLog` is exported as raw JSONL lines, so Task 15 can push a `failed` line or a torn one to get `damaged > 0`; a rejected `activityLog` needs a stubbed wrapper.
- **The header total does not shrink by itself.** `scan_status` returns patched totals, but `useScan` reads it once on mount and is otherwise fed by events, and a batch emits none. Take the total from the invalidated root `NodeView` — `treeNode().size`, which the mock does shrink — or add a refresh seam to `ScanController`.
- `treeNode(id)` of a deleted node rejects with `unknown node <id>`, as the arena does after a splice. `actionRun` deletes the nodes from the in-memory fixture, subtracts their sizes from the ancestors and appends to a mock log array that `activityLog` reads. Export a `resetMockActions()` helper for tests, and call it from `src/test/setup.ts` between tests.

**Step 4: Run the tests and commit**

```bash
git add apps/desktop/src
git commit -m "feat(desktop): add the deletion commands to the IPC layer and the mock"
```

---

### Task 12: Selection in the table

**Files:**

- Modify: `apps/desktop/src/components/NodeTable.tsx`
- Test: `apps/desktop/src/components/NodeTable.test.tsx`

**Step 1: Write the failing tests**

Clicking a checkbox selects the row and calls `onSelectionChange` with that id; shift-clicking a second checkbox selects the range between them in the order the table currently shows; the header checkbox selects every visible row and clears them all on the second click; pressing Space on a focused row toggles it without opening the directory; a row with `error` set is still selectable (a locked directory can be deleted even when its contents could not be listed); the selection is not cleared by sorting.

**Step 2: Run to verify failure**

Run: `pnpm --filter @storage-monitor/desktop test src/components/NodeTable.test.tsx`

**Step 3: Implement**

Add to `NodeTableProps` a **paired-optional** pair — both or neither, enforced by a union so half a pair is a compile error:

```ts
  /** Ids of the selected rows. */
  selection: ReadonlySet<NodeId>;
  onSelectionChange: (selection: ReadonlySet<NodeId>) => void;
```

Required props would not compile: `ExplorerPage` is the only call site and Task 14 owns it. Optional also keeps the column invisible until then, which matters because rendering it shifts every positional selector — see the hand-off below.

**The range anchor is an id, not an index.** An index is wrong the moment the table is re-sorted; the id is resolved against the displayed order at click time.

**The header checkbox replaces the selection rather than merging into it.** Select-all reports exactly the shown rows, and clear reports the empty set — even when the incoming selection holds an id the table is not showing. In a deletion UI nothing invisible may survive a "clear", or the action bar goes on counting it and the batch goes on deleting it.

A new first column, width `w-8`, padding `px-2`, holding an `<input type="checkbox">` with an `aria-label` of the row name. The existing `COLUMNS` comment records the fixed widths; update the arithmetic in it (452 px becomes 484 px) so the next reader is not misled. Keep the name column flexible.

Shift-click needs the id of the last row the user touched; hold it in a ref and clear it when the node changes.

**Hand-off, because the checkbox column shifts every positional selector.** When Task 14 renders it, `ExplorerPage.test.tsx`'s `getAllByRole('cell')[0]` becomes `[1]`, and Task 16's e2e selectors `td:first-child span[title]` and `getByRole('cell').nth(3)` become `td:nth-child(2)` and `.nth(4)`. The column is optional precisely so those breakages land in the task that renders it rather than in this one.

**Step 4: Run the tests and commit**

```bash
git add apps/desktop/src
git commit -m "feat(desktop): select rows in the node table"
```

---

### Task 13: The confirmation dialog

**Files:**

- Create: `apps/desktop/src/components/ConfirmDeleteDialog.tsx`
- Test: `apps/desktop/src/components/ConfirmDeleteDialog.test.tsx`

**Step 1: Write the failing tests**

The dialog lists every preview entry with its formatted size; blocked entries are shown with their reason and are visually muted; the total counts only the ready entries; the Trash option explains that space is freed when the Trash is emptied; choosing Permanent disables the confirm button until the "I understand" checkbox is ticked, and switching back to Trash re-enables it and clears the checkbox; Escape calls `onClose` except while the batch runs, when it does nothing; focus lands on Cancel when the dialog opens; the confirm button reports the mode it was confirmed with; the dialog opens in `initialMode` (Trash by default) and never in the mode the preview was computed with; while the batch runs the buttons are disabled and a busy label is shown; when an outcome arrives the dialog switches to the result view, listing failures with their messages **and skipped entries with their reasons** — the run-time skipped set is not the preview's blocked set, because `missing` and `kindChanged` are decided while the batch runs, so a view that lists only failures says "Moved 2 items" while a third is still on disk — and — because a batch can succeed at deleting and still fail to tell anyone — saying so when `recorded` is false ("deleted, but not recorded") and when `treeStale` is true ("the Explorer may be stale until the next scan"). Neither is an error in the deletion; both are the app admitting the record or the view no longer matches the disk.

Use `fireEvent` for the keyboard assertions, as `NodeTable.test.tsx` does. (An earlier draft said `@testing-library/user-event` ships with `@testing-library/dom`; the dependency runs the other way and the package is in neither `package.json` nor `pnpm-lock.yaml`. The only keyboard behaviour that belongs to this component is the trap's boundary handler — `preventDefault` plus an explicit `focus()` — and `fireEvent` drives that exactly. Know the cost: `fireEvent` moves no focus, so a test can pin the boundary but not that Tab walks the tab order, and a radio group's single tab stop is invisible to the suite.)

**Step 2: Run to verify failure**

Run: `pnpm --filter @storage-monitor/desktop test src/components/ConfirmDeleteDialog.test.tsx`

**Step 3: Implement**

A controlled component: `{ preview: Omit<Preview, 'mode'>, status, initialMode?, onConfirm(mode), onClose }`, where

```ts
type BatchStatus =
  | { phase: 'asking' }
  | { phase: 'running' }
  | { phase: 'done'; result: BatchResult }
  | { phase: 'failed'; message: string };
```

An earlier draft said `{ preview, outcome, busy, onConfirm, onCancel }`. That shape cannot work: `recorded` and `treeStale` live on `BatchResult`, outside `Outcome`, so the result view could not reach the two things it exists to admit; a rejected `action_run` had no arm at all; and flat props admit `busy && outcome`. The union makes those unrepresentable. `Omit<Preview, 'mode'>` is deliberate too — it turns reading `preview.mode` into a compile error, so the dialog cannot render "Items move to the Trash" over a button armed to delete permanently, which is the worst failure this component has. `onClose` is one callback in all phases and says nothing about whether a batch ran; the caller knows that from its own state.

Markup: a fixed overlay, a panel with `role="dialog"`, `aria-modal="true"` and `aria-labelledby`. Trap focus by cycling Tab between the first and last focusable element; return focus to the trigger on close (the caller passes nothing — use `document.activeElement` captured on mount). Three things the trap gets wrong if written the obvious way: the focusable list must be read per key press, not once at mount, because the acknowledgement checkbox appears and disappears and buttons become disabled; a disabled element is not focusable and must not be a boundary; and a radio group has exactly one tab stop, the checked radio, so an unchecked one must not be `focusable[0]`. Put the key listener on the document rather than the panel — a click on the overlay blurs to `<body>`, and a panel listener then stops hearing Escape at all.

Wording, in English as everywhere in the UI:

- Trash: "Items move to the Trash. Space is freed when you empty it."
- Permanent: "Items are deleted immediately. This cannot be undone."
- Result view: "Moved 12 items to the Trash · 4.3 GB" or "Deleted 12 items · 4.3 GB" — counting the `removed` entries only, never `entries.length`, which overstates what left the disk in the same sentence that reports bytes. Never "Put Back", which macOS may not offer, and **no "Show in Trash" button**: an earlier draft promised one, and design section 11 now records why phase 2a ships without it. `move_to_trash` returns no post-move URL, so there is no path to reveal; opening the Trash folder needs `opener:allow-open-path`, a real privilege grant. Do not wire up a per-item reveal either.

**Step 4: Run the tests and commit**

```bash
git add apps/desktop/src
git commit -m "feat(desktop): add the deletion confirmation dialog"
```

---

### Task 14: Wiring the Explorer

**Files:**

- Modify: `apps/desktop/src/pages/ExplorerPage.tsx`
- Test: `apps/desktop/src/pages/ExplorerPage.test.tsx`

**Step 1: Write the failing tests**

Selecting two rows shows the action bar with the count and the summed size; "Move to Trash" opens the dialog with a preview of exactly those paths; "Delete permanently" opens the same dialog already in Permanent mode, with its confirm button still disabled until the acknowledgement; confirming removes the rows from the table and clears the selection; the ancestors' sizes in the breadcrumbs shrink; cancelling leaves everything alone; a batch where one entry is blocked still deletes the other and the result view names the blocked one; switching directories clears the selection; a rescan (new generation) clears the selection.

**Step 2: Run to verify failure**

Run: `pnpm --filter @storage-monitor/desktop test src/pages/ExplorerPage.test.tsx`

**Step 3: Implement**

The page holds `selection` and the dialog state.

**Drop the selection on a new generation, not only on a navigation.** A `NodeId` means nothing across a scan or a splice — `ScanManager` bumps its generation for exactly that reason, "because both replace every id in it". A selection that survives a rescan silently points at different files: measured in `NodeTable`, selecting the sixth row and rescanning leaves the row that took id 6 ticked, and a batch built from `${view.path}/${child.name}` then deletes it. The table drops its range anchor on the same signal; the selection is the page's half of the same rule.

**Decide the grid question here or in Task 16, and record it.** The table is an implicit `table` whose rows already carry `tabIndex` and arrow navigation — it behaves like a grid without saying so, which is why a selected row cannot carry `aria-selected` today. Promoting it to `role="grid"` with `aria-multiselectable` is the accessible answer and moves the e2e selectors a second time, so it belongs in whichever of these two tasks takes it, deliberately, rather than drifting past both.

Paths are built from the current node: `` `${view.path}/${child.name}` `` — `ChildView` carries no path of its own.

After a successful run, invalidate the tree queries of the current generation and the disk usage query so both refetch; do not bump the generation, since the scan itself did not change. The action bar sits between the breadcrumbs and the table, and disappears when the selection empties.

**That invalidation is not optional, and nothing upstream does it for you.** The generation the backend bumps after a splice is `Inner.generation`, which never crosses the wire — `ScanStatus` has no field for it, and `useScan().generation` is a front-end counter bumped by `scan:done`. Its only job is to stop one patch being spliced onto an arena another patch already replaced. So when `actionRun` resolves, every `['treeNode', generation, id]` entry in the cache still holds `NodeId`s from the arena the splice threw away: the Explorer goes on rendering a tree that no longer exists, which is the symptom this whole phase is built to avoid, arriving through the cache instead of through the splice.

**Do not let a second batch start while one is running.** Two overlapping `action_run` calls end with the second patch dropped by the generation check, so that batch's rows stay in the tree until the next scan. Nothing is lost or mis-spliced — it is the documented behaviour of `patch_paths` step 3 — but it is avoidable, and **this page is what avoids it**. An earlier draft said the dialog rules it out "by staying busy until its outcome arrives"; it cannot. The dialog is controlled: it renders whatever `status` it is handed, so the guard is the page's mutation state, not the component's.

**The preview is this page's to wait for and to fail.** `ConfirmDeleteDialog` takes a resolved `preview` as a required prop and renders no pending state, so the click on "Move to Trash" and the dialog appearing are separated by a round trip the user gets no feedback about. `action_preview` returns a `Result`, so it can reject, and that rejection has no home in `BatchStatus` — `failed` there means the batch failed, which is a different and much more alarming sentence. Surface both here: something between the click and the dialog, and an error path for a preview that never arrives.

**The action bar has two entry points, and both must land honestly.** "Move to Trash" and "Delete permanently" (danger style) open the same dialog with `initialMode` set accordingly. The safety gate does not move: Permanent still needs the "I understand" tick before its confirm button enables, so the danger button opens a dialog that is armed for nothing yet. Passing no `initialMode` gives Trash, which is the default the rest of the app assumes.

**Step 4: Run the tests and commit**

```bash
git add apps/desktop/src
git commit -m "feat(desktop): delete selected entries from the Explorer"
```

---

### Task 15: The Activity page

**Files:**

- Create: `apps/desktop/src/pages/ActivityPage.tsx`
- Modify: `apps/desktop/src/lib/pages.ts`, the shell that routes to the pages
- Test: `apps/desktop/src/pages/ActivityPage.test.tsx`

**Step 1: Write the failing tests**

An empty log renders "No actions yet"; entries render newest first with a formatted time, the path, the mode and the size; a failed entry shows its message; a row's path is the one the guards normalized, which under a symlinked scan root is **not** the spelling the Explorer showed — so do not try to match an Activity row back to a tree row by string; a **skipped** entry shows its reason, mapped from `detail` — and the map reads `result` first, because `detail` carries a failure message for `Failed` and a block reason for `Skipped`, so a failure whose message reads `denylisted` is not a blocked entry; a non-zero `damaged` count renders as "N damaged entries hidden", never silently; a log that could not be read renders as an error rather than as an empty list; the page refetches when it is opened after a batch.

**Step 2: Run to verify failure**

Run: `pnpm --filter @storage-monitor/desktop test src/pages/ActivityPage.test.tsx`

**Step 3: Implement**

Flip `available` to `true` for `activity` in `pages.ts` and route to the new page. A `useQuery` over `activityLog(200)` with `staleTime: 0`. Reuse `formatBytes` and `formatDate` from `src/lib/format.ts`; add a time-of-day format there if the date alone is not enough, with its own unit test.

**Step 4: Run the tests and commit**

```bash
git add apps/desktop/src
git commit -m "feat(desktop): add the Activity screen"
```

---

### Task 16: End to end

**Files:**

- Modify: `apps/desktop/e2e/explorer.spec.ts`
- Create: `apps/desktop/e2e/activity.spec.ts`

**Step 1: Write the failing tests**

In `explorer.spec.ts`: select two rows, click "Move to Trash", assert the dialog lists both with the total, confirm, and assert both rows disappear while the header total shrinks. Take `explorer-selection.png` while the dialog is open.

**Serve the app's own CSP from the mock server, and fail a spec on any violation.** This is the only place in the project where the policy can be exercised by a test: `just dev` does not apply it (Tauri attaches the header in the `tauri://localhost` handler, which a `devUrl` document never reaches), and CI's `tauri build` compiles the binary without launching it. Set `Content-Security-Policy` as a response header on port 1430 — the same string as `tauri.conf.json` — and register a `securitypolicyviolation` listener that fails the test.

The origin differs, so `'self'` does not mean the same thing here. What transfers is the class that actually breaks: `'unsafe-eval'`, a `blob:` worker, a `data:` font. That matters because `echarts` is a caret range — a `pnpm update` pulling a chart build that uses a blob worker would ship an app whose treemap silently fails to render, with every existing test green.

In `activity.spec.ts`: run a batch, open Activity from the sidebar, assert two entries with the right paths, and write `activity.png`.

**Step 2: Run**

Run: `just e2e`
Expected: the existing nine tests plus the new ones pass; `apps/desktop/test-results/` holds `explorer.png`, `explorer-dark.png`, `home.png`, `explorer-selection.png` and `activity.png`.

**Step 3: Commit**

```bash
git add apps/desktop/e2e
git commit -m "test(desktop): cover deletion and the Activity screen end to end"
```

---

### Task 17: Documentation and the pull request

**Files:**

- Create: `docs/adr/0005-patching-the-scan-tree.md`, `docs/adr/0006-content-security-policy.md`
- Modify: `CLAUDE.md`, `README.md`, `docs/plans/2026-09-17-storage-monitor-design.md`, `docs/images/`

**Step 1: The ADR**

`0005-patching-the-scan-tree.md`, following the shape of the existing four. Context: a finished batch leaves the in-memory tree stale and a full rescan of the home folder costs 25 seconds. Decision: rescan the deleted paths only and rebuild the arena around the results; the snapshot is not rewritten. Consequences: hard-link attribution is not recomputed, so twins of a deleted hard link report 0 until the next full scan; the deltas keep pointing at the previous snapshot and the next scan shows the deletion as negative growth.

`0006-content-security-policy.md` takes the prose that has outgrown the guard test: why `style-src` keeps `'unsafe-inline'` (inline style *attributes* in the disk bar, the table's column widths and the ECharts tooltip — not Tailwind, which compiles at build time), what makes that grant tolerable (`img-src 'self'` leaves an injected `url()` nowhere to beacon, `font-src 'self'` closes the same door for `@font-face`), why `require-trusted-types-for` is omitted deliberately, and why nothing but a manual run and the Playwright header can exercise the policy at all. Leave `the_content_security_policy_stays_closed` pointing at it — the test is where a future editor lands, the ADR is where the argument belongs.

**Step 2: `CLAUDE.md`**

Add `crates/core/src/system/` and `crates/core/src/action/` to the layout, `apps/desktop/src-tauri/src/actions.rs`, and the new UI files. Extend the Data section with `actions.jsonl`: where it lives, that it is created `0o600` because it names every path the user has ever deleted, that a batch is one append and `Ok` means the bytes reached the volume, and that — unlike snapshots, which are pruned to ten — nothing prunes it. Add to Conventions: deletion goes through `System`, never through `std::fs` directly; the CSP is set (drop the line that says it is still `null`).

**Step 3: `README.md`**

Extend "What it does today" with deletion in both modes and the Activity screen; add the `activity.png` screenshot next to `explorer.png`.

**Step 4: The design roadmap**

In section 14 split the phase 2 row into 2a (this slice, done) and 2b (module framework, Cleanup, Settings, process execution).

**Step 5: Full check**

```bash
just ci
just clippy-ci
```

Both must be green. `just ci` covers fmt, clippy, typecheck, eslint, prettier, cargo test, vitest, the web build and Playwright.

**Step 6: Verify on the real machine**

This is the first destructive code in the project, so exercise it by hand before the PR:

1. `just dev`, scan the home folder.
2. Create `~/storage-monitor-scratch/` with a few hundred megabytes of junk, rescan, select it, delete it in Trash mode. Confirm: the row disappears, the parent shrinks, the folder is in the Trash, Activity lists it, and `~/Library/Application Support/storage-monitor/actions.jsonl` has the line.
3. Restore it from the Trash, rescan, and delete it again in Permanent mode. Confirm the free space in the header grows and the folder is gone.
4. Try to delete `~/Library` and the scan root itself: both must come back blocked with a reason, before anything is touched.
5. Note the wall-clock time of the tree patch for a large directory in the plan's Outcome section.

**Step 7: The pull request**

```bash
git push -u origin feat/phase-2a-deletion
gh pr create --title "feat: delete from the Explorer (phase 2a)" --body "..."
```

**The squash message carries what the individual commits did not.** At least one commit on this branch describes only part of its own diff — `8c38b90` is the CSP guard and the widened registration test by its message, and also the `activity` → `activity_tail` rename and a round of doc work. Read the full diff when writing the squash body, not the commit subjects, or a reviewer meets those changes unannounced.

```bash
# (the command above)
```

The body carries the test plan, the manual verification above, and the screenshots. Wait for green CI, self-review the diff, then squash-merge. release-please will open the release PR for `0.3.0` (a `feat` commit bumps the minor); close and reopen it once so CI runs, check that the diff is versions and changelog only, merge it, and verify the built assets like in the previous phases.
