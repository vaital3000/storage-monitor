//! The stages a deletion goes through. [`preview`] only decides: it reads the filesystem
//! and writes nothing, so a preview can be built, shown and thrown away at will.
//! [`execute`] is the other one, and the only code in the app that destroys anything.
//!
//! Every entry of the plan comes back, in its place, whether or not it survived the checks
//! — the dialog lists what the user selected next to what will happen to each row, and a
//! silently shortened list would be a dialog that lies about the selection. The outcome
//! keeps that shape one stage further, where it becomes the action log.

use std::path::PathBuf;

use crate::scan::NodeKind;
use crate::system::{System, SystemError};

use super::guards::{Checked, Limits, drop_nested};
use super::model::{
    BlockReason, EntryOutcome, EntryResult, EntryStatus, Mode, Outcome, Plan, PlanEntry, Preview,
    PreviewEntry,
};

/// Checks a plan without touching anything. Every entry keeps its place in the list, so
/// the UI can show blocked ones with their reason.
pub fn preview(plan: &Plan, limits: &Limits, sys: &dyn System) -> Preview {
    let mut entries: Vec<PreviewEntry> = Vec::with_capacity(plan.entries.len());
    // Where each still-ready entry sits in `entries`, and the form to compare it by.
    let mut ready: Vec<(usize, PathBuf)> = Vec::new();
    for planned in &plan.entries {
        match check_entry(planned, limits, sys) {
            Verdict::Ready { entry, judged } => {
                ready.push((entries.len(), judged));
                entries.push(entry);
            }
            Verdict::Blocked(entry) => entries.push(entry),
        }
    }

    // Only the entries that are still ready take part: an entry that will not be deleted
    // cannot swallow the one below it. Selecting the root row together with a file inside
    // it is one click in a tree view, and that file still has to be deletable.
    //
    // They are compared by `Checked::judged`, never by the path that gets deleted. That one
    // keeps the caller's spelling of the last component — deliberately, since a symlink must
    // not resolve to its target — and two spellings of one directory would then look like
    // two entries: `Data` and `data`, or one name in NFC and in NFD, are a single directory
    // on a stock macOS volume. Comparing the deleted form would promise those bytes twice
    // and then fail to delete whichever came second. `drop_nested` cannot catch the
    // substitution itself: its `debug_assert` weighs absoluteness and nothing else, and says
    // nothing at all in a release build.
    //
    // An exact duplicate lands here too, as `Nested` rather than a reason of its own: the
    // dialog then says "another entry contains it" about a row that contains nothing, which
    // is the wrong word for the right verdict. A `Duplicate` reason would be a wire change,
    // a new arm in every consumer and a new string to translate, for one cosmetic line in a
    // case the user reaches by selecting one row twice. Deliberate; revisit it if the
    // Activity screen ever has to explain the difference.
    let judged: Vec<PathBuf> = ready.iter().map(|(_, judged)| judged.clone()).collect();
    for (&(index, _), kept) in ready.iter().zip(drop_nested(&judged)) {
        if !kept {
            entries[index].status = EntryStatus::Blocked(BlockReason::Nested);
        }
    }

    let total_bytes = entries
        .iter()
        .filter(|entry| entry.status == EntryStatus::Ready)
        // Saturating because a promise of free space is not worth a panic; the real sizes
        // of one selection cannot come near the limit.
        .fold(0u64, |sum, entry| sum.saturating_add(entry.size));
    Preview {
        entries,
        total_bytes,
        mode: plan.mode,
    }
}

/// The verdict on one entry before the batch is looked at as a whole.
enum Verdict {
    /// Still a candidate, with the form the batch comparison needs. That form has no place
    /// on [`PreviewEntry`]: it crosses IPC, and nothing outside this file may delete by it.
    Ready {
        entry: PreviewEntry,
        judged: PathBuf,
    },
    /// Refused by the guards or by the disk, and so out of the comparison entirely.
    Blocked(PreviewEntry),
}

/// One entry against the guards and the disk, before the batch is looked at as a whole.
///
/// The size is the plan's throughout — the number the Explorer showed the user, which is
/// what the dialog is about — while the kind is re-read for the entries that get that far.
/// Only that one is worth a syscall: a stale size costs nothing, and a stale kind deletes
/// the wrong thing.
fn check_entry(entry: &PlanEntry, limits: &Limits, sys: &dyn System) -> Verdict {
    let blocked = |path: PathBuf, reason| {
        Verdict::Blocked(PreviewEntry {
            path,
            // Nothing was read, so the plan's claim about the kind is all there is. It is
            // unverified, and `PreviewEntry::kind` says so.
            kind: entry.kind,
            size: entry.size,
            status: EntryStatus::Blocked(reason),
        })
    };
    // Reports `Missing`, `Unreadable` and `Malformed` besides the placement rules, and each
    // of them means something different to the person reading the dialog. Pass them on.
    let checked = match limits.check(&entry.path) {
        Ok(checked) => checked,
        // Without a normalized path the only honest thing to show is what was asked for.
        Err(reason) => return blocked(entry.path.clone(), reason),
    };
    // `checked.path`, never `checked.judged`. Nothing enforces that: the two are both
    // absolute, both in normal form, and name the same inode, so the substitution compiles,
    // passes clippy and passes every test here — only a staged race could tell them apart.
    // The reason is that the port must be asked about the form that will be deleted, so the
    // kind it reports is the kind the re-validation before the deletion compares against.
    match sys.symlink_metadata(&checked.path) {
        Ok(meta) => Verdict::Ready {
            entry: PreviewEntry {
                path: checked.path,
                // The walker's own classifier, so that a socket or a fifo is `Other` here
                // as well. A private `if is_dir { Dir } else { File }` would disagree with
                // the scan about every such entry, and the re-validation before the
                // deletion would refuse them all as `KindChanged`, for ever.
                kind: NodeKind::from_metadata(&meta),
                size: entry.size,
                status: EntryStatus::Ready,
            },
            judged: checked.judged,
        },
        Err(err) => blocked(checked.path, reason_for(&err)),
    }
}

/// What the port's refusal to describe an entry means for that entry: the one mapping both
/// stages use, so that a dialog and the log after it cannot blame different things for one
/// entry that never changed in between.
///
/// About [`System::symlink_metadata`] only. A deletion that fails is answered in `run_entry`
/// instead: the port's own message in [`EntryResult::Failed`], or `Skipped` when the error
/// says the entry was already gone and so nothing was destroyed.
fn reason_for(err: &SystemError) -> BlockReason {
    match err {
        // The guards judged where the path points, without insisting anything is there.
        SystemError::Missing(_) => BlockReason::Missing,
        // The port refuses a path that is not absolute and in normal form, and `check` is
        // what produces that form — so this is a bug on this side of the port, not a state
        // the user can do anything about. `Malformed` is at least the truth about the path;
        // `Unreadable` below means "grant Full Disk Access", which would be an instruction
        // to go and fix the wrong thing.
        SystemError::Rejected { .. } => BlockReason::Malformed,
        // Permissions that changed under us, a loop, a variant added later: whatever it is,
        // nobody can look at this entry, and that is not one to delete on a guess.
        _ => BlockReason::Unreadable,
    }
}

