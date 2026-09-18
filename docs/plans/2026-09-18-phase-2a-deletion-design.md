# Phase 2a: deleting from the app

- Date: 2026-09-18
- Status: Accepted
- Scope: core action engine, `System` port, Explorer deletion, Activity screen

## 1. Problem

The app finds space but cannot free it. Everything shipped in `v0.2.0` is
read-only: scan, tree queries, disk usage, growth deltas, "Reveal in Finder".
A user who sees 40 GB of stale build output has to leave for Finder.

The roadmap answers this with phase 2 as a whole: module framework, action
engine, and the Cleanup, Activity and Settings screens. That is a large step,
and none of it deletes anything until the very end.

## 2. Goal

Delete arbitrary files and folders from the Explorer, safely, with the action
engine that the modules will reuse. Phase 2 is split in two:

- **2a (this design)**: `System` port, action engine, deletion in the Explorer,
  action log and a minimal Activity screen.
- **2b**: module contract, a dummy module, the Cleanup and Settings screens,
  process execution in the `System` port.

Splitting this way puts the destructive code on real targets early, while the
batches are small and the user picks every path by hand.

## 3. Decisions

1. **The `System` port carries filesystem and clock only.** Process execution
   has no consumer until the first module in 2b.
2. **Both modes ship now** (ADR 0003): Trash by default, Permanent as an
   explicit choice in the confirmation dialog. Until Settings exists in 2b, the
   choice is per batch and is not remembered.
3. **Multi-select and batches**, not one row at a time. The engine takes a list
   of entries from the start, so Cleanup in 2b reuses it unchanged.
4. **After a batch, only the deleted paths are rescanned** — not their parents,
   and not the whole root. Rescanning the parent of a folder deleted directly
   under `~` would mean a full 25-second rescan of the home folder.
5. **The arena is rebuilt** to splice the rescanned subtrees in. Children live
   in contiguous `Range<NodeId>` slots, so in-place surgery is not possible.
6. **The Activity screen is a list**, no filters and no pagination.

## 4. The `System` port

```rust
pub trait System: Send + Sync {
    /// Metadata that does not follow symlinks.
    fn symlink_metadata(&self, path: &Path) -> Result<Metadata, SystemError>;
    /// Move to the macOS Trash (section 11).
    fn move_to_trash(&self, path: &Path) -> Result<(), SystemError>;
    /// Delete a file, a symlink or a whole tree. Symlinks are removed as links.
    fn remove(&self, path: &Path) -> Result<(), SystemError>;
    fn now(&self) -> DateTime<Utc>;
}
```

`RealSystem` performs the real operations. `TestSystem` runs the same standard
library calls against a temporary filesystem — `fs::Metadata` cannot be
constructed by hand, so faking it is not an option — but redirects the Trash
into a temp directory and returns a fixed clock. Engine tests therefore pass on
the Linux CI runners, which have no macOS Trash.

`symlink_metadata` belongs in the port because the engine re-validates every
entry immediately before deleting it (section 5). Without the indirection, the
race between preview and execution cannot be reproduced in a test.

Two invariants live in the port itself rather than only in the guards above it,
because it is the single door through which deletion passes:

- **Paths are absolute and normalized.** A path carrying a `..` component is
  refused. `symlink_metadata` does not follow the last component, but the kernel
  always resolves a trailing `..`, so `remove("/a/b/c/..")` would otherwise stat
  `/a/b`, find a directory, and empty it — deleting siblings nobody named, and
  reporting success.
- **`remove` is not atomic.** `fs::remove_dir_all` can delete most of a tree and
  then fail. The engine copes through the rescan of section 6; what the port
  owes its caller is to say so rather than imply all-or-nothing.

`TestSystem` enforces its own confinement: a path outside its temporary
directory makes it panic rather than return an error, so a test written against
a broken guard cannot quietly delete the developer's home folder. The comparison
is component-wise and against canonicalized forms — on macOS a temp directory is
`/var/folders/…` while its canonical form is `/private/var/folders/…`.

## 5. The action engine

