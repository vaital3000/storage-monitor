//! The safety rules: which paths may be deleted at all.
//!
//! Decisions only: nothing here deletes, moves or writes. It does read the filesystem —
//! `canonicalize` to normalize a path, and one `symlink_metadata` to learn whether the
//! entry is a symlink — because a rule that judged the spelling the caller happened to
//! choose would judge almost nothing. On a case-insensitive volume `Library` and `library`
//! are one directory; on macOS `/etc` and `/private/etc` are one directory everywhere.
//! Every deletion passes [`Limits::check`] first, and what it returns — not the path as it
//! arrived — is what the `System` port is handed.

use std::path::{Path, PathBuf};

use super::BlockReason;

/// The names under `~/Library` that stay denied outright, contents and all (ADR 0007).
///
/// The list is short because everything on it shares one property: it is never a place
/// anyone goes to reclaim space, so denying it costs no gigabytes. Two of them cost more
/// than the rest to get wrong. `Mobile Documents` is iCloud Drive and `CloudStorage` is
/// where Dropbox, OneDrive and Google Drive mount their file providers: they sit on the
/// data volume like ordinary folders — measured, not assumed — so nothing structural tells
/// them apart, and the Trash does not undo them. Moving a file-provider item to the Trash
/// *is* the deletion, on every device signed into that account.
///
/// Names absent from a given Mac cost nothing: a denied entry that resolves to nothing
/// matches nothing (`a_denied_path_nothing_can_resolve_stays_powerless`).
const HOME_LIBRARY_DENIED: [&str; 16] = [
    "Accounts",
    "Application Scripts",
    "Autosave Information",
    "Calendars",
    "CloudStorage",
    "Contacts",
    "Cookies",
    "IdentityServices",
    "Keychains",
    "Mail",
    "Messages",
    "Mobile Documents",
    "Photos",
    "Preferences",
    "Reminders",
    "Safari",
];

/// The folders under `~/Library` that are refused as an entry while their contents are not.
///
/// These three hold application data: expensive to lose in one tick, and the ordinary way
/// to reclaim space from them is one application at a time. `~/Library` itself is shielded
/// beside them, in [`Limits::with_home`], which is what makes `Caches`, `Developer`,
/// `Logs` and the rest of it deletable at all.
const HOME_LIBRARY_SHIELDED: [&str; 3] = ["Application Support", "Containers", "Group Containers"];

/// What [`Limits::check`] decided about one path: the two forms the engine needs.
///
/// They differ in the last component alone, and only when the disk spells it otherwise than
/// the caller did. Keeping both is what lets one entry be deleted by the name it was asked
/// for while the batch reasons about which entries are the same thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// The normalized path: absolute, in the port's normal form, and carrying the caller's
    /// own last component. This is what gets deleted.
    pub path: PathBuf,
    /// The same entry in the spelling the rules judged: fully resolved — except for a
    /// symlink, which is judged as written, because deleting one leaves its target alone.
    ///
    /// Never a path to delete; [`Self::path`] is. It is the only form in which two entries
    /// of one batch can be compared, and [`drop_nested`] is given these. `Data` and `data`,
    /// or one name in NFC and in NFD, are a single directory on a stock macOS volume and
    /// two different [`Path`]s: a batch that compared [`Self::path`] would promise their
    /// bytes twice and then fail to delete whichever came second.
    ///
    /// The symlink exception carries that gap with it, deliberately: two spellings of one
    /// *symlink* resolve to nothing and so are not collapsed either. Both stay ready, and
    /// once the first is gone the second is skipped as missing — while `total_bytes` does
    /// not move, because the size a scan records for a symlink is its own allocated blocks,
    /// which is 0. Resolving them instead would hand a link the identity of its target, and
    /// selecting both would then block a deletion the user really did ask for. A cosmetic
    /// row is the cheaper of the two mistakes.
    pub judged: PathBuf,
}

/// Where deletion is allowed and where it is never allowed.
#[derive(Debug, Clone)]
pub struct Limits {
    /// The scan root, fully resolved: what rule 5 measures every entry against.
    root: PathBuf,
    /// The root as the caller spelled it, with only its parent resolved. Differs from
    /// `root` when the root itself is a symlink, and rule 4 has to refuse both spellings.
    root_as_given: PathBuf,
    /// Every denied path in both the spellings `forms` produces.
    denied: Vec<PathBuf>,
    /// Every shielded path in both those spellings: refused as an entry, and as nothing
    /// else. Unlike `denied` these are never dropped for containing the root, and they do
    /// not need to be — rule 4 reaches a shielded path that is the root or above it first,
    /// and rule 5 reaches one that is outside it.
    shielded: Vec<PathBuf>,
}