/// Runs a checked preview: the one function in the app that deletes anything.
///
/// Blocked entries are reported as skipped and never touched. A failure does not stop the
/// rest of the batch — the entries are independent, and giving up halfway would leave the
/// user with a list where nothing says which half ran.
///
/// The guards run again on every ready entry, not only the kind comparison of the design.
/// [`Preview`] has public fields and derives `Deserialize`, so one can be built without ever
/// having passed them; re-checking here means a forged preview buys nothing, and every
/// deletion is guarded where it happens instead of by a promise made upstream. A path the
/// guards now refuse comes back as `Skipped` with the reason they gave.
///
/// An entry that goes away while the batch runs is `Skipped { reason: Missing }` and not a
/// failure: this app destroyed nothing. That holds for as much of the race as the port can
/// see, which is most of it — both deletions of [`crate::system::RealSystem`] stat the entry
/// first, and that is what turns the usual vanish into `Missing`. It does not hold for the
/// sliver after that stat: `remove` reports its own not-found error there, and the Trash an
/// opaque Cocoa string, and both land as `Failed`. Narrowed, not closed.
///
/// In [`Mode::Trash`] the freed bytes are what *will* be freed once the Trash is emptied
/// (ADR 0003); the number is the same and only the wording in the UI differs.
///
/// There is no cancellation seam, deliberately, in phase 2a. A batch runs to its end, and a
/// single `remove` of a 50 GB tree is one call into the port that nothing here can interrupt
/// — a caller on a worker thread cannot offer a Stop button that does anything. The place to
/// add one is the port, not this loop, and it is not needed until a module deletes thousands
/// of entries at once.
pub fn execute(preview: &Preview, limits: &Limits, sys: &dyn System) -> Outcome {
    // One instant for the whole batch, read before the first deletion: the log timestamps
    // an action, not an entry, and this is when the action began.
    let at = sys.now();
    let mut entries: Vec<EntryOutcome> = Vec::with_capacity(preview.entries.len());
    let mut freed_bytes = 0u64;
    // No nesting pass here, deliberately. `preview` has already blocked every entry another
    // one contains, and running `drop_nested` again would mean carrying the judged forms
    // through this stage too — the form that must never be deleted, next to the one that
    // must, one stage further from the comment that says so. What a missing pass costs is
    // bounded: the entries are checked one by one, so a forged preview holding both an
    // ancestor and its descendant still deletes nothing the guards refuse. Whichever comes
    // first is removed, and the other is then `Missing` — unless the descendant came first,
    // in which case its bytes are counted inside its ancestor's as well. A number in that
    // preview's own log, wrong only for a preview that was already forged.
    //
    // If that ever changes — a nesting pass here, or anything else that needs `judged` in
    // this stage — write the newtype that day: a `Judged(PathBuf)` the port cannot be handed
    // at all. Today the two forms are told apart only by a volume that folds two spellings
    // into one name, which is every Mac and no Linux runner; the moment `judged` is in scope
    // next to the deletion, that is too thin a thread to hang it on.
    for entry in &preview.entries {
        let outcome = run_entry(entry, preview.mode, limits, sys);
        if let EntryResult::Removed { bytes } = &outcome.result {
            // Saturating like `preview`: a wrong total is not worth a panic, least of all in
            // the middle of a batch that is already deleting.
            freed_bytes = freed_bytes.saturating_add(*bytes);
        }
        entries.push(outcome);
    }
    // The one invariant a reader of the log depends on, and one public field plus
    // `Deserialize` away from being representable: the total is the entries, nothing else.
    debug_assert_eq!(
        freed_bytes,
        entries.iter().fold(0u64, |sum, entry| match entry.result {
            EntryResult::Removed { bytes } => sum.saturating_add(bytes),
            _ => sum,
        }),
        "freed_bytes must be the sum over the removed entries"
    );
    Outcome {
        entries,
        freed_bytes,
        at,
        mode: preview.mode,
    }
}