```rust
pub enum Mode { Trash, Permanent }

pub struct PlanEntry { pub path: PathBuf, pub kind: NodeKind, pub size: u64 }
pub struct Plan { pub entries: Vec<PlanEntry>, pub mode: Mode }

pub enum BlockReason { OutsideRoots, Denylisted, IsRoot, Nested, Missing, KindChanged }
pub enum EntryStatus { Ready, Blocked(BlockReason) }
pub struct PreviewEntry { pub path: PathBuf, pub kind: NodeKind, pub size: u64, pub status: EntryStatus }
pub struct Preview { pub entries: Vec<PreviewEntry>, pub total_bytes: u64, pub mode: Mode }

pub enum EntryResult { Removed { bytes: u64 }, Failed(String), Skipped(BlockReason) }
pub struct Outcome { pub entries: Vec<EntryOutcome>, pub freed_bytes: u64, pub at: DateTime<Utc> }
```

`plan → preview → execute`, as in design section 9. `preview` has no side
effects; `execute` takes a checked `Preview` and touches only `Ready` entries.

Guards live in `guards.rs` as pure functions:

- **Normalization.** The *parent* is canonicalized and the last component
  appended. Canonicalizing the whole path would resolve a symlink to its target
  and delete the wrong thing; a symlink is always removed as a link.
- **Allowed roots.** An entry must sit inside the scan root. The root itself and
  any of its ancestors are refused.
- **Denylist.** `/`, `/System`, `/usr`, `/bin`, `/sbin`, `/Library`,
  `~/Library`, and the home folder itself.
- **Nesting.** When a batch holds both `a/` and `a/b`, the descendant is
  dropped: otherwise its bytes are counted twice and its deletion fails with
  "no such file".
- **Re-validation at execution time.** `symlink_metadata` again: the kind must
  still match the plan, or the entry becomes `Skipped(KindChanged)`. A changed
  size is fine — the disk keeps living.

A failing entry does not abort the batch. Every entry, successful or not, is
appended to `~/Library/Application Support/storage-monitor/actions.jsonl`
through `paths::actions_log()`: timestamp, path, kind, mode, result, bytes.
One JSON object per line, appended with a single write; a damaged line is
skipped when reading instead of failing the screen.

In Trash mode `freed_bytes` is what *will* be freed once the Trash is emptied,
and the UI says exactly that (ADR 0003).

## 6. Patching the tree

```rust
/// A fresh subtree for `path`; `None` when the path is gone.
pub fn rescan_path(path: &Path, options: &ScanOptions) -> Result<Option<Subtree>, ScanError>

/// A new arena with the given nodes replaced (or dropped) and ancestors reaggregated.
pub fn replace_subtrees(tree: &Tree, patches: &[(NodeId, Option<Subtree>)]) -> Tree
```

Three outcomes are covered: the path is gone (the node is dropped), a file or
symlink remains, or a directory remains with whatever could not be deleted —
which lands in the tree as the exact remainder.

The rebuild copies the arena: 3.7M nodes of 64 B is roughly 240 MB and a few
hundred milliseconds, with a transient doubling of that memory. In exchange
every invariant — contiguous children, BFS order, the side table of errors —
holds by construction.

**Known limitation.** Hard-link attribution runs once, after the full walk. If
a deleted path owned the bytes of a hard-linked file, its twins keep reporting
0 until the next full scan. Re-attribution would mean keeping all `(dev, ino)`
pairs of the tree — 3.7M entries for a rare case.

The snapshot is not rewritten and the deltas are left alone: they describe
growth since the previous snapshot, and the next scan reports the deletion
honestly as negative growth. `disk_usage` is re-read after every batch, since
Permanent mode really does change the free space.

## 7. Desktop

Three commands, thin over the core, with camelCase views as usual:

```ts
actionPreview(paths: string[], mode: Mode): Promise<Preview>
actionRun(paths: string[], mode: Mode): Promise<Outcome>
activityLog(limit?: number): Promise<ActionLogEntry[]>
```

`actionRun` takes paths and a mode, not a finished `Preview`: the backend
re-plans and re-checks the guards itself. A forged preview must not be able to
walk around the denylist.