impl Limits {
    /// `root` and `denied` are resolved here, each kept in both the spellings a path can
    /// have: with only its parent resolved, and fully resolved.
    ///
    /// `root` must be absolute, the way a scan produces it.
    ///
    /// A path that does not exist keeps whatever spelling it was handed; it then matches
    /// only a checked path that resolves to exactly that, which on macOS usually means
    /// never, since a real path resolves through `/private` or the case on disk.
    ///
    /// A denied entry that contains the root is dropped, so pointing the scan root at a
    /// denied folder unlocks it: limits built for the root `~/Library` hand out
    /// `~/Library/Keychains` as an ordinary deletable row. That is the policy and not an
    /// accident — the denylist keeps a scan of the home folder from wandering into places
    /// the user never meant to visit, and a root the user named is a place they did mean
    /// to visit. The rule is also what makes the standard denylist usable at all: `/` is
    /// the ancestor of everything, so without it every entry of every batch would be
    /// refused. Do not "restore" it.
    pub fn new(root: PathBuf, denied: Vec<PathBuf>) -> Self {
        let root_as_given = parent_resolved(&root);
        let root = root
            .canonicalize()
            .unwrap_or_else(|_| root_as_given.clone());
        // An empty root would pass rules 4 and 5 for every path on the machine, since
        // everything starts with the empty path. A relative one refuses everything
        // instead. Both are callers' mistakes rather than states to handle.
        debug_assert!(
            root.is_absolute(),
            "a scan root must be absolute, got {}",
            root.display()
        );
        let denied = denied
            .iter()
            .flat_map(|d| forms(d))
            // Against the resolved root, since that is what rules 5 and 6 judge: a form the
            // resolved root is not under can never swallow it. What makes dropping safe is
            // rule 4 running first and testing both root spellings — reorder them and a
            // dropped entry stops being covered.
            .filter(|d| !root.starts_with(d))
            .collect();
        Self {
            root,
            root_as_given,
            denied,
            shielded: Vec::new(),
        }
    }

    /// Adds paths that are refused as an entry while everything inside them is judged on
    /// its own. Rule 7; [`BlockReason::Shielded`] says what that means to a user.
    #[must_use]
    pub fn shielding(mut self, shielded: Vec<PathBuf>) -> Self {
        self.shielded = shielded.iter().flat_map(|s| forms(s)).collect();
        self
    }

    /// The root plus the standard denylist of the design (section 5).
    pub fn for_scan_root(root: PathBuf) -> Self {
        Self::with_home(root, crate::paths::home_dir())
    }

    /// The seam of [`Self::for_scan_root`]: tests pass a temporary directory as the home
    /// folder instead of the one the machine running them happens to have.
    fn with_home(root: PathBuf, home: Option<PathBuf>) -> Self {
        let mut denied = vec![
            PathBuf::from("/"),
            PathBuf::from("/System"),
            PathBuf::from("/usr"),
            PathBuf::from("/bin"),
            PathBuf::from("/sbin"),
            PathBuf::from("/Library"),
            PathBuf::from("/Applications"),
            PathBuf::from("/opt"),
            PathBuf::from("/cores"),
            // Other people's home folders, and whatever is mounted. A scan rooted inside
            // one of them still works: `Limits::new` drops whichever entry contains the
            // root, so a `/Volumes/Backup` root keeps its own contents deletable.
            PathBuf::from("/Users"),
            PathBuf::from("/Volumes"),
            // On macOS these are where much of the above actually lives, and the first
            // three are symlinks into the fourth: `/etc` is `/private/etc`. `Limits::new`
            // keeps both spellings, which is what makes denying them work from either side.
            PathBuf::from("/etc"),
            PathBuf::from("/var"),
            PathBuf::from("/tmp"),
            PathBuf::from("/private"),
        ];
        let mut shielded = Vec::new();
        if let Some(home) = home {
            let library = home.join("Library");
            denied.extend(HOME_LIBRARY_DENIED.iter().map(|name| library.join(name)));
            shielded.extend(HOME_LIBRARY_SHIELDED.iter().map(|name| library.join(name)));
            shielded.push(library);
            denied.push(home);
        }
        Self::new(root, denied).shielding(shielded)
    }

    /// Normalizes `path` and applies every rule. [`Checked::path`] is what the engine
    /// deletes — the caller's own last component, never the target of a link — and
    /// [`Checked::judged`] is the form the rules were applied to, which the engine needs in
    /// turn to tell two spellings of one entry apart.
    ///
    /// The rules are numbered as in the design; [`Self::new`] refers to rule 4.
    pub fn check(&self, path: &Path) -> Result<Checked, BlockReason> {
        // 1. No last component to speak of: this is `/`, or a path ending in `..`. Both
        //    name something other than they appear to — `remove("/a/b/..")` empties `/a`.
        //    (A path with a file name always has a parent, so the two fail together.)
        let (Some(name), Some(parent)) = (path.file_name(), path.parent()) else {
            return Err(BlockReason::Malformed);
        };
        // 2. Only the parent is resolved. Not every failure means the entry is gone: a
        //    directory above it without `+x`, a symlink loop, a component that is not a
        //    directory all say "we cannot look", which is a different thing to tell a user
        //    — especially on macOS, where the answer is usually Full Disk Access.
        let parent = parent.canonicalize().map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => BlockReason::Missing,
            _ => BlockReason::Unreadable,
        })?;
        // 3. Canonicalizing the whole path would follow a symlink to its target and delete
        //    that instead; resolving the parent alone is what stops a symlinked directory
        //    in the middle of the path from smuggling the entry elsewhere. It also makes
        //    the result absolute and in normal form, which is what the port demands, even
        //    when the scan root was relative.
        let normalized = parent.join(name);
        let judged = judged_form(&normalized)?;
        // 4. The root itself and every ancestor of it (`starts_with` includes equality), in
        //    both spellings: a symlinked root has to recognise itself, or deleting the root
        //    row would come back as `OutsideRoots` — true, and no help to anyone.
        if self.root.starts_with(&judged) || self.root_as_given.starts_with(&judged) {
            return Err(BlockReason::IsRoot);
        }
        // 5. Component-wise, so `/h/ab` is not inside `/h/a`.
        if !judged.starts_with(&self.root) {
            return Err(BlockReason::OutsideRoots);
        }
        // 6. The denied entry itself (equality again) and everything below it.
        if self.denied.iter().any(|d| judged.starts_with(d)) {
            return Err(BlockReason::Denylisted);
        }
        // 7. A shielded entry, and *only* the entry: equality, never `starts_with`. That one
        //    operator is the whole difference between this rule and rule 6, and it is why
        //    rule 6 runs first — `~/Library` is shielded and `~/Library/Keychains` is denied,
        //    so the denied answer has to win inside a shield it lives in.
        if self.shielded.contains(&judged) {
            return Err(BlockReason::Shielded);
        }
        Ok(Checked {
            path: normalized,
            judged,
        })
    }
}