/// One entry through the last checks and, if it survives them, the port.
///
/// The kind reported is the preview's throughout, which for anything that gets deleted is
/// the kind the disk just confirmed — the comparison below is what makes those two the same
/// value. For a blocked entry it is whatever the preview carried, and for six of the eight
/// block reasons that is the plan's unverified claim (see [`PreviewEntry::kind`]): an
/// unverified kind travels on into the action log, deliberately, because a row that is drawn
/// needs an icon and the result beside it says how much the kind is worth.
fn run_entry(entry: &PreviewEntry, mode: Mode, limits: &Limits, sys: &dyn System) -> EntryOutcome {
    let skipped = |path: PathBuf, reason| EntryOutcome {
        path,
        kind: entry.kind,
        result: EntryResult::Skipped { reason },
    };
    // Reported where the user left it, with the reason they were shown. Not re-checked
    // either: the guards know nothing about the batch, so `Nested` would come back as
    // `Missing` once its ancestor was gone — the same row with the wrong explanation.
    if let EntryStatus::Blocked(reason) = entry.status {
        return skipped(entry.path.clone(), reason);
    }
    // `Checked::path`, never `Checked::judged` — the same rule as in `check_entry`, and this
    // is the call site where breaking it destroys something. `judged` is the resolved form:
    // for a symlink the link itself, deliberately, but for everything else the name the disk
    // has rather than the one the caller wrote. Deleting by it compiles, type-checks, passes
    // clippy and passes this suite everywhere a volume does not fold two spellings into one
    // directory — which is to say everywhere except macOS, where this app runs. So the field
    // is dropped here, at the one point that still knows why, and the rest of the function
    // has no name for it to reach for.
    let Checked { path, judged: _ } = match limits.check(&entry.path) {
        Ok(checked) => checked,
        Err(reason) => return skipped(entry.path.clone(), reason),
    };
    let meta = match sys.symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(err) => return skipped(path, reason_for(&err)),
    };
    // What the preview showed the user is what they confirmed. A file where the dialog said
    // directory is not that, whoever swapped it and whyever.
    if NodeKind::from_metadata(&meta) != entry.kind {
        return skipped(path, BlockReason::KindChanged);
    }
    let attempt = match mode {
        Mode::Trash => sys.move_to_trash(&path),
        Mode::Permanent => sys.remove(&path),
    };
    EntryOutcome {
        path,
        kind: entry.kind,
        result: match attempt {
            // The size the plan carried and the dialog promised. Re-reading it would mean
            // walking a subtree that is no longer there.
            Ok(()) => EntryResult::Removed { bytes: entry.size },
            // The entry went away in the syscall between the check above and this one. The
            // app reports what it did, and it did nothing: `Skipped`, exactly as for an
            // entry that was already gone when the batch reached it. Those two windows are
            // one syscall apart and neither is a fault, so a red row saying "no longer
            // exists" would alarm a user about a race that cost them nothing.
            Err(SystemError::Missing(_)) => EntryResult::Skipped {
                reason: BlockReason::Missing,
            },
            // `remove` is not atomic: a tree can be part-deleted and then fail, and this
            // reports 0 bytes for gigabytes that are really gone. A deliberate lower bound —
            // measuring the partial progress would mean walking what is left of a tree that
            // is still being torn down, and the rescan that follows a batch splices in
            // whatever survived either way.
            Err(err) => EntryResult::Failed {
                message: err.to_string(),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::NodeKind;
    use crate::system::{Invocation, Output, TestSystem};
    use chrono::{DateTime, TimeDelta, Utc};
    use std::fs::{self, Metadata};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    /// A port that cannot look at anything, records what it was asked about, and screams if
    /// asked to delete. Both stages use it: an entry neither of them can describe is one
    /// neither of them may delete, and the scream is how a test says so.
    ///
    /// `Limits::check` stats the entry too, so on a real filesystem it reports anything but
    /// "not found" before either stage gets to look. What is left for them to meet is a race
    /// — the entry was readable a syscall ago and is not now — and a race is not a thing to
    /// reproduce with `chmod`.
    struct Failing {
        /// What `symlink_metadata` answers. A function, because which `SystemError` arrives
        /// is the whole point of two of the tests below.
        error: fn(&Path) -> SystemError,
        /// Every path the port was asked about, in order. Without it a test can only check
        /// what the stage stored, never what it went and looked at.
        asked: Mutex<Vec<PathBuf>>,
    }

    impl Failing {
        fn new(error: fn(&Path) -> SystemError) -> Self {
            Self {
                error,
                asked: Mutex::new(Vec::new()),
            }
        }

        /// Cannot be read at all: the shape of a permission that changed under us.
        fn unreadable() -> Self {
            Self::new(|path| SystemError::Metadata {
                path: path.to_path_buf(),
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            })
        }

        /// Refuses the path outright, the way the port does when the engine hands it a
        /// shape it must never hand it.
        fn rejecting() -> Self {
            Self::new(|path| SystemError::Rejected {
                path: path.to_path_buf(),
                reason: "the path is not in normal form",
            })
        }

        fn asked(&self) -> Vec<PathBuf> {
            self.asked.lock().expect("an unpoisoned record").clone()
        }
    }

    impl System for Failing {
        fn symlink_metadata(&self, path: &Path) -> Result<Metadata, SystemError> {
            self.asked
                .lock()
                .expect("an unpoisoned record")
                .push(path.to_path_buf());
            Err((self.error)(path))
        }

        fn move_to_trash(&self, path: &Path) -> Result<(), SystemError> {
            unreachable!(
                "this port never deletes, and was asked to trash {}",
                path.display()
            );
        }

        fn remove(&self, path: &Path) -> Result<(), SystemError> {
            unreachable!(
                "this port never deletes, and was asked to remove {}",
                path.display()
            );
        }

        fn now(&self) -> DateTime<Utc> {
            Utc::now()
        }

        fn locate(&self, tool: &str) -> Option<PathBuf> {
            unreachable!("this port runs nothing, and was asked where {tool} is");
        }

        fn run(&self, invocation: &Invocation) -> Result<Output, SystemError> {
            unreachable!(
                "this port runs nothing, and was asked to run {}",
                invocation.program.display()
            );
        }
    }

    /// What a port was handed, and by which call.
    ///
    /// The two forms `Limits::check` returns name one entry, so a batch that deleted by the
    /// wrong one leaves the same disk behind: on a case-insensitive volume `Data` and `data`
    /// open one directory. What the port was asked is the only witness of which of the two
    /// the engine chose.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Deletion {
        Trashed(PathBuf),
        Removed(PathBuf),
    }

    impl Deletion {
        /// For the assertions `Path` equality is too generous for: it compares
        /// component-wise and calls `/a/link/` equal to `/a/link`.
        fn path(&self) -> &Path {
            match self {
                Self::Trashed(path) | Self::Removed(path) => path,
            }
        }
    }

    /// How far [`Recording`] moves the clock on each deletion.
    fn tick() -> TimeDelta {
        TimeDelta::seconds(1)
    }

    /// A [`TestSystem`] that writes down every path it is asked about and every deletion —
    /// the deletion before performing it, so that a path the port itself refuses is recorded
    /// as well — and moves the clock by [`tick`] as it deletes.
    ///
    /// The clock is the point of the last part: `TestSystem`'s own stands still, so
    /// `outcome.at == sys.now()` holds for every instant the port could produce and asserts
    /// nothing at all. Under this one the two ends of a batch are different instants.
    struct Recording<'a> {
        inner: &'a TestSystem,
        stats: Mutex<Vec<PathBuf>>,
        done: Mutex<Vec<Deletion>>,
    }

    impl<'a> Recording<'a> {
        fn new(inner: &'a TestSystem) -> Self {
            Self {
                inner,
                stats: Mutex::new(Vec::new()),
                done: Mutex::new(Vec::new()),
            }
        }

        /// Every path the port was asked to describe, in order — the preview's questions
        /// first, then the batch's.
        fn stats(&self) -> Vec<PathBuf> {
            self.stats.lock().expect("an unpoisoned record").clone()
        }

        fn done(&self) -> Vec<Deletion> {
            self.done.lock().expect("an unpoisoned record").clone()
        }

        fn record(&self, deletion: Deletion) {
            self.done
                .lock()
                .expect("an unpoisoned record")
                .push(deletion);
            self.inner.advance(tick());
        }
    }

    impl System for Recording<'_> {
        fn symlink_metadata(&self, path: &Path) -> Result<Metadata, SystemError> {
            self.stats
                .lock()
                .expect("an unpoisoned record")
                .push(path.to_path_buf());
            self.inner.symlink_metadata(path)
        }

        fn move_to_trash(&self, path: &Path) -> Result<(), SystemError> {
            self.record(Deletion::Trashed(path.to_path_buf()));
            self.inner.move_to_trash(path)
        }

        fn remove(&self, path: &Path) -> Result<(), SystemError> {
            self.record(Deletion::Removed(path.to_path_buf()));
            self.inner.remove(path)
        }

        fn now(&self) -> DateTime<Utc> {
            self.inner.now()
        }

        fn locate(&self, tool: &str) -> Option<PathBuf> {
            self.inner.locate(tool)
        }

        fn run(&self, invocation: &Invocation) -> Result<Output, SystemError> {
            self.inner.run(invocation)
        }
    }

    /// A [`TestSystem`] in which one entry disappears in the syscall between the last look
    /// and the deletion — the narrower of the two windows of that race, and the only way to
    /// reach it, since nothing a test can do from outside fits between two calls.
    struct Vanishing<'a> {
        inner: &'a TestSystem,
        when: PathBuf,
    }

    impl<'a> Vanishing<'a> {
        fn new(inner: &'a TestSystem, when: PathBuf) -> Self {
            Self { inner, when }
        }

        fn take_it(&self, path: &Path) {
            if path == self.when {
                // One of the two applies and the other fails; which is not the point.
                let _ = std::fs::remove_file(path);
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }

    impl System for Vanishing<'_> {
        fn symlink_metadata(&self, path: &Path) -> Result<Metadata, SystemError> {
            self.inner.symlink_metadata(path)
        }

        fn move_to_trash(&self, path: &Path) -> Result<(), SystemError> {
            self.take_it(path);
            self.inner.move_to_trash(path)
        }

        fn remove(&self, path: &Path) -> Result<(), SystemError> {
            self.take_it(path);
            self.inner.remove(path)
        }

        fn now(&self) -> DateTime<Utc> {
            self.inner.now()
        }

        fn locate(&self, tool: &str) -> Option<PathBuf> {
            self.inner.locate(tool)
        }

        fn run(&self, invocation: &Invocation) -> Result<Output, SystemError> {
            self.inner.run(invocation)
        }
    }

    /// Sizes rise with the position — 10, 20, 30 — so that a total over a batch names one
    /// entry and not just a count of them. With every entry at 10, `freed_bytes == 10` is as
    /// true of the entry that was deleted as of the one that was not.
    fn plan(sys: &TestSystem, names: &[&str], mode: Mode) -> Plan {
        Plan {
            entries: names
                .iter()
                .enumerate()
                .map(|(i, n)| PlanEntry {
                    path: sys.root().join(n),
                    kind: NodeKind::File,
                    size: 10 * (i as u64 + 1),
                })
                .collect(),
            mode,
        }
    }

    /// One entry with a path, or a claimed kind, the `plan` helper cannot spell.
    fn entry(path: PathBuf, kind: NodeKind, size: u64) -> PlanEntry {
        PlanEntry { path, kind, size }
    }

    #[test]
    fn ready_entries_are_totalled_and_blocked_ones_are_not() {
        let sys = TestSystem::new();
        fs::write(sys.root().join("a.bin"), b"x").unwrap();
        let mut p = plan(&sys, &["a.bin", "gone.bin"], Mode::Trash);
        p.entries[1].size = 999;
        let checked = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        assert_eq!(checked.entries[0].status, EntryStatus::Ready);
        assert_eq!(
            checked.entries[1].status,
            EntryStatus::Blocked(BlockReason::Missing)
        );
        assert_eq!(
            checked.total_bytes, 10,
            "a blocked entry contributes nothing"
        );
        assert_eq!(checked.mode, Mode::Trash);
    }

    #[test]
    fn a_descendant_of_another_entry_is_blocked_as_nested() {
        let sys = TestSystem::new();
        fs::create_dir_all(sys.root().join("dir/inner")).unwrap();
        let p = Plan {
            entries: vec![
                PlanEntry {
                    path: sys.root().join("dir"),
                    kind: NodeKind::Dir,
                    size: 100,
                },
                PlanEntry {
                    path: sys.root().join("dir/inner"),
                    kind: NodeKind::Dir,
                    size: 40,
                },
            ],
            mode: Mode::Permanent,
        };
        let checked = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        assert_eq!(
            checked.entries[1].status,
            EntryStatus::Blocked(BlockReason::Nested)
        );
        assert_eq!(
            checked.total_bytes, 100,
            "the child's bytes are not counted twice"
        );
    }

    #[test]
    fn preview_touches_nothing_on_disk() {
        let sys = TestSystem::new();
        let file = sys.root().join("a.bin");
        fs::write(&file, b"x").unwrap();
        let p = plan(&sys, &["a.bin"], Mode::Permanent);
        preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        // Not `exists`, which follows a symlink: it would also pass if the entry had been
        // replaced by a link to something else.
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

    // The rest are not in the plan.

    #[test]
    fn the_reason_the_guards_gave_survives_into_the_preview() {
        // `check` reports six reasons and the tests above exercise one of them. Collapsing
        // `Unreadable` into `Missing` would tell a user that a path they cannot look at has
        // vanished, and send them hunting for a ghost instead of granting Full Disk Access;
        // collapsing `Malformed` would do the same for a path that names no entry at all.
        let sys = TestSystem::new();
        let a = sys.root().join("loop-a");
        let b = sys.root().join("loop-b");
        std::os::unix::fs::symlink(&b, &a).unwrap();
        std::os::unix::fs::symlink(&a, &b).unwrap();
        fs::create_dir(sys.root().join("real")).unwrap();
        std::os::unix::fs::symlink(sys.root().join("real"), sys.root().join("link")).unwrap();
        fs::write(sys.root().join("keep.bin"), b"x").unwrap();
        let p = Plan {
            entries: vec![
                // No last component to delete.
                entry(PathBuf::from("/"), NodeKind::File, 1),
                // The parent resolves to a loop.
                entry(a.join("x.bin"), NodeKind::File, 2),
                // Nothing is there — spelled through a symlinked parent, so that the path
                // the guards examined and the one the plan carried can be told apart.
                entry(sys.root().join("link/gone.bin"), NodeKind::File, 4),
                entry(sys.root().join("keep.bin"), NodeKind::File, 8),
            ],
            mode: Mode::Permanent,
        };
        let checked = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        let statuses: Vec<EntryStatus> = checked.entries.iter().map(|e| e.status).collect();
        assert_eq!(
            statuses,
            vec![
                EntryStatus::Blocked(BlockReason::Malformed),
                EntryStatus::Blocked(BlockReason::Unreadable),
                EntryStatus::Blocked(BlockReason::Missing),
                EntryStatus::Ready,
            ]
        );
        assert_eq!(
            checked.entries[0].path,
            PathBuf::from("/"),
            "an entry the guards refused keeps the path the caller wrote"
        );
        assert_eq!(
            checked.entries[2].path,
            sys.root().join("real/gone.bin"),
            "an entry blocked after them reports the path they examined"
        );
        assert_eq!(checked.total_bytes, 8);
    }

    #[test]
    fn the_mode_of_the_plan_is_the_mode_of_the_preview() {
        // The test above pins `Trash`, which a preview that always said `Trash` would
        // satisfy as well. The mode is the whole difference between a file in the Trash and
        // a file that is gone, so both directions are pinned.
        let sys = TestSystem::new();
        fs::write(sys.root().join("a.bin"), b"x").unwrap();
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        for mode in [Mode::Trash, Mode::Permanent] {
            assert_eq!(
                preview(&plan(&sys, &["a.bin"], mode), &limits, &sys).mode,
                mode
            );
        }
    }

    #[test]
    fn the_disk_is_asked_about_the_path_that_would_be_deleted() {
        // The UI hands the backend strings, and POSIX resolves a last component written as
        // a directory by following it: `<root>/link/` describes the target, `<root>/link`
        // the link. The guards strip the separator, and the entry the preview stats has to
        // be the one the deletion will touch.
        let sys = TestSystem::new();
        let target = sys.root().join("target");
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, sys.root().join("link")).unwrap();
        let p = plan(&sys, &["link/"], Mode::Trash);
        assert!(
            p.entries[0].path.to_string_lossy().ends_with('/'),
            "the plan carries the separator the caller wrote"
        );
        let checked = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        // Byte for byte: `Path` compares component-wise and calls `<root>/link/` equal to
        // `<root>/link`, which is the one difference this test is about.
        assert_eq!(
            checked.entries[0].path.as_os_str(),
            sys.root().join("link").as_os_str(),
            "the guards stripped it"
        );
        assert_eq!(
            checked.entries[0].kind,
            NodeKind::Symlink,
            "the link, not the directory it points at"
        );
    }

    #[test]
    fn an_entry_that_cannot_be_looked_at_is_not_called_gone() {
        // The other half of the same rule, one stage later: the guards let the path through
        // and the port cannot read it. "It vanished" would be a different instruction to
        // the user than "you cannot see it", and only one of them is true.
        let sys = TestSystem::new();
        fs::create_dir(sys.root().join("real")).unwrap();
        fs::write(sys.root().join("real/a.bin"), b"x").unwrap();
        std::os::unix::fs::symlink(sys.root().join("real"), sys.root().join("link")).unwrap();
        // Through a symlinked parent, so that the path the guards returned and the one the
        // plan carried are two different paths and the assertions below can tell them
        // apart. The claimed kind is wrong on purpose: `a.bin` is a file.
        let p = Plan {
            entries: vec![entry(sys.root().join("link/a.bin"), NodeKind::Dir, 10)],
            mode: Mode::Trash,
        };
        let port = Failing::unreadable();
        let checked = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &port);
        assert_eq!(
            checked.entries[0].status,
            EntryStatus::Blocked(BlockReason::Unreadable)
        );
        assert_eq!(
            port.asked(),
            vec![sys.root().join("real/a.bin")],
            "the preview asked the port about the path the guards returned"
        );
        assert_eq!(
            checked.entries[0].path,
            sys.root().join("real/a.bin"),
            "and reports the path it asked about"
        );
        // Nothing was read, so both come from the plan, wrong claim and all; the dialog
        // still has a row to draw.
        assert_eq!(
            checked.entries[0].kind,
            NodeKind::Dir,
            "the plan's unverified claim, not a guess of our own"
        );
        assert_eq!(checked.entries[0].size, 10, "and the plan's size");
        assert_eq!(checked.total_bytes, 0);
    }

    #[test]
    fn a_path_the_port_refuses_is_malformed_not_unreadable() {
        // `Rejected` means the engine handed the port a path it must never hand it — a bug
        // on this side, not a state anyone can fix. `Unreadable` is the reason that sends a
        // user to System Settings for Full Disk Access, which would be the wrong errand.
        let sys = TestSystem::new();
        fs::write(sys.root().join("a.bin"), b"x").unwrap();
        let p = plan(&sys, &["a.bin"], Mode::Trash);
        let port = Failing::rejecting();
        let checked = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &port);
        assert_eq!(
            checked.entries[0].status,
            EntryStatus::Blocked(BlockReason::Malformed)
        );
        assert_eq!(checked.total_bytes, 0);
    }

    #[test]
    fn a_symlink_is_previewed_as_a_symlink() {
        // Through `NodeKind::from_metadata`, the classifier the walker uses. A local
        // `if is_dir { Dir } else { File }` would call this one a file, and the
        // re-validation before the deletion would then refuse it as `KindChanged` for ever.
        let sys = TestSystem::new();
        let target = sys.root().join("target");
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, sys.root().join("link")).unwrap();
        let p = plan(&sys, &["link"], Mode::Trash);
        let checked = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        assert_eq!(checked.entries[0].kind, NodeKind::Symlink);
        assert_eq!(checked.entries[0].status, EntryStatus::Ready);
    }

    #[test]
    fn nesting_is_judged_on_the_paths_the_guards_returned() {
        // A plan carries the spelling the caller had. Here `link/inner` and `real/inner`
        // are one directory, so a nesting check on the raw spelling would keep both
        // entries, promise the inner bytes twice, and then fail to delete what is already
        // gone with its ancestor.
        let sys = TestSystem::new();
        fs::create_dir_all(sys.root().join("real/inner")).unwrap();
        std::os::unix::fs::symlink(sys.root().join("real"), sys.root().join("link")).unwrap();
        let p = Plan {
            entries: vec![
                PlanEntry {
                    path: sys.root().join("real"),
                    kind: NodeKind::Dir,
                    size: 100,
                },
                PlanEntry {
                    path: sys.root().join("link/inner"),
                    kind: NodeKind::Dir,
                    size: 40,
                },
            ],
            mode: Mode::Trash,
        };
        let checked = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        assert_eq!(
            checked.entries[1].path,
            sys.root().join("real/inner"),
            "the guards resolved the parent"
        );
        assert_eq!(
            checked.entries[1].status,
            EntryStatus::Blocked(BlockReason::Nested)
        );
        assert_eq!(checked.total_bytes, 100);
    }

    /// One directory, selected twice: once under the name it has on disk and once under a
    /// spelling that opens the same directory on a stock macOS volume. Whether the two are
    /// one directory is asked of the volume, not guessed from the OS, the way `guards.rs`
    /// probes for case sensitivity.
    /// What it means when a volume does not open both names of a pair.
    #[derive(Clone, Copy)]
    enum Aliasing {
        /// Case. APFS can be formatted case-sensitive, and a developer's volume sometimes
        /// is; there the two names really are two directories and the rule has nothing to
        /// do, on a Mac as much as on Linux.
        MayNotFold,
        /// Normalization. Every APFS variant folds it, the case-sensitive one included, so
        /// a Mac that does not is a fixture that stopped working rather than a volume.
        FoldsOnEveryMac,
    }

    /// Whether to go quiet about a pair this volume keeps apart, or to stop.
    fn skip_or_stop(name: &str, aliasing: Aliasing) {
        // A pair every Mac folds and this one does not is the strongest test in the file
        // turning into a no-op that still reports `ok`. A case pair on a case-sensitive dev
        // volume is just that volume, and saying otherwise would be a red test with a false
        // explanation — and would stop the normalization pairs, which do cover the rule
        // there, from ever running.
        if cfg!(target_os = "macos") && matches!(aliasing, Aliasing::FoldsOnEveryMac) {
            panic!(
                "{name} is its own name here: every APFS variant folds normalization, so \
                 this is a broken fixture rather than a volume, and the aliasing rules are \
                 untested on this Mac"
            );
        }
        eprintln!("skipped: {name} is its own name on this volume");
    }

    fn assert_one_directory_counted_once(on_disk: &str, alias: &str, aliasing: Aliasing) {
        let sys = TestSystem::new();
        let real = sys.root().join(on_disk);
        fs::create_dir(&real).unwrap();
        let aliased = sys.root().join(alias);
        if !aliased.is_dir() {
            // Two names, two directories, and nothing here for the rule to do. Asserting the
            // duller outcome instead would let a green run on such a volume read as coverage
            // it does not have — and CI runs the core tests on Linux as well as on macOS.
            skip_or_stop(alias, aliasing);
            return;
        }
        let p = Plan {
            entries: vec![
                PlanEntry {
                    path: real,
                    kind: NodeKind::Dir,
                    size: 100,
                },
                PlanEntry {
                    path: aliased,
                    kind: NodeKind::Dir,
                    size: 100,
                },
            ],
            mode: Mode::Trash,
        };
        let checked = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        assert_eq!(checked.entries[0].status, EntryStatus::Ready);
        assert_eq!(
            checked.entries[1].status,
            EntryStatus::Blocked(BlockReason::Nested),
            "{alias} opens the directory created as {on_disk} on this volume"
        );
        assert_eq!(
            checked.total_bytes, 100,
            "one directory on disk, promised once"
        );
    }

    #[test]
    fn another_case_of_one_name_is_not_counted_twice() {
        assert_one_directory_counted_once("Data", "data", Aliasing::MayNotFold);
    }

    #[test]
    fn another_normalization_of_one_name_is_not_counted_twice() {
        // No user error at all: the same name typed on macOS arrives in NFC from one source
        // and in NFD from another, and APFS treats them as one directory.
        assert_one_directory_counted_once("caf\u{e9}", "cafe\u{301}", Aliasing::FoldsOnEveryMac);
        assert_one_directory_counted_once("cafe\u{301}", "caf\u{e9}", Aliasing::FoldsOnEveryMac);
    }

    #[test]
    fn a_symlink_and_the_directory_it_points_at_are_two_entries() {
        // The other side of judging the resolved form: a symlink must stay judged as
        // written. Deleting the link leaves the target alone, so neither entry contains the
        // other and both bytes are really freed.
        let sys = TestSystem::new();
        let target = sys.root().join("target");
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, sys.root().join("link")).unwrap();
        let p = Plan {
            entries: vec![
                PlanEntry {
                    path: sys.root().join("link"),
                    kind: NodeKind::Symlink,
                    size: 10,
                },
                PlanEntry {
                    path: target,
                    kind: NodeKind::Dir,
                    size: 100,
                },
            ],
            mode: Mode::Trash,
        };
        let checked = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        assert_eq!(checked.entries[0].status, EntryStatus::Ready);
        assert_eq!(checked.entries[1].status, EntryStatus::Ready);
        assert_eq!(checked.total_bytes, 110);
    }

    #[test]
    fn one_entry_selected_twice_is_promised_once() {
        // `guards.rs` pins the first-wins rule on `drop_nested` itself; this is the
        // consequence the user sees. The reason reads `Nested`, which is the wrong word for
        // a row that contains nothing — a deliberate trade, spelled out in `preview`.
        let sys = TestSystem::new();
        fs::create_dir(sys.root().join("dir")).unwrap();
        let p = Plan {
            entries: vec![
                PlanEntry {
                    path: sys.root().join("dir"),
                    kind: NodeKind::Dir,
                    size: 100,
                },
                PlanEntry {
                    path: sys.root().join("dir"),
                    kind: NodeKind::Dir,
                    size: 100,
                },
            ],
            mode: Mode::Trash,
        };
        let checked = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        assert_eq!(checked.entries[0].status, EntryStatus::Ready);
        assert_eq!(
            checked.entries[1].status,
            EntryStatus::Blocked(BlockReason::Nested)
        );
        assert_eq!(checked.total_bytes, 100);
    }

    #[test]
    fn a_blocked_ancestor_neither_nests_what_is_below_it_nor_moves_it() {
        // Selecting the root row together with things inside it costs one click in a tree
        // view. The root is refused and stays where it is, so what is below it is not
        // inside anything that is about to be deleted — and the entries that follow a
        // blocked one must keep their own places in the list.
        //
        // All three are directories on disk and all three are planned as something else, so
        // that the kinds below say where each value came from instead of agreeing by luck.
        let sys = TestSystem::new();
        fs::create_dir_all(sys.root().join("dir/inner")).unwrap();
        let p = Plan {
            entries: vec![
                entry(sys.root().to_path_buf(), NodeKind::Symlink, 1_000),
                entry(sys.root().join("dir"), NodeKind::File, 100),
                entry(sys.root().join("dir/inner"), NodeKind::File, 40),
            ],
            mode: Mode::Trash,
        };
        let checked = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        let statuses: Vec<EntryStatus> = checked.entries.iter().map(|e| e.status).collect();
        assert_eq!(
            statuses,
            vec![
                EntryStatus::Blocked(BlockReason::IsRoot),
                EntryStatus::Ready,
                EntryStatus::Blocked(BlockReason::Nested),
            ]
        );
        let kinds: Vec<NodeKind> = checked.entries.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![NodeKind::Symlink, NodeKind::Dir, NodeKind::Dir],
            "refused before anything was read, so the plan's claim stands; the other two \
             were read, and a `Nested` entry keeps what the disk said"
        );
        assert_eq!(checked.total_bytes, 100);
    }

    // From here on `execute`: the six of the plan first, then the ones that are not in it.

    #[test]
    fn trash_mode_moves_the_entries_to_the_trash() {
        let sys = TestSystem::new();
        fs::write(sys.root().join("a.bin"), b"xxx").unwrap();
        let p = plan(&sys, &["a.bin"], Mode::Trash);
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let outcome = execute(&preview(&p, &limits, &sys), &limits, &sys);
        assert_eq!(outcome.freed_bytes, 10);
        assert!(matches!(
            outcome.entries[0].result,
            EntryResult::Removed { bytes: 10 }
        ));
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
        assert!(matches!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::Missing
            }
        ));
        assert!(matches!(
            outcome.entries[1].result,
            EntryResult::Removed { .. }
        ));
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
        assert!(matches!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::KindChanged
            }
        ));
        assert!(
            fs::symlink_metadata(sys.root().join("a.bin"))
                .unwrap()
                .is_dir(),
            "the replacement is left alone"
        );
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
        assert!(matches!(
            outcome.entries[0].result,
            EntryResult::Failed { .. }
        ));
        assert!(matches!(
            outcome.entries[1].result,
            EntryResult::Removed { .. }
        ));
        assert!(
            fs::symlink_metadata(sys.root().join("a.bin")).is_ok(),
            "a failure leaves the entry alone"
        );
        assert_eq!(outcome.freed_bytes, 20);
    }

    #[test]
    fn blocked_entries_are_reported_but_never_touched() {
        let sys = TestSystem::new();
        let p = plan(&sys, &["gone.bin"], Mode::Trash);
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let outcome = execute(&preview(&p, &limits, &sys), &limits, &sys);
        assert_eq!(outcome.entries.len(), 1);
        assert!(matches!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::Missing
            }
        ));
        assert_eq!(outcome.freed_bytes, 0);
    }

    // The rest are not in the plan.

    #[test]
    fn a_forged_preview_is_still_judged_by_the_guards() {
        // `Preview` has public fields and derives `Deserialize`, so one can be built without
        // ever having passed them. Every row here claims to be ready, and every one of them
        // is a path the guards refuse.
        let sys = TestSystem::new();
        let denied = sys.root().join("denied");
        fs::create_dir(&denied).unwrap();
        let below_denied = denied.join("keep.bin");
        fs::write(&below_denied, b"keep").unwrap();
        // Outside the scan root but inside the temporary tree, so it is this code that has
        // to refuse it: `TestSystem` would panic instead of deleting anything further out,
        // and a test that passed because the double screamed would prove nothing.
        let outside_root = sys.trash_dir().join("keep.bin");
        fs::write(&outside_root, b"keep").unwrap();
        let limits = Limits::new(sys.root().to_path_buf(), vec![denied.clone()]);
        let forged = Preview {
            entries: vec![
                PreviewEntry {
                    path: below_denied.clone(),
                    kind: NodeKind::File,
                    size: 10,
                    status: EntryStatus::Ready,
                },
                PreviewEntry {
                    path: outside_root.clone(),
                    kind: NodeKind::File,
                    size: 10,
                    status: EntryStatus::Ready,
                },
                PreviewEntry {
                    path: sys.root().to_path_buf(),
                    kind: NodeKind::Dir,
                    size: 10,
                    status: EntryStatus::Ready,
                },
            ],
            total_bytes: 30,
            mode: Mode::Permanent,
        };
        let outcome = execute(&forged, &limits, &sys);
        let results: Vec<EntryResult> = outcome.entries.iter().map(|e| e.result.clone()).collect();
        assert_eq!(
            results,
            vec![
                EntryResult::Skipped {
                    reason: BlockReason::Denylisted
                },
                EntryResult::Skipped {
                    reason: BlockReason::OutsideRoots
                },
                EntryResult::Skipped {
                    reason: BlockReason::IsRoot
                },
            ],
            "the reason the guards gave, not one of `execute`'s own"
        );
        assert_eq!(fs::read(&below_denied).unwrap(), b"keep");
        assert_eq!(fs::read(&outside_root).unwrap(), b"keep");
        assert!(fs::symlink_metadata(sys.root()).unwrap().is_dir());
        assert_eq!(outcome.freed_bytes, 0);
    }

    #[test]
    fn an_entry_the_preview_blocked_is_left_where_it_is() {
        // The plan's blocked entry is one that is not there at all, which a batch that
        // ignored the statuses would also leave alone — there is nothing to delete. This
        // one is on disk and deletable-looking, and the status is all that stands in the
        // way. The claimed kind is wrong on purpose, so the kinds below say where each
        // value came from instead of agreeing by luck.
        let sys = TestSystem::new();
        let denied = sys.root().join("denied");
        fs::create_dir(&denied).unwrap();
        let keep = denied.join("keep.bin");
        fs::write(&keep, b"keep").unwrap();
        fs::write(sys.root().join("go.bin"), b"x").unwrap();
        let limits = Limits::new(sys.root().to_path_buf(), vec![denied]);
        let p = Plan {
            entries: vec![
                entry(keep.clone(), NodeKind::Symlink, 10),
                entry(sys.root().join("go.bin"), NodeKind::File, 20),
            ],
            mode: Mode::Permanent,
        };
        let outcome = execute(&preview(&p, &limits, &sys), &limits, &sys);
        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::Denylisted
            }
        );
        assert_eq!(fs::read(&keep).unwrap(), b"keep", "still there, untouched");
        assert!(matches!(
            outcome.entries[1].result,
            EntryResult::Removed { bytes: 20 }
        ));
        assert!(fs::symlink_metadata(sys.root().join("go.bin")).is_err());
        assert_eq!(
            outcome.freed_bytes, 20,
            "the entry that went, not the one that was refused"
        );
        let kinds: Vec<NodeKind> = outcome.entries.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![NodeKind::Symlink, NodeKind::File],
            "a blocked row carries the plan's unverified claim on into the log; a deleted \
             one carries what the disk said"
        );
    }

    #[test]
    fn a_symlink_that_became_a_directory_is_skipped() {
        // The transition the re-check exists for, and the only one where the loss has no
        // bound: the row promises a symlink and the 0 bytes a scan records for one, and by
        // the time the batch runs that name is a real directory with a tree under it.
        // Deleting it would destroy all of it and write `Removed { bytes: 0 }` in the log,
        // under the kind the user confirmed — so the log would not even say what went.
        let sys = TestSystem::new();
        let target = sys.root().join("target");
        fs::create_dir(&target).unwrap();
        let swapped = sys.root().join("swapped");
        std::os::unix::fs::symlink(&target, &swapped).unwrap();
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let p = Plan {
            entries: vec![entry(swapped.clone(), NodeKind::Symlink, 0)],
            mode: Mode::Permanent,
        };
        let checked = preview(&p, &limits, &sys);
        assert_eq!(checked.entries[0].kind, NodeKind::Symlink);
        fs::remove_file(&swapped).unwrap();
        let deep = swapped.join("one/two");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("precious.bin"), b"keep").unwrap();
        let outcome = execute(&checked, &limits, &sys);
        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::KindChanged
            }
        );
        assert_eq!(
            fs::read(deep.join("precious.bin")).unwrap(),
            b"keep",
            "the tree that took the name is untouched, all the way down"
        );
        assert_eq!(outcome.freed_bytes, 0);
    }

    #[test]
    fn a_socket_that_became_a_directory_is_skipped() {
        // The same loss profile as the symlink row above — nothing promised, everything
        // destroyed, a log line that would not record what went — for the kind that is
        // easiest to think of as exotic and is not: `Other` is what the walker mints for
        // every socket, fifo and device node, and a home folder is full of them. It is also
        // what the desktop layer falls back to when the tree does not know a path, so it is
        // the last kind whose re-check should go untested.
        let sys = TestSystem::new();
        let swapped = sys.root().join("sock");
        // A socket with std alone. Dropping the listener closes the descriptor and leaves
        // the entry on disk, which is all this needs.
        drop(std::os::unix::net::UnixListener::bind(&swapped).expect("bind a unix socket"));
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let p = Plan {
            entries: vec![entry(swapped.clone(), NodeKind::Other, 0)],
            mode: Mode::Permanent,
        };
        let checked = preview(&p, &limits, &sys);
        assert_eq!(
            checked.entries[0].kind,
            NodeKind::Other,
            "the disk said neither file, nor directory, nor link"
        );
        fs::remove_file(&swapped).unwrap();
        let deep = swapped.join("one/two");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("precious.bin"), b"keep").unwrap();
        let outcome = execute(&checked, &limits, &sys);
        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::KindChanged
            }
        );
        assert_eq!(
            fs::read(deep.join("precious.bin")).unwrap(),
            b"keep",
            "the tree that took the name is untouched, all the way down"
        );
        assert_eq!(outcome.freed_bytes, 0);
    }

    #[test]
    fn a_directory_that_became_a_symlink_is_skipped() {
        // The other direction. Deleting the link would free none of the bytes the dialog
        // promised, leave the directory the user meant where it was, and say `Removed`.
        let sys = TestSystem::new();
        let swapped = sys.root().join("swapped");
        fs::create_dir(&swapped).unwrap();
        let elsewhere = sys.root().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let p = Plan {
            entries: vec![entry(swapped.clone(), NodeKind::Dir, 100)],
            mode: Mode::Trash,
        };
        let checked = preview(&p, &limits, &sys);
        assert_eq!(checked.entries[0].kind, NodeKind::Dir);
        fs::remove_dir(&swapped).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &swapped).unwrap();
        let outcome = execute(&checked, &limits, &sys);
        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::KindChanged
            }
        );
        assert!(
            fs::symlink_metadata(&swapped).unwrap().is_symlink(),
            "the link that took the name is left alone"
        );
        assert_eq!(fs::read_dir(sys.trash_dir()).unwrap().count(), 0);
        assert_eq!(outcome.freed_bytes, 0);
    }

    #[test]
    fn a_file_that_became_a_symlink_is_skipped() {
        // The transition a comparison of "is it a directory?" cannot see: neither form is
        // one, so such a check would call them equal and delete the link — freeing nothing,
        // while the log claims the file's bytes and the file is still somewhere on the disk.
        let sys = TestSystem::new();
        let swapped = sys.root().join("a.bin");
        fs::write(&swapped, b"x").unwrap();
        let other = sys.root().join("other.bin");
        fs::write(&other, b"keep").unwrap();
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let checked = preview(&plan(&sys, &["a.bin"], Mode::Permanent), &limits, &sys);
        assert_eq!(checked.entries[0].kind, NodeKind::File);
        fs::remove_file(&swapped).unwrap();
        std::os::unix::fs::symlink(&other, &swapped).unwrap();
        let outcome = execute(&checked, &limits, &sys);
        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::KindChanged
            }
        );
        assert!(
            fs::symlink_metadata(&swapped).unwrap().is_symlink(),
            "the link that took the name is left alone"
        );
        assert_eq!(fs::read(&other).unwrap(), b"keep");
        assert_eq!(outcome.freed_bytes, 0);
    }

    #[test]
    fn an_entry_that_cannot_be_looked_at_when_the_batch_runs_is_not_called_gone() {
        // The same rule as one stage earlier, now in the stage that deletes: `Missing` sends
        // a user hunting for a ghost while `Unreadable` sends them to Full Disk Access, and
        // only one of them is true. The double's deletion methods are `unreachable!`, which
        // is the other half of the assertion — an entry nobody can describe is not deleted.
        let sys = TestSystem::new();
        fs::write(sys.root().join("a.bin"), b"x").unwrap();
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let checked = preview(&plan(&sys, &["a.bin"], Mode::Permanent), &limits, &sys);
        let port = Failing::unreadable();
        let outcome = execute(&checked, &limits, &port);
        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::Unreadable
            }
        );
        assert_eq!(
            port.asked(),
            vec![sys.root().join("a.bin")],
            "and it asked about the path it would have deleted"
        );
        assert_eq!(fs::read(sys.root().join("a.bin")).unwrap(), b"x");
        assert_eq!(outcome.freed_bytes, 0);
    }

    #[test]
    fn a_path_the_port_refuses_when_the_batch_runs_is_malformed_not_unreadable() {
        // `Rejected` is a bug on this side of the port, not a permission the user can grant.
        // One `Err(_)` away from being collapsed into one of the other two, in the stage
        // where the row it produces is the only record of what happened.
        let sys = TestSystem::new();
        fs::write(sys.root().join("a.bin"), b"x").unwrap();
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let checked = preview(&plan(&sys, &["a.bin"], Mode::Trash), &limits, &sys);
        let outcome = execute(&checked, &limits, &Failing::rejecting());
        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::Malformed
            }
        );
        assert_eq!(fs::read(sys.root().join("a.bin")).unwrap(), b"x");
        assert_eq!(outcome.freed_bytes, 0);
    }

    #[test]
    fn an_entry_that_vanishes_inside_the_deletion_is_skipped_not_failed() {
        // The narrower window of the same race: the check above saw the entry, and it was
        // gone one syscall later. The app destroyed nothing, so it reports that it did
        // nothing — the same word as for an entry that was already gone when the batch
        // reached it. A red row reading "no longer exists" would alarm a user about a race
        // that cost them nothing.
        let sys = TestSystem::new();
        fs::write(sys.root().join("a.bin"), b"x").unwrap();
        fs::write(sys.root().join("b.bin"), b"x").unwrap();
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let p = plan(&sys, &["a.bin", "b.bin"], Mode::Trash);
        let checked = preview(&p, &limits, &sys);
        let port = Vanishing::new(&sys, sys.root().join("a.bin"));
        let outcome = execute(&checked, &limits, &port);
        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::Missing
            }
        );
        assert!(matches!(
            outcome.entries[1].result,
            EntryResult::Removed { bytes: 20 }
        ));
    }

    #[test]
    fn an_entry_that_vanished_inside_the_deletion_frees_nothing() {
        // The other half: not a failure, and not a removal either. Counting it would put
        // bytes in the log for an entry this app never touched.
        let sys = TestSystem::new();
        fs::write(sys.root().join("a.bin"), b"x").unwrap();
        fs::write(sys.root().join("b.bin"), b"x").unwrap();
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let p = plan(&sys, &["a.bin", "b.bin"], Mode::Trash);
        let checked = preview(&p, &limits, &sys);
        let port = Vanishing::new(&sys, sys.root().join("a.bin"));
        let outcome = execute(&checked, &limits, &port);
        assert_eq!(
            outcome.freed_bytes, 20,
            "the bytes of the entry that really went, and only those"
        );
        assert_eq!(
            fs::read_dir(sys.trash_dir()).unwrap().count(),
            1,
            "and nothing of the vanished one reached the Trash"
        );
    }

    #[test]
    fn the_batch_is_timestamped_when_it_began() {
        // Under `TestSystem` alone the clock stands still, so `outcome.at == sys.now()` is
        // true of every instant there is. This port moves it as it deletes, which is what
        // makes the assertion an assertion: `at` is the start of the batch, the key the
        // Activity screen sorts by, and not whenever the last tree finished coming down.
        let sys = TestSystem::new();
        fs::write(sys.root().join("a.bin"), b"x").unwrap();
        fs::write(sys.root().join("b.bin"), b"x").unwrap();
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let started = sys.now();
        let port = Recording::new(&sys);
        let p = plan(&sys, &["a.bin", "b.bin"], Mode::Trash);
        let outcome = execute(&preview(&p, &limits, &port), &limits, &port);
        assert_eq!(
            sys.now(),
            started + tick() * 2,
            "the clock did move under it"
        );
        assert_eq!(
            outcome.at, started,
            "and the batch is stamped with the instant it began"
        );
    }

    #[test]
    fn the_stat_and_the_deletion_both_use_the_path_the_guards_returned_now() {
        // The preview stored `<root>/dir/a.bin`; between the stages `dir` becomes a link to
        // another directory, so the guards — which resolve parents — answer a different path
        // for the same spelling. Everything after them has to use that answer: the stat that
        // decides, the deletion that acts, and the row that records it. Deleting where the
        // guards last looked is the whole point of re-checking; staying on the preview's
        // path would delete somewhere they did not.
        let sys = TestSystem::new();
        fs::create_dir(sys.root().join("dir")).unwrap();
        fs::write(sys.root().join("dir/a.bin"), b"stale").unwrap();
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let port = Recording::new(&sys);
        let checked = preview(&plan(&sys, &["dir/a.bin"], Mode::Permanent), &limits, &port);
        assert_eq!(checked.entries[0].path, sys.root().join("dir/a.bin"));
        fs::create_dir(sys.root().join("other")).unwrap();
        let fresh = sys.root().join("other/a.bin");
        fs::write(&fresh, b"fresh").unwrap();
        fs::remove_dir_all(sys.root().join("dir")).unwrap();
        std::os::unix::fs::symlink(sys.root().join("other"), sys.root().join("dir")).unwrap();
        let outcome = execute(&checked, &limits, &port);
        // Two entries and not four: the guards stat through `std::fs` on purpose, so their
        // own looks do not pass the port and are not recorded here. If that ever changes,
        // this is the assertion that will break first, and confusingly.
        assert_eq!(
            port.stats(),
            vec![sys.root().join("dir/a.bin"), fresh.clone()],
            "the preview asked about its own path, the batch about the one it re-checked"
        );
        assert_eq!(port.done(), vec![Deletion::Removed(fresh.clone())]);
        assert_eq!(
            outcome.entries[0].path, fresh,
            "and the row names what was deleted, not what the preview held"
        );
        assert!(fs::symlink_metadata(&fresh).is_err());
    }

    #[test]
    fn an_empty_preview_is_an_empty_outcome() {
        // What the command gets when the user clears the selection: no rows, no bytes, and
        // still a timestamp and a mode, because the log takes whole outcomes.
        let sys = TestSystem::new();
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let outcome = execute(
            &Preview {
                entries: vec![],
                total_bytes: 0,
                mode: Mode::Permanent,
            },
            &limits,
            &sys,
        );
        assert!(outcome.entries.is_empty());
        assert_eq!(outcome.freed_bytes, 0);
        assert_eq!(outcome.mode, Mode::Permanent);
        assert_eq!(outcome.at, sys.now());
    }

    #[test]
    fn the_mode_of_the_preview_is_the_mode_of_the_outcome() {
        // `Removed` is two different facts — moved and recoverable, or gone — and a row of
        // the action log is read long after the dialog that produced it. Both directions are
        // pinned, as for the preview: an outcome that always said `Trash` would satisfy one.
        let sys = TestSystem::new();
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        for mode in [Mode::Trash, Mode::Permanent] {
            fs::write(sys.root().join("a.bin"), b"x").unwrap();
            let checked = preview(&plan(&sys, &["a.bin"], mode), &limits, &sys);
            let outcome = execute(&checked, &limits, &sys);
            assert!(matches!(
                outcome.entries[0].result,
                EntryResult::Removed { .. }
            ));
            assert_eq!(outcome.mode, mode);
        }
    }

    #[test]
    fn a_row_skipped_at_execution_time_keeps_the_kind_the_dialog_showed() {
        // The entry is a directory by the time the batch runs and the preview saw a file.
        // Which of the two the outcome names is the difference between "the file you chose
        // is not a file any more" and a line about a directory the user never selected —
        // and this is the value the action log keeps.
        let sys = TestSystem::new();
        fs::write(sys.root().join("a.bin"), b"x").unwrap();
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let checked = preview(&plan(&sys, &["a.bin"], Mode::Trash), &limits, &sys);
        assert_eq!(checked.entries[0].kind, NodeKind::File);
        fs::remove_file(sys.root().join("a.bin")).unwrap();
        fs::create_dir(sys.root().join("a.bin")).unwrap();
        let outcome = execute(&checked, &limits, &sys);
        assert_eq!(
            outcome.entries[0].result,
            EntryResult::Skipped {
                reason: BlockReason::KindChanged
            }
        );
        assert_eq!(
            outcome.entries[0].kind,
            NodeKind::File,
            "what the preview showed, not what took its place"
        );
        assert_eq!(
            fs::read_dir(sys.trash_dir()).unwrap().count(),
            0,
            "and what took its place is left where it is"
        );
    }

    #[test]
    fn a_nested_entry_is_skipped_for_the_reason_the_preview_gave() {
        // `execute` runs the guards again and the guards know nothing about the batch, so an
        // implementation that went by the paths and ignored the statuses would delete the
        // ancestor and report the descendant as `Missing`: the same row with the wrong
        // explanation, and a log that says an entry vanished when this app took it.
        //
        // Which is also why `execute` has no nesting pass of its own. The preview blocked
        // the descendant already, and for a forged preview the worst a missing pass can do
        // is count an ancestor's bytes twice in its own log — while a second `drop_nested`
        // would mean carrying the judged forms through `execute` as well, where deleting one
        // by mistake is a symlink's target instead of the symlink.
        let sys = TestSystem::new();
        fs::create_dir_all(sys.root().join("dir/inner")).unwrap();
        let p = Plan {
            entries: vec![
                entry(sys.root().join("dir"), NodeKind::Dir, 100),
                entry(sys.root().join("dir/inner"), NodeKind::Dir, 40),
            ],
            mode: Mode::Trash,
        };
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let outcome = execute(&preview(&p, &limits, &sys), &limits, &sys);
        assert!(matches!(
            outcome.entries[0].result,
            EntryResult::Removed { bytes: 100 }
        ));
        assert_eq!(
            outcome.entries[1].result,
            EntryResult::Skipped {
                reason: BlockReason::Nested
            }
        );
        assert_eq!(outcome.freed_bytes, 100, "one directory, counted once");
        assert!(
            fs::symlink_metadata(sys.root().join("dir")).is_err(),
            "the ancestor went, and the descendant with it"
        );
    }

    #[test]
    fn a_symlink_is_deleted_as_a_link_in_the_form_the_port_demands() {
        let sys = TestSystem::new();
        let target = sys.root().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("inside.bin"), b"keep").unwrap();
        std::os::unix::fs::symlink(&target, sys.root().join("link")).unwrap();
        // The caller wrote the separator and the guards stripped it. The port refuses
        // anything else, but that is the second line of defence, not the assertion here:
        // `remove("<root>/link/")` deletes the directory behind the link.
        let p = plan(&sys, &["link/"], Mode::Permanent);
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let port = Recording::new(&sys);
        let outcome = execute(&preview(&p, &limits, &port), &limits, &port);
        let done = port.done();
        assert_eq!(done, vec![Deletion::Removed(sys.root().join("link"))]);
        assert_eq!(
            done[0].path().as_os_str(),
            sys.root().join("link").as_os_str(),
            "byte for byte: `Path` compares component-wise and calls `<root>/link/` equal \
             to `<root>/link`"
        );
        assert!(matches!(
            outcome.entries[0].result,
            EntryResult::Removed { bytes: 10 }
        ));
        assert_eq!(outcome.entries[0].kind, NodeKind::Symlink);
        assert!(fs::symlink_metadata(sys.root().join("link")).is_err());
        assert_eq!(
            fs::read(target.join("inside.bin")).unwrap(),
            b"keep",
            "the target of the link is untouched"
        );
    }

    /// One directory on disk, deleted under the name the caller wrote. `Checked` carries
    /// that name and, as `judged`, the one the disk has; on a volume where both open that
    /// one directory a deletion by either leaves the same disk behind, so what the port was
    /// handed is the only witness of which form the engine used. A symlink cannot stand in
    /// for this — the guards judge one as written, which makes its two forms identical.
    fn assert_the_port_is_asked_for(on_disk: &str, asked_as: &str, aliasing: Aliasing) {
        let sys = TestSystem::new();
        fs::create_dir(sys.root().join(on_disk)).unwrap();
        let asked = sys.root().join(asked_as);
        if !asked.is_dir() {
            // The two names are two directories here, so the two forms of a `Checked` cannot
            // differ and there is nothing to tell apart. This is the one test that separates
            // `Checked::path` from `Checked::judged`, which is why a Mac where it says
            // nothing is worth stopping for — see `skip_or_stop`.
            skip_or_stop(asked_as, aliasing);
            return;
        }
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let p = Plan {
            entries: vec![entry(asked.clone(), NodeKind::Dir, 10)],
            mode: Mode::Trash,
        };
        let port = Recording::new(&sys);
        let outcome = execute(&preview(&p, &limits, &port), &limits, &port);
        assert_eq!(
            port.done(),
            vec![Deletion::Trashed(asked.clone())],
            "the spelling the caller wrote, not the one the rules judged"
        );
        assert!(matches!(
            outcome.entries[0].result,
            EntryResult::Removed { .. }
        ));
        assert_eq!(
            outcome.entries[0].path, asked,
            "and the log names what was handed to the port"
        );
        assert!(
            fs::symlink_metadata(&asked).is_err(),
            "the directory really left, under whichever of its names"
        );
    }

    #[test]
    fn the_port_is_asked_for_the_spelling_the_caller_wrote() {
        // The name on disk always opens itself; the assertion is the same one, and the
        // pairs below are what can tell the two forms of a `Checked` apart.
        assert_the_port_is_asked_for("Data", "Data", Aliasing::FoldsOnEveryMac);
        assert_the_port_is_asked_for("Data", "data", Aliasing::MayNotFold);
        // No user error at all: the same name typed on macOS arrives in NFC from one source
        // and in NFD from another, and APFS treats them as one directory.
        assert_the_port_is_asked_for("caf\u{e9}", "cafe\u{301}", Aliasing::FoldsOnEveryMac);
        assert_the_port_is_asked_for("cafe\u{301}", "caf\u{e9}", Aliasing::FoldsOnEveryMac);
    }

    #[test]
    fn a_permanent_deletion_that_fails_keeps_the_entry_and_says_why() {
        // The plan's failure test runs in `Trash` mode; the other call has to report its
        // error too, and the message is what the user is shown.
        let sys = TestSystem::new();
        let file = sys.root().join("a.bin");
        fs::write(&file, b"x").unwrap();
        let p = plan(&sys, &["a.bin"], Mode::Permanent);
        let limits = Limits::new(sys.root().to_path_buf(), vec![]);
        let checked = preview(&p, &limits, &sys);
        sys.fail_next(&file);
        let outcome = execute(&checked, &limits, &sys);
        let EntryResult::Failed { message } = &outcome.entries[0].result else {
            panic!("expected a failure, got {:?}", outcome.entries[0].result);
        };
        assert!(
            message.contains(&file.display().to_string()),
            "the message names the entry, got {message:?}"
        );
        assert_eq!(fs::read(&file).unwrap(), b"x", "and nothing was deleted");
        assert_eq!(outcome.freed_bytes, 0);
    }
}