Both action commands are `async fn` whose body runs in `spawn_blocking`, so the
window stays responsive. No progress events in 2a: a batch here is a handful of
entries. Progress arrives in 2b with Cleanup.

The Content Security Policy in `tauri.conf.json`, still `null` since phase 0,
is set in this slice — `CLAUDE.md` requires it before the first destructive
command ships. Capabilities do not change: deletion is our own Rust code, not a
Tauri plugin.

## 8. UI

`NodeTable` gains a checkbox column. The selection is a set of `NodeId` held by
`ExplorerPage`, cleared when the directory changes and when the scan generation
changes. Space toggles the focused row, shift-click takes a range, the header
checkbox selects everything visible.

A non-empty selection reveals an action bar above the table: "12 selected ·
4.3 GB", "Move to Trash", and "Delete permanently" in a danger style.

The confirmation dialog is our own component (Radix is not a dependency):
`role="dialog"`, `aria-modal`, a focus trap, Escape closes, and the initial
focus is on Cancel rather than on the confirming button. It lists the entries
with their sizes, greys out blocked ones with the reason, totals the bytes, and
carries the mode switch. Trash mode says space is freed only when the Trash is
emptied; Permanent mode says the deletion cannot be undone and keeps its button
disabled until an "I understand" checkbox is ticked.

The wording for Trash mode points at the Trash itself rather than at "Put
Back", for the reason in section 11.

When the batch finishes, the same dialog reports what happened: entries
deleted, bytes freed, and every failure with its reason.

The Activity page replaces its placeholder with a table of the most recent log
entries — time, path, mode, result, bytes — and an empty state.

## 9. Testing

Rust unit tests: each guard rule separately (a symlink is not resolved, the
scan root and `~` are refused, the denylist, nested entries collapse); the
engine (one entry fails and the rest still run, a changed kind becomes
`Skipped`, both modes); the log (append, tail, a damaged line); `rescan_path`
on its three outcomes; `replace_subtrees` on ancestor aggregates and on the
contiguity of children.

Everything runs on `TestSystem` over a temporary filesystem with the Trash
redirected, so the suite is green on Linux.

Desktop tests use `STORAGE_MONITOR_DATA_DIR` and a temporary snapshot
directory, and assert that the tree is patched and `disk_usage` re-read after a
batch.

Vitest covers the selection logic, the dialog (blocked entries, the disabled
button without the checkbox, Escape and the focus trap), the Activity page and
the three mocked commands. Playwright: select two rows, move them to the Trash,
confirm, and watch the rows disappear while the ancestors shrink and Activity
gains two entries. The PR carries `explorer.png` and a new `activity.png`.

## 10. Out of scope

No undo inside the app — Finder's "Put Back" is the undo. No Cleanup or
Settings screen, no module contract, no process execution, no batch progress
events. All of that is 2b.

## 11. How files reach the Trash

`move_to_trash` uses the `trash` crate (5.2.9, MIT) with the delete method set
explicitly to `NsFileManager`, which calls `NSFileManager.trashItemAtURL:`
through objc2.

The crate offers two methods on macOS and neither is free of trouble. Its
default, `Finder`, runs `osascript -e 'tell application "Finder" to delete
{...}'`; that is the only way to get a reliable "Put Back" entry, but it needs
the Automation permission for Finder. Our app is ad-hoc signed, and TCC keys a
grant to the code directory hash (see the Full Disk Access design), so that
grant would silently lapse with every new build and deletion would start
failing. It also plays the Finder sound and is slow for a batch.

`NsFileManager` needs no extra permission, is fast and silent, and survives
rebuilds. Its documented cost is that "Put Back" may be missing on some
systems — a macOS bug the crate links to. Files still land in the Trash and can
be restored by dragging them out, so the UI offers "Show in Trash" and never
promises "Put Back".

The crate is preferred over calling objc2 directly because it already handles
non-UTF-8 paths (percent-encoding), maps the Cocoa error, and — unlike a
`#[cfg(target_os = "macos")]` block of our own — compiles on the Linux CI
runners, where `RealSystem` is built but never exercised.