/// The form the rules are applied to, which is not always the form that gets deleted.
///
/// The last component of a normalized path is still spelled the way the caller spelled it,
/// and a rule that trusted that spelling would be a rule in name only: on a
/// case-insensitive volume a denied `Library` would let `library` through, and it is the
/// same directory. So anything that is not a symlink is judged fully resolved — one
/// `symlink_metadata` to find that out, one `canonicalize` to do it. A symlink is judged
/// as written, because deleting one removes the link and leaves its target alone, so where
/// that target lives must not decide the verdict.
fn judged_form(normalized: &Path) -> Result<PathBuf, BlockReason> {
    match std::fs::symlink_metadata(normalized) {
        Ok(meta) if meta.file_type().is_symlink() => Ok(normalized.to_path_buf()),
        // The one fallback in here that judges the spelling the caller wrote: the entry was
        // there a syscall ago and cannot be resolved now, so something is moving underneath
        // us. Narrow enough to leave — the rules still run, and `preview` stats the entry
        // again — but it is the branch to look at first if a verdict ever surprises anyone.
        Ok(_) => Ok(normalized
            .canonicalize()
            .unwrap_or_else(|_| normalized.to_path_buf())),
        // Not there: the rules still run — the path may be denied or outside the root —
        // and `preview` is what turns a survivor into `Blocked(Missing)` a moment later.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(normalized.to_path_buf()),
        // There, or not: we cannot tell, and an entry nobody can look at is not one to
        // delete on a guess.
        Err(_) => Err(BlockReason::Unreadable),
    }
}

/// Every spelling of a path the rules must recognise: with only its parent resolved, and
/// fully resolved. The two differ when the path is a symlink — `/etc` is also
/// `/private/etc` — and when the disk spells the name differently, and a denylist that
/// knew only one of them would guard only one of the two doors.
fn forms(path: &Path) -> Vec<PathBuf> {
    let mut forms = vec![parent_resolved(path)];
    match path.canonicalize() {
        Ok(resolved) if resolved != forms[0] => forms.push(resolved),
        _ => {}
    }
    forms
}

/// The path with only its parent resolved: the form a symlink is judged and deleted by.
/// Falls back to the path as given when there is no parent, or it cannot be read.
fn parent_resolved(path: &Path) -> PathBuf {
    match (path.file_name(), path.parent()) {
        (Some(name), Some(parent)) => match parent.canonicalize() {
            Ok(parent) => parent.join(name),
            Err(_) => path.to_path_buf(),
        },
        _ => path.to_path_buf(),
    }
}

