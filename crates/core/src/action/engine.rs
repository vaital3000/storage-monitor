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
    let mut entries: Vec<PreviewEntry> = plan
        .entries
        .iter()
        .map(|entry| check_entry(entry, limits, sys))
        .collect();

    // Only the entries that are still ready take part: an entry that will not be deleted
    // cannot swallow the one below it. Selecting the root row together with a file inside
    // it is one click in a tree view, and that file still has to be deletable.
    //
    // The paths are the ones `Limits::check` returned, never the ones the plan carried:
    // `<root>/link/inner` and `<root>/real/inner` are one directory, and only the resolved
    // spelling shows it. `drop_nested` cannot tell the difference itself — its
    // `debug_assert` weighs absoluteness and nothing else, and says nothing at all in a
    // release build.
    let ready: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.status == EntryStatus::Ready)
        .map(|(index, _)| index)
        .collect();
    let paths: Vec<PathBuf> = ready
        .iter()
        .map(|&index| entries[index].path.clone())
        .collect();
    for (&index, kept) in ready.iter().zip(drop_nested(&paths)) {
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

/// One entry against the guards and the disk, before the batch is looked at as a whole.
///
/// The size is the plan's throughout — the number the Explorer showed the user, which is
/// what the dialog is about — while the kind is re-read for the entries that get that far.
/// Only that one is worth a syscall: a stale size costs nothing, and a stale kind deletes
/// the wrong thing.
fn check_entry(entry: &PlanEntry, limits: &Limits, sys: &dyn System) -> PreviewEntry {
    let blocked = |path: PathBuf, reason| PreviewEntry {
        path,
        // Nothing was successfully looked at, so there is no better answer than the plan's.
        kind: entry.kind,
        size: entry.size,
        status: EntryStatus::Blocked(reason),
    };
    // Reports `Missing`, `Unreadable` and `Malformed` besides the placement rules, and each
    // of them means something different to the person reading the dialog. Pass them on.
    let path = match limits.check(&entry.path) {
        Ok(path) => path,
        // Without a normalized path the only honest thing to show is what was asked for.
        Err(reason) => return blocked(entry.path.clone(), reason),
    };
    match sys.symlink_metadata(&path) {
        Ok(meta) => PreviewEntry {
            path,
            // The walker's own classifier, so that a socket or a fifo is `Other` here as
            // well. A private `if is_dir { Dir } else { File }` would disagree with the
            // scan about every such entry, and the re-validation before the deletion would
            // refuse them all as `KindChanged`, for ever.
            kind: NodeKind::from_metadata(&meta),
            size: entry.size,
            status: EntryStatus::Ready,
        },
        // The guard judged where the path points, without insisting anything is there.
        Err(SystemError::Missing(_)) => blocked(path, BlockReason::Missing),
        // Permissions that changed under us, a loop, a path the port refuses: whatever it
        // is, nobody can look at this entry, and that is not one to delete on a guess.
        Err(_) => blocked(path, BlockReason::Unreadable),
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

    /// A port that cannot look at anything, and that screams if asked to delete.
    ///
    /// `Limits::check` stats the entry too, so on a real filesystem it reports anything but
    /// "not found" before the preview ever gets to look. What is left for the preview to
    /// meet is a race — the entry was readable a syscall ago and is not now — and a race is
    /// not a thing to reproduce with `chmod`.
    struct Unreadable;

    impl System for Unreadable {
        fn symlink_metadata(&self, path: &Path) -> Result<Metadata, SystemError> {
            Err(SystemError::Metadata {
                path: path.to_path_buf(),
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            })
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

    /// One entry with a path the `plan` helper cannot spell.
    fn entry(path: PathBuf, size: u64) -> PlanEntry {
        PlanEntry {
            path,
            kind: NodeKind::File,
            size,
        }
    }

    #[test]
    fn ready_entries_are_totalled_and_blocked_ones_are_not() {
        let sys = TestSystem::new();
        fs::write(sys.root().join("a.bin"), b"x").unwrap();
        let mut p = plan(&sys, &["a.bin", "gone.bin"], Mode::Trash);
        p.entries[1].size = 999;
        let preview = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        assert_eq!(preview.entries[0].status, EntryStatus::Ready);
        assert_eq!(
            preview.entries[1].status,
            EntryStatus::Blocked(BlockReason::Missing)
        );
        assert_eq!(
            preview.total_bytes, 10,
            "a blocked entry contributes nothing"
        );
        assert_eq!(preview.mode, Mode::Trash);
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
        let preview = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        assert_eq!(
            preview.entries[1].status,
            EntryStatus::Blocked(BlockReason::Nested)
        );
        assert_eq!(
            preview.total_bytes, 100,
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
        assert!(file.exists());
        assert_eq!(fs::read_dir(sys.trash_dir()).unwrap().count(), 0);
    }

    #[test]
    fn the_kind_recorded_in_the_preview_comes_from_the_disk() {
        let sys = TestSystem::new();
        fs::create_dir(sys.root().join("a.bin")).unwrap();
        let p = plan(&sys, &["a.bin"], Mode::Trash); // planned as a File
        let preview = preview(&p, &Limits::new(sys.root().to_path_buf(), vec![]), &sys);
        assert_eq!(preview.entries[0].kind, NodeKind::Dir);
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
                entry(PathBuf::from("/"), 1), // no last component to delete
                entry(a.join("x.bin"), 2),    // the parent resolves to a loop
                // Nothing is there — spelled through a symlinked parent, so that the path
                // the guards examined and the one the plan carried can be told apart.
                entry(sys.root().join("link/gone.bin"), 4),
                entry(sys.root().join("keep.bin"), 8),
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
        // plan carried are two different paths and the assertion below can tell them apart.
        let p = plan(&sys, &["link/a.bin"], Mode::Trash);
        let checked = preview(
            &p,
            &Limits::new(sys.root().to_path_buf(), vec![]),
            &Unreadable,
        );
        assert_eq!(
            checked.entries[0].status,
            EntryStatus::Blocked(BlockReason::Unreadable)
        );
        assert_eq!(
            checked.entries[0].path,
            sys.root().join("real/a.bin"),
            "an entry blocked after the guards reports the path they examined"
        );
        // Nothing was read, so both come from the plan; the dialog still has a row to show.
        assert_eq!(checked.entries[0].kind, NodeKind::File, "the plan's kind");
        assert_eq!(checked.entries[0].size, 10, "and the plan's size");
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

    #[test]
    fn a_blocked_ancestor_neither_nests_what_is_below_it_nor_moves_it() {
        // Selecting the root row together with things inside it costs one click in a tree
        // view. The root is refused and stays where it is, so what is below it is not
        // inside anything that is about to be deleted — and the entries that follow a
        // blocked one must keep their own places in the list.
        let sys = TestSystem::new();
        fs::create_dir_all(sys.root().join("dir/inner")).unwrap();
        let p = Plan {
            entries: vec![
                PlanEntry {
                    path: sys.root().to_path_buf(),
                    kind: NodeKind::Dir,
                    size: 1_000,
                },
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
        assert_eq!(checked.total_bytes, 100);
    }
}
