//! The stages a deletion goes through. This one only decides: it reads the filesystem and
//! writes nothing, so a preview can be built, shown and thrown away at will.
//!
//! Every entry of the plan comes back, in its place, whether or not it survived the checks
//! — the dialog lists what the user selected next to what will happen to each row, and a
//! silently shortened list would be a dialog that lies about the selection.

use std::path::PathBuf;

use crate::scan::NodeKind;
use crate::system::{System, SystemError};

use super::guards::{Limits, drop_nested};
use super::model::{BlockReason, EntryStatus, Plan, PlanEntry, Preview, PreviewEntry};

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
        // The guard judged where the path points, without insisting anything is there.
        Err(SystemError::Missing(_)) => blocked(checked.path, BlockReason::Missing),
        // The port refuses a path that is not absolute and in normal form, and `check` is
        // what produces that form — so this is a bug on this side of the port, not a state
        // the user can do anything about. `Malformed` is at least the truth about the path;
        // `Unreadable` below means "grant Full Disk Access", which would be an instruction
        // to go and fix the wrong thing.
        Err(SystemError::Rejected { .. }) => blocked(checked.path, BlockReason::Malformed),
        // Permissions that changed under us, a loop, a variant added later: whatever it is,
        // nobody can look at this entry, and that is not one to delete on a guess.
        Err(_) => blocked(checked.path, BlockReason::Unreadable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // `Mode` is named by the tests but not by the code above them; Task 4's `execute`
    // matches on it and can drop this line.
    use crate::action::Mode;
    use crate::scan::NodeKind;
    use crate::system::TestSystem;
    use chrono::{DateTime, Utc};
    use std::fs::{self, Metadata};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    /// A port that cannot look at anything, records what it was asked about, and screams if
    /// asked to delete.
    ///
    /// `Limits::check` stats the entry too, so on a real filesystem it reports anything but
    /// "not found" before the preview ever gets to look. What is left for the preview to
    /// meet is a race — the entry was readable a syscall ago and is not now — and a race is
    /// not a thing to reproduce with `chmod`.
    struct Failing {
        /// What `symlink_metadata` answers. A function, because which `SystemError` arrives
        /// is the whole point of two of the tests below.
        error: fn(&Path) -> SystemError,
        /// Every path the port was asked about, in order. Without it a test can only check
        /// what `preview` stored, never what it went and looked at.
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
            unreachable!("a preview asked to trash {}", path.display());
        }

        fn remove(&self, path: &Path) -> Result<(), SystemError> {
            unreachable!("a preview asked to remove {}", path.display());
        }

        fn now(&self) -> DateTime<Utc> {
            Utc::now()
        }
    }

    fn plan(sys: &TestSystem, names: &[&str], mode: Mode) -> Plan {
        Plan {
            entries: names
                .iter()
                .map(|n| PlanEntry {
                    path: sys.root().join(n),
                    kind: NodeKind::File,
                    size: 10,
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
    fn assert_one_directory_counted_once(on_disk: &str, alias: &str) {
        let sys = TestSystem::new();
        let real = sys.root().join(on_disk);
        fs::create_dir(&real).unwrap();
        let aliased = sys.root().join(alias);
        if !aliased.is_dir() {
            // A case- and normalization-sensitive volume: two names, two directories, and
            // nothing here for the rule to do. Asserting the duller outcome instead would
            // let a green run on such a volume read as coverage it does not have — and CI
            // runs the core tests on Linux as well as on macOS.
            eprintln!("skipped: {alias} is its own name on this volume");
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
        assert_one_directory_counted_once("Data", "data");
    }

    #[test]
    fn another_normalization_of_one_name_is_not_counted_twice() {
        // No user error at all: the same name typed on macOS arrives in NFC from one source
        // and in NFD from another, and APFS treats them as one directory.
        assert_one_directory_counted_once("caf\u{e9}", "cafe\u{301}");
        assert_one_directory_counted_once("cafe\u{301}", "caf\u{e9}");
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
}