/// Per entry, in input order: `false` when another entry of the batch contains it.
///
/// Selecting a directory and something inside it costs one click in a tree view. Without
/// this the descendant's bytes would be counted twice and its deletion would fail with "no
/// such file" once the ancestor is gone. Of several copies of one path the first survives.
///
/// Takes the [`Checked::judged`] forms [`Limits::check`] returned, not the paths it hands
/// on to the port: those keep the caller's spelling of the last component, and two
/// spellings of one directory would then look like two entries. Anything else makes the
/// answer meaningless rather than merely wrong — every path starts with the empty one, so a
/// single empty entry would drop the whole batch.
///
/// Quadratic. The 500 entries of a full listing (`DEFAULT_CHILDREN_LIMIT` in the desktop
/// crate, the most a selection can hold) cost about 12 ms in a release build and 17 ms in
/// a debug one, measured with no nesting, which is the worst case since nothing
/// short-circuits. So the cheapest obviously correct thing stays.
pub fn drop_nested(paths: &[PathBuf]) -> Vec<bool> {
    debug_assert!(
        paths.iter().all(|path| path.is_absolute()),
        "drop_nested takes the paths Limits::check returned"
    );
    paths
        .iter()
        .enumerate()
        .map(|(i, path)| {
            !paths.iter().enumerate().any(|(j, other)| {
                // A strict ancestor always wins; between equals, the earlier one does.
                i != j && path.starts_with(other) && (other != path || j < i)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use serde::Deserialize;

    /// The scenarios of `tests/fixtures/guard-cases.json`, which the mock of the desktop app
    /// answers as well. The file itself says what it is for and what `ready` means there.
    #[derive(Deserialize)]
    struct GuardCases {
        scenarios: Vec<GuardScenario>,
        nesting: Vec<NestingCase>,
    }

    #[derive(Deserialize)]
    struct NestingCase {
        /// Relative to the scenario root each side uses; `drop_nested` takes absolute paths.
        paths: Vec<String>,
        keep: Vec<bool>,
        why: String,
    }

    #[derive(Deserialize)]
    struct GuardScenario {
        name: String,
        /// Relative to the base of the scenario; empty is the base itself.
        root: String,
        home: String,
        tree: GuardTree,
        cases: Vec<GuardCase>,
    }

    #[derive(Deserialize)]
    struct GuardTree {
        dirs: Vec<String>,
        files: Vec<String>,
    }

    #[derive(Deserialize)]
    struct GuardCase {
        base: String,
        path: String,
        expect: String,
        why: String,
    }

    fn under(base: &Path, relative: &str) -> PathBuf {
        if relative.is_empty() {
            base.to_path_buf()
        } else {
            base.join(relative)
        }
    }

    /// The verdict as the shared cases name it: the wire name of the reason, or `ready`.
    fn verdict(checked: &Result<Checked, BlockReason>) -> String {
        match checked {
            Ok(_) => "ready".to_owned(),
            Err(reason) => match serde_json::to_value(reason) {
                Ok(serde_json::Value::String(name)) => name,
                other => panic!("a block reason is a string on the wire, got {other:?}"),
            },
        }
    }

    /// The cases the desktop mock answers too, so that a rule cannot change on one side
    /// alone. The mock is the oracle every UI test of the deletion is written against, and
    /// nothing else compares the two.
    #[test]
    fn the_shared_guard_cases_hold() {
        let text = fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/guard-cases.json"
        ))
        .expect("the shared guard cases are next to this crate");
        let file: GuardCases = serde_json::from_str(&text).expect("the shared cases parse");
        assert!(!file.scenarios.is_empty(), "there are cases to run");
        for scenario in &file.scenarios {
            let base = tempfile::tempdir().unwrap();
            for dir in &scenario.tree.dirs {
                fs::create_dir_all(base.path().join(dir)).unwrap();
            }
            for path in &scenario.tree.files {
                fs::write(base.path().join(path), b"x").unwrap();
            }
            let root = under(base.path(), &scenario.root);
            let home = under(base.path(), &scenario.home);
            let limits = Limits::with_home(root.clone(), Some(home.clone()));
            let parent = root
                .parent()
                .expect("a scan root has a parent")
                .to_path_buf();
            for case in &scenario.cases {
                let path = match case.base.as_str() {
                    "root" => under(&root, &case.path),
                    "home" => under(&home, &case.path),
                    "parent" => under(&parent, &case.path),
                    // The root's own path with the case appended to its last component: the
                    // neighbour whose name begins with the root's, which only a comparison
                    // that is not component-wise would call "inside".
                    "sibling" => {
                        let mut name = root.as_os_str().to_owned();
                        name.push(&case.path);
                        PathBuf::from(name)
                    }
                    "absolute" => PathBuf::from(&case.path),
                    other => panic!("unknown base {other} in the shared cases"),
                };
                assert_eq!(
                    verdict(&limits.check(&path)),
                    case.expect,
                    "{}: {} {} — {}",
                    scenario.name,
                    case.base,
                    case.path,
                    case.why
                );
            }
        }
        the_shared_nesting_cases(&file.nesting);
    }

    /// The other half of the shared set: which entries of one batch swallow the others.
    ///
    /// `drop_nested` needs no disk and no `System`, so it is on this side of the boundary
    /// the shared cases keep — it only sat outside it because the boundary was drawn by
    /// which file a function lives in.
    fn the_shared_nesting_cases(cases: &[NestingCase]) {
        assert!(!cases.is_empty(), "there are nesting cases to run");
        let root = PathBuf::from("/h");
        for case in cases {
            assert_eq!(
                case.paths.len(),
                case.keep.len(),
                "a nesting case answers every path it lists: {}",
                case.why
            );
            let paths: Vec<PathBuf> = case.paths.iter().map(|path| under(&root, path)).collect();
            assert_eq!(
                drop_nested(&paths),
                case.keep,
                "{:?} — {}",
                case.paths,
                case.why
            );
        }
    }

    fn limits(root: &Path) -> Limits {
        Limits::new(root.to_path_buf(), vec![root.join("Library")])
    }

    /// APFS is case-insensitive unless the volume was formatted otherwise, and Linux is
    /// case-sensitive. The rule under test only has anything to do on the first kind, so
    /// the test asks the directory it is about to use instead of guessing from the OS.
    fn case_insensitive(dir: &Path) -> bool {
        let probe = dir.join("CaseProbe");
        fs::create_dir(&probe).unwrap();
        let answer = dir.join("caseprobe").is_dir();
        fs::remove_dir(&probe).unwrap();
        answer
    }

    /// The `System` port refuses anything that is not absolute and in normal form, and
    /// `check` is what produces that form. Mirrored here rather than called, so a guard
    /// test fails before a deletion ever reaches the port.
    fn assert_port_normal_form(path: &Path) {
        assert!(path.is_absolute(), "{} is not absolute", path.display());
        let normal: PathBuf = path.components().collect();
        assert_eq!(
            normal.as_os_str(),
            path.as_os_str(),
            "{} is not in normal form",
            path.display()
        );
    }

    #[test]
    fn a_path_inside_the_root_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("keep/a.bin");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"x").unwrap();
        let checked = limits(dir.path()).check(&file).unwrap();
        assert_eq!(checked.path, fs::canonicalize(&file).unwrap());
        assert_eq!(
            checked.judged, checked.path,
            "an ordinary file resolves to itself, so the two forms agree"
        );
    }

    #[test]
    fn the_scan_root_itself_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            limits(dir.path()).check(dir.path()),
            Err(BlockReason::IsRoot)
        );
    }

    #[test]
    fn an_ancestor_of_the_root_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("home/user");
        fs::create_dir_all(&root).unwrap();
        assert_eq!(
            limits(&root).check(&dir.path().join("home")),
            Err(BlockReason::IsRoot)
        );
    }

    #[test]
    fn a_path_outside_the_root_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("home");
        fs::create_dir_all(&root).unwrap();
        let outside = dir.path().join("elsewhere");
        fs::create_dir(&outside).unwrap();
        assert_eq!(
            limits(&root).check(&outside),
            Err(BlockReason::OutsideRoots)
        );
    }

    #[test]
    fn a_denylisted_subtree_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let inside = dir.path().join("Library/Caches/app");
        fs::create_dir_all(&inside).unwrap();
        assert_eq!(
            limits(dir.path()).check(&inside),
            Err(BlockReason::Denylisted)
        );
    }

    #[test]
    fn a_path_whose_parent_is_gone_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            limits(dir.path()).check(&dir.path().join("gone/a.bin")),
            Err(BlockReason::Missing)
        );
    }

    #[test]
    fn a_missing_entry_under_a_live_parent_is_allowed() {
        // The guard rules on where a path points, not on whether anything is there. The
        // entry is stat'ed by `preview`, which is what turns this into `Blocked(Missing)`
        // a moment later — and the guard must not pretend to have looked.
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            limits(dir.path())
                .check(&dir.path().join("gone.bin"))
                .unwrap()
                .path,
            fs::canonicalize(dir.path()).unwrap().join("gone.bin")
        );
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
        let checked = limits(&root).check(&link).unwrap();
        assert_eq!(checked.path, fs::canonicalize(&root).unwrap().join("link"));
        assert_eq!(
            checked.judged, checked.path,
            "a symlink is judged as written; resolving it here would put the batch's \
             comparison on the target and let a link swallow a sibling entry"
        );
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
        // — the denylist never gets a word in, since "/" has no last component to judge,
        // and `Limits::new` drops "/" from the denylist anyway as an ancestor of every
        // root. One rule, so it gets its own test.
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            limits(dir.path()).check(Path::new("/")),
            Err(BlockReason::Malformed)
        );
    }

    #[test]
    fn nested_entries_collapse_to_their_ancestor() {
        let paths = vec![
            PathBuf::from("/h/a"),
            PathBuf::from("/h/a/b"),
            PathBuf::from("/h/c"),
            PathBuf::from("/h/ab"),
        ];
        assert_eq!(drop_nested(&paths), vec![true, false, true, true]);
    }

    // The rest are not in the plan: shapes that read as safe and resolve to something else.

    #[test]
    fn a_trailing_dot_dot_is_refused() {
        // `remove("<root>/sub/..")` would empty the root. `file_name` is `None` here, and
        // rule 1 is what catches it — as `Malformed`, not `Denylisted`: the path is not in
        // a forbidden place, it names no entry at all.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        assert_eq!(
            limits(dir.path()).check(&dir.path().join("sub/..")),
            Err(BlockReason::Malformed)
        );
    }

    #[test]
    fn a_symlinked_parent_cannot_smuggle_a_path_out_of_the_root() {
        // The last component is not resolved, but every component above it is: otherwise
        // `<root>/link/victim.bin` would delete a file outside the root.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("home");
        fs::create_dir(&root).unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("victim.bin"), b"x").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        assert_eq!(
            limits(&root).check(&root.join("link/victim.bin")),
            Err(BlockReason::OutsideRoots)
        );
    }

    #[test]
    fn a_trailing_separator_is_stripped_from_the_result() {
        // POSIX resolves a last component written as a directory by following it, so
        // `remove("<root>/link/")` deletes what the link points at. The port refuses that
        // shape; the guard must not hand it one.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("home");
        fs::create_dir(&root).unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        let with_slash = PathBuf::from(format!("{}/link/", root.display()));
        let checked = limits(&root).check(&with_slash).unwrap();
        assert_eq!(checked.path, fs::canonicalize(&root).unwrap().join("link"));
        assert_port_normal_form(&checked.path);
    }

    #[test]
    fn dots_inside_the_path_are_normalized_away() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("home");
        fs::create_dir_all(root.join("sub")).unwrap();
        let file = root.join("sub/a.bin");
        fs::write(&file, b"x").unwrap();
        let noisy = root.join("./sub/../sub/./a.bin");
        let checked = limits(&root).check(&noisy).unwrap();
        assert_eq!(checked.path, fs::canonicalize(&file).unwrap());
        assert_port_normal_form(&checked.path);
    }

    #[test]
    fn a_relative_path_comes_back_absolute() {
        // `Tree::path` keeps whatever the scan root was, and only the CLI normalizes it,
        // so a relative path can reach the guard; the port refuses one.
        let root = std::env::current_dir().unwrap(); // the crate directory under `cargo test`
        let file = root.join("Cargo.toml");
        let limits = Limits::new(root, vec![]);
        let checked = limits.check(Path::new("./Cargo.toml")).unwrap();
        assert_eq!(checked.path, fs::canonicalize(&file).unwrap());
        assert_port_normal_form(&checked.path);
    }

    #[test]
    fn a_denied_entry_that_contains_the_root_does_not_block_the_whole_root() {
        // Rule 4 already refuses the root and its ancestors, so such an entry protects
        // only itself; keeping it as a subtree rule would block every deletion.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("home");
        fs::create_dir(&root).unwrap();
        let file = root.join("a.bin");
        fs::write(&file, b"x").unwrap();
        let limits = Limits::new(root.clone(), vec![dir.path().to_path_buf(), root.clone()]);
        assert_eq!(
            limits.check(&file).unwrap().path,
            fs::canonicalize(&file).unwrap()
        );
        assert_eq!(limits.check(&root), Err(BlockReason::IsRoot));
        assert_eq!(limits.check(dir.path()), Err(BlockReason::IsRoot));
    }

    #[test]
    fn the_standard_denylist_leaves_an_ordinary_scan_root_usable() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.bin");
        fs::write(&file, b"x").unwrap();
        let limits = Limits::for_scan_root(dir.path().to_path_buf());
        assert_eq!(
            limits.check(&file).unwrap().path,
            fs::canonicalize(&file).unwrap()
        );
    }

    #[test]
    fn the_standard_limits_shield_the_library_folder_and_deny_the_few_names_inside_it() {
        // `for_scan_root` reads the real home folder, so the rule is exercised through the
        // seam it delegates to, with a temp directory standing in for the home.
        let dir = tempfile::tempdir().unwrap();
        // The order does not matter any more: `Limits::new` resolves the parent of a denied
        // path, so `<home>/Library` is recorded in the spelling a checked path resolves to
        // whether or not it exists yet. `a_denied_folder_that_appears_later_is_still_denied`
        // is the test for that; this one is about the rule itself.
        let caches = dir.path().join("Library/Caches");
        fs::create_dir_all(&caches).unwrap();
        let keychains = dir.path().join("Library/Keychains");
        fs::create_dir_all(&keychains).unwrap();
        let support = dir.path().join("Library/Application Support");
        fs::create_dir_all(support.join("JetBrains")).unwrap();
        let keep = dir.path().join("Downloads/a.bin");
        fs::create_dir_all(keep.parent().unwrap()).unwrap();
        fs::write(&keep, b"x").unwrap();
        let limits = Limits::with_home(dir.path().to_path_buf(), Some(dir.path().to_path_buf()));

        assert_eq!(
            limits.check(&dir.path().join("Library")),
            Err(BlockReason::Shielded),
            "~/Library is refused as an entry"
        );
        assert_eq!(
            limits.check(&support),
            Err(BlockReason::Shielded),
            "and so is each folder of application data under it"
        );
        // The positive control of the whole rule. A shield that took its contents with it
        // would be a denylist under another name, and would pass every assertion above.
        assert_eq!(
            limits.check(&caches).unwrap().path,
            fs::canonicalize(&caches).unwrap(),
            "what is inside a shield is judged on its own"
        );
        assert_eq!(
            limits.check(&support.join("JetBrains")).unwrap().path,
            fs::canonicalize(support.join("JetBrains")).unwrap(),
            "one application's data at a time, which is why the folder is shielded and not denied"
        );

        assert_eq!(
            limits.check(&keychains),
            Err(BlockReason::Denylisted),
            "the few names inside the shield that are denied outright"
        );
        assert_eq!(
            limits.check(&keychains.join("login.keychain-db")),
            Err(BlockReason::Denylisted),
            "with everything below them — the one difference between the two lists"
        );
        assert_eq!(
            limits.check(&keep).unwrap().path,
            fs::canonicalize(&keep).unwrap(),
            "the home folder in the denylist must not block the scan of the home folder"
        );
    }

    #[test]
    fn the_standard_denylist_protects_the_system_folders() {
        // The one scan root from which a denied folder is reachable at all: anywhere else
        // `/usr` is refused as `OutsideRoots` before the denylist is consulted.
        //
        // `/etc` and `/var` are symlinks into `/private` on macOS and ordinary directories
        // on Linux; `/Applications` and `/private` exist only on one of the two. All four
        // are refused either way, which is the point of keeping both spellings.
        let limits = Limits::for_scan_root(PathBuf::from("/"));
        for path in [
            "/usr",
            "/usr/lib",
            "/etc",
            "/etc/hosts",
            "/var",
            "/tmp",
            "/private",
            "/Applications",
            "/Users",
            "/Volumes",
            "/opt",
            "/cores",
        ] {
            assert_eq!(
                limits.check(Path::new(path)),
                Err(BlockReason::Denylisted),
                "{path}"
            );
        }
    }

    #[test]
    fn an_absent_denylist_entry_blocks_nothing_else() {
        // Not observable from outside that it was kept as given; what matters is that it
        // did not become something that matches everything, such as an empty path.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.bin");
        fs::write(&file, b"x").unwrap();
        let limits = Limits::new(
            dir.path().to_path_buf(),
            vec![dir.path().join("absent"), PathBuf::from("/no/such/place")],
        );
        assert_eq!(
            limits.check(&file).unwrap().path,
            fs::canonicalize(&file).unwrap()
        );
    }

    #[test]
    fn a_sibling_whose_name_extends_the_root_is_outside_it() {
        // The dangerous direction of a prefix check on bytes: `<tmp>/homework` would pass
        // for the root `<tmp>/home` and be deleted.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("home");
        fs::create_dir(&root).unwrap();
        let sibling = dir.path().join("homework/a.bin");
        fs::create_dir_all(sibling.parent().unwrap()).unwrap();
        fs::write(&sibling, b"x").unwrap();
        assert_eq!(
            limits(&root).check(&sibling),
            Err(BlockReason::OutsideRoots)
        );
    }

    #[test]
    fn a_sibling_whose_name_extends_a_denied_folder_is_allowed() {
        // The other half of the same hazard: `Library-backup` is not inside `Library`.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Library-backup/a.bin");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"x").unwrap();
        fs::create_dir(dir.path().join("Library")).unwrap();
        assert_eq!(
            limits(dir.path()).check(&file).unwrap().path,
            fs::canonicalize(&file).unwrap()
        );
    }

    #[test]
    fn a_repeated_entry_survives_once() {
        let paths = vec![
            PathBuf::from("/h/a"),
            PathBuf::from("/h/a"),
            PathBuf::from("/h/b"),
        ];
        assert_eq!(drop_nested(&paths), vec![true, false, true]);
    }

    #[test]
    fn a_descendant_is_dropped_however_the_batch_is_ordered() {
        let paths = vec![
            PathBuf::from("/h/a/b/c"),
            PathBuf::from("/h/a"),
            PathBuf::from("/h/a/b"),
        ];
        assert_eq!(drop_nested(&paths), vec![false, true, false]);
    }

    #[test]
    fn a_denied_folder_is_denied_in_every_spelling_the_volume_accepts() {
        // The last component keeps the caller's spelling — it has to, since that is what
        // gets deleted — so the rules judge the resolved form instead. Without that, a
        // denied `Library` stops nobody: `library` is the same directory.
        let dir = tempfile::tempdir().unwrap();
        let insensitive = case_insensitive(dir.path());
        fs::create_dir_all(dir.path().join("Library/Caches")).unwrap();
        let limits = limits(dir.path());
        assert_eq!(
            limits.check(&dir.path().join("Library")),
            Err(BlockReason::Denylisted),
            "the spelling on disk, on any volume"
        );
        for spelling in ["library", "LIBRARY"] {
            let verdict = limits.check(&dir.path().join(spelling));
            if insensitive {
                assert_eq!(
                    verdict,
                    Err(BlockReason::Denylisted),
                    "<root>/{spelling} opens the denied directory on this volume"
                );
            } else {
                // Case-sensitive: a different name, naming nothing, and denying it would
                // be denying a path that has nothing to do with the denied one.
                assert_eq!(
                    verdict.unwrap().path,
                    fs::canonicalize(dir.path()).unwrap().join(spelling),
                    "<root>/{spelling} is its own path on this volume"
                );
            }
        }
        let below = limits.check(&dir.path().join("library/Caches"));
        if insensitive {
            assert_eq!(below, Err(BlockReason::Denylisted), "and below it");
        } else {
            assert_eq!(below, Err(BlockReason::Missing), "and nothing is below it");
        }
    }

    #[test]
    fn a_shielded_folder_is_shielded_in_every_spelling_the_volume_accepts() {
        // The hazard the test above pins for rule 6, which rule 7 does not inherit for free:
        // it compares by equality where rule 6 compares by containment, and an equality is
        // the easier of the two to write against the spelling that happened to arrive. A
        // shield on `Library` that let `library` past would be no shield at all.
        let dir = tempfile::tempdir().unwrap();
        let insensitive = case_insensitive(dir.path());
        let caches = dir.path().join("Library/Caches");
        fs::create_dir_all(&caches).unwrap();
        let limits = Limits::new(dir.path().to_path_buf(), vec![])
            .shielding(vec![dir.path().join("Library")]);

        assert_eq!(
            limits.check(&dir.path().join("Library")),
            Err(BlockReason::Shielded),
            "the spelling on disk, on any volume"
        );
        for spelling in ["library", "LIBRARY"] {
            let verdict = limits.check(&dir.path().join(spelling));
            if insensitive {
                assert_eq!(
                    verdict,
                    Err(BlockReason::Shielded),
                    "<root>/{spelling} opens the shielded directory on this volume"
                );
            } else {
                // Case-sensitive: a different name, naming nothing, and shielding it would
                // be shielding a path that has nothing to do with the shielded one.
                assert_eq!(
                    verdict.unwrap().path,
                    fs::canonicalize(dir.path()).unwrap().join(spelling),
                    "<root>/{spelling} is its own path on this volume"
                );
            }
        }
        // The other half of the rule, and the half a `starts_with` would quietly take away.
        assert_eq!(
            limits.check(&caches).unwrap().path,
            fs::canonicalize(&caches).unwrap(),
            "the shield covers the folder and nothing under it"
        );
    }

    #[test]
    fn the_checked_path_keeps_the_last_component_the_caller_wrote() {
        // Judging the resolved form must not turn into returning it. Only the spelling of
        // a name can differ here, and on this volume either spelling opens the same
        // directory — but it is the same line of code that keeps a symlink from being
        // handed to the port as its target.
        // On a case-sensitive volume `data` is simply absent and the same assertion holds
        // for the duller reason, so the test branches on nothing: either way the answer is
        // the name that was asked for.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("Data")).unwrap();
        let limits = Limits::new(dir.path().to_path_buf(), vec![]);
        let checked = limits.check(&dir.path().join("data")).unwrap();
        assert_eq!(
            checked.path,
            fs::canonicalize(dir.path()).unwrap().join("data")
        );
        assert_port_normal_form(&checked.path);
        // The other half of the same decision: the form the rules judged carries the name
        // the disk has, and the engine compares the entries of a batch by it. Without it,
        // `Data` and `data` would be two entries and their bytes would be promised twice.
        if case_insensitive(dir.path()) {
            assert_eq!(
                checked.judged,
                fs::canonicalize(dir.path()).unwrap().join("Data"),
                "one directory, under the name it has on disk"
            );
        } else {
            assert_eq!(
                checked.judged, checked.path,
                "a different name on this volume, naming nothing, so there is nothing to resolve"
            );
        }
    }

    #[test]
    fn a_denied_symlink_is_refused_as_the_link_and_as_its_target() {
        // Canonicalizing a denied symlink would deny its target and leave the link itself
        // open — the one path everybody types.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("inside.bin"), b"x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let limits = Limits::new(dir.path().to_path_buf(), vec![link.clone()]);
        assert_eq!(
            limits.check(&link),
            Err(BlockReason::Denylisted),
            "the link itself"
        );
        assert_eq!(
            limits.check(&link.join("inside.bin")),
            Err(BlockReason::Denylisted),
            "what is reached through it"
        );
        assert_eq!(
            limits.check(&target),
            Err(BlockReason::Denylisted),
            "and the target it names"
        );
    }

    #[test]
    fn a_symlinked_scan_root_recognises_itself() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = dir.path().join("rootlink");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let file = real.join("a.bin");
        fs::write(&file, b"x").unwrap();
        let limits = Limits::new(link.clone(), vec![]);
        assert_eq!(
            limits.check(&link),
            Err(BlockReason::IsRoot),
            "the root as the caller spelled it"
        );
        assert_eq!(
            limits.check(&real),
            Err(BlockReason::IsRoot),
            "and as it resolves"
        );
        assert_eq!(
            limits.check(&link.join("a.bin")).unwrap().path,
            fs::canonicalize(&file).unwrap(),
            "what is inside it stays deletable"
        );
    }

    #[test]
    fn a_path_behind_an_unreadable_directory_is_not_called_gone() {
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        fs::create_dir_all(locked.join("inner")).unwrap();
        fs::write(locked.join("inner/a.bin"), b"x").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read_dir(&locked).is_ok() {
            // root, or a filesystem that does not enforce the mode: there is nothing
            // unreadable here to test against.
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
            eprintln!("skipped: the mode was not enforced");
            return;
        }
        let limits = Limits::new(dir.path().to_path_buf(), vec![]);
        // Collected before the permissions go back, so a failure still leaves a tempdir
        // that can be cleaned up.
        let behind_the_parent = limits.check(&locked.join("inner/a.bin"));
        let in_the_locked_dir = limits.check(&locked.join("a.bin"));
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            behind_the_parent,
            Err(BlockReason::Unreadable),
            "the parent cannot be resolved"
        );
        assert_eq!(
            in_the_locked_dir,
            Err(BlockReason::Unreadable),
            "the entry cannot be looked at"
        );
    }

    #[test]
    fn a_scan_root_the_user_pointed_at_a_denied_folder_unlocks_it() {
        // Pinned as a decision, not left as an accident: the denylist is there to keep a
        // scan of the home folder from wandering into ~/Library, and a root the user named
        // is a place they meant to go. `Limits::new` documents the same thing.
        //
        // What it unlocks is what the *home* entry was covering, and no more. The names of
        // `HOME_LIBRARY_DENIED` are not a navigation guard that a root can answer — they are
        // there because losing one is not recoverable — so they stay denied wherever the
        // root is pointed. Only an entry that contains the root is dropped, and none of them
        // ever contains it.
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("Library");
        let caches = library.join("Caches");
        let keychains = library.join("Keychains");
        fs::create_dir_all(&caches).unwrap();
        fs::create_dir_all(&keychains).unwrap();
        let limits = Limits::with_home(library.clone(), Some(dir.path().to_path_buf()));
        assert_eq!(
            limits.check(&caches).unwrap().path,
            fs::canonicalize(&caches).unwrap()
        );
        assert_eq!(
            limits.check(&library),
            Err(BlockReason::IsRoot),
            "the root itself is still refused"
        );
        assert_eq!(
            limits.check(&keychains),
            Err(BlockReason::Denylisted),
            "and naming the root is not a way to reach what is denied outright"
        );
    }

    #[test]
    fn a_denied_folder_that_appears_later_is_still_denied() {
        // The denied path does not exist when the limits are built, but its parent does,
        // which is enough to record the spelling a checked path will resolve to.
        let dir = tempfile::tempdir().unwrap();
        let limits = Limits::new(dir.path().to_path_buf(), vec![dir.path().join("Caches")]);
        let late = dir.path().join("Caches/app");
        fs::create_dir_all(&late).unwrap();
        assert_eq!(limits.check(&late), Err(BlockReason::Denylisted));
    }

    #[test]
    fn a_denied_path_nothing_can_resolve_stays_powerless() {
        // The residual limitation of keeping an unresolvable path as given: here the
        // denied path runs through a symlink that was not there to follow, so the checked
        // path resolves to a spelling the denylist never learned. The standard denylist
        // does not lean on this — every entry of it exists.
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let limits = Limits::new(
            dir.path().to_path_buf(),
            vec![dir.path().join("link/Caches")],
        );
        std::os::unix::fs::symlink(&real, dir.path().join("link")).unwrap();
        let late = real.join("Caches/app");
        fs::create_dir_all(&late).unwrap();
        assert_eq!(
            limits
                .check(&dir.path().join("link/Caches/app"))
                .unwrap()
                .path,
            fs::canonicalize(&late).unwrap()
        );
    }

    #[test]
    fn a_bare_relative_path_is_refused() {
        // `./Cargo.toml` has a parent to resolve; `Cargo.toml` has an empty one, which
        // names nothing, so it is refused instead of being resolved against whatever the
        // working directory happens to be. Unreachable from a scan, and safe.
        let limits = Limits::new(std::env::current_dir().unwrap(), vec![]);
        assert_eq!(
            limits.check(Path::new("Cargo.toml")),
            Err(BlockReason::Missing)
        );
    }
}
