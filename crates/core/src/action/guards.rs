//! The safety rules: which paths may be deleted at all.
//!
//! Pure functions over paths. Nothing here reads metadata or touches an entry; the only
//! syscall is the `canonicalize` that normalizes a path before it is judged. Everything a
//! deletion goes through passes [`check`] first, and what it returns — never what the
//! caller wrote — is what the `System` port is handed.

use std::path::{Path, PathBuf};

use super::BlockReason;

/// Where deletion is allowed and where it is never allowed.
#[derive(Debug, Clone)]
pub struct Limits {
    root: PathBuf,
    denied: Vec<PathBuf>,
}

impl Limits {
    /// `root` and `denied` are canonicalized here; paths that do not exist are kept as
    /// given, since a denylist entry may be absent on this machine — such an entry then
    /// only matches paths spelled the same way it is.
    ///
    /// A denied entry that contains the root is dropped. It would otherwise block every
    /// entry of every batch (`/` is the ancestor of everything, and the home folder is the
    /// usual scan root), and it protects nothing that rule 4 of [`check`] does not already
    /// refuse as [`BlockReason::IsRoot`]. Do not "restore" it.
    pub fn new(root: PathBuf, denied: Vec<PathBuf>) -> Self {
        let root = canonical(root);
        let denied = denied
            .into_iter()
            .map(canonical)
            .filter(|d| !root.starts_with(d))
            .collect();
        Self { root, denied }
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
        ];
        if let Some(home) = home {
            denied.push(home.join("Library"));
            denied.push(home);
        }
        Self::new(root, denied)
    }
}

/// Normalizes `path` and applies every rule. The returned path is what the engine deletes.
///
/// The rules are numbered as in the design; `Limits::new` refers to rule 4.
pub fn check(path: &Path, limits: &Limits) -> Result<PathBuf, BlockReason> {
    // 1. No last component to speak of: this is `/`, or a path ending in `..`. Both name
    //    something other than they appear to — `remove("/a/b/..")` empties `/a`. (A path
    //    with a file name always has a parent, so the two halves fail together.)
    let (Some(name), Some(parent)) = (path.file_name(), path.parent()) else {
        return Err(BlockReason::Denylisted);
    };
    // 2. Only the parent is resolved: a gone parent is a gone entry.
    let parent = parent.canonicalize().map_err(|_| BlockReason::Missing)?;
    // 3. Canonicalizing the whole path would follow a symlink to its target and delete that
    //    instead; resolving the parent alone is what stops a symlinked directory in the
    //    middle of the path from smuggling the entry elsewhere. It also makes the result
    //    absolute and in normal form, which is what the port demands, even when the scan
    //    root was relative.
    let normalized = parent.join(name);
    // 4. The root itself and every ancestor of it (`starts_with` includes equality).
    if limits.root.starts_with(&normalized) {
        return Err(BlockReason::IsRoot);
    }
    // 5. Component-wise, so `/h/ab` is not inside `/h/a`.
    if !normalized.starts_with(&limits.root) {
        return Err(BlockReason::OutsideRoots);
    }
    // 6. The denied entry itself (equality again) and everything below it.
    if limits.denied.iter().any(|d| normalized.starts_with(d)) {
        return Err(BlockReason::Denylisted);
    }
    Ok(normalized)
}

/// Per entry, in input order: `false` when another entry of the batch contains it.
///
/// Selecting a directory and something inside it costs one click in a tree view. Without
/// this the descendant's bytes would be counted twice and its deletion would fail with "no
/// such file" once the ancestor is gone. Of several copies of one path the first survives.
///
/// Quadratic, which for a hand-made selection is the cheapest thing that is obviously
/// correct.
pub fn drop_nested(paths: &[PathBuf]) -> Vec<bool> {
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

/// The canonical form when there is one. A path that does not exist is kept as given.
fn canonical(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn limits(root: &Path) -> Limits {
        Limits::new(root.to_path_buf(), vec![root.join("Library")])
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
        assert_eq!(
            check(&file, &limits(dir.path())).unwrap(),
            fs::canonicalize(&file).unwrap()
        );
    }

    #[test]
    fn the_scan_root_itself_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            check(dir.path(), &limits(dir.path())),
            Err(BlockReason::IsRoot)
        );
    }

    #[test]
    fn an_ancestor_of_the_root_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("home/user");
        fs::create_dir_all(&root).unwrap();
        assert_eq!(
            check(&dir.path().join("home"), &limits(&root)),
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
            check(&outside, &limits(&root)),
            Err(BlockReason::OutsideRoots)
        );
    }

    #[test]
    fn a_denylisted_subtree_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let inside = dir.path().join("Library/Caches/app");
        fs::create_dir_all(&inside).unwrap();
        assert_eq!(
            check(&inside, &limits(dir.path())),
            Err(BlockReason::Denylisted)
        );
    }

    #[test]
    fn a_missing_path_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            check(&dir.path().join("gone/a.bin"), &limits(dir.path())),
            Err(BlockReason::Missing)
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
        assert_eq!(
            check(&link, &limits(&root)).unwrap(),
            fs::canonicalize(&root).unwrap().join("link")
        );
    }

    #[test]
    fn dot_dot_cannot_climb_out_of_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("home");
        fs::create_dir_all(root.join("sub")).unwrap();
        let sneaky = root.join("sub/../../outside");
        fs::create_dir(dir.path().join("outside")).unwrap();
        assert_eq!(
            check(&sneaky, &limits(&root)),
            Err(BlockReason::OutsideRoots)
        );
    }

    #[test]
    fn the_filesystem_root_is_refused() {
        // The port accepts "/": it is a well-formed absolute path and the port is not a
        // policy layer. These two rules are the only thing between a batch and
        // `remove_dir_all("/")`, so they get their own test.
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            check(Path::new("/"), &limits(dir.path())),
            Err(BlockReason::Denylisted)
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
        // rule 1 is what catches it.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        assert_eq!(
            check(&dir.path().join("sub/.."), &limits(dir.path())),
            Err(BlockReason::Denylisted)
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
            check(&root.join("link/victim.bin"), &limits(&root)),
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
        let checked = check(&with_slash, &limits(&root)).unwrap();
        assert_eq!(checked, fs::canonicalize(&root).unwrap().join("link"));
        assert_port_normal_form(&checked);
    }

    #[test]
    fn dots_inside_the_path_are_normalized_away() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("home");
        fs::create_dir_all(root.join("sub")).unwrap();
        let file = root.join("sub/a.bin");
        fs::write(&file, b"x").unwrap();
        let noisy = root.join("./sub/../sub/./a.bin");
        let checked = check(&noisy, &limits(&root)).unwrap();
        assert_eq!(checked, fs::canonicalize(&file).unwrap());
        assert_port_normal_form(&checked);
    }

    #[test]
    fn a_relative_path_comes_back_absolute() {
        // `Tree::path` keeps whatever the scan root was, and only the CLI normalizes it,
        // so a relative path can reach the guard; the port refuses one.
        let root = std::env::current_dir().unwrap(); // the crate directory under `cargo test`
        let file = root.join("Cargo.toml");
        let checked = check(Path::new("./Cargo.toml"), &Limits::new(root, vec![])).unwrap();
        assert_eq!(checked, fs::canonicalize(&file).unwrap());
        assert_port_normal_form(&checked);
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
            check(&file, &limits).unwrap(),
            fs::canonicalize(&file).unwrap()
        );
        assert_eq!(check(&root, &limits), Err(BlockReason::IsRoot));
        assert_eq!(check(dir.path(), &limits), Err(BlockReason::IsRoot));
    }

    #[test]
    fn the_standard_denylist_leaves_an_ordinary_scan_root_usable() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.bin");
        fs::write(&file, b"x").unwrap();
        let limits = Limits::for_scan_root(dir.path().to_path_buf());
        assert_eq!(
            check(&file, &limits).unwrap(),
            fs::canonicalize(&file).unwrap()
        );
    }

    #[test]
    fn the_standard_denylist_protects_the_library_folder_of_the_home_it_is_given() {
        // `for_scan_root` reads the real home folder, so the rule is exercised through the
        // seam it delegates to, with a temp directory standing in for the home.
        let dir = tempfile::tempdir().unwrap();
        // Created before the limits are built: an absent denied path is kept as given and
        // would then not match the canonical form of the checked path.
        let library = dir.path().join("Library/Caches");
        fs::create_dir_all(&library).unwrap();
        let keep = dir.path().join("Downloads/a.bin");
        fs::create_dir_all(keep.parent().unwrap()).unwrap();
        fs::write(&keep, b"x").unwrap();
        let limits = Limits::with_home(dir.path().to_path_buf(), Some(dir.path().to_path_buf()));
        assert_eq!(
            check(&library, &limits),
            Err(BlockReason::Denylisted),
            "~/Library is denied"
        );
        assert_eq!(
            check(&library.join("app"), &limits),
            Err(BlockReason::Denylisted),
            "and everything under it"
        );
        assert_eq!(
            check(&keep, &limits).unwrap(),
            fs::canonicalize(&keep).unwrap(),
            "the home folder in the denylist must not block the scan of the home folder"
        );
    }

    #[test]
    fn the_standard_denylist_protects_the_system_folders() {
        // The one scan root from which a denied folder is reachable at all: anywhere else
        // `/usr` is refused as `OutsideRoots` before the denylist is consulted.
        let limits = Limits::for_scan_root(PathBuf::from("/"));
        assert_eq!(
            check(Path::new("/usr"), &limits),
            Err(BlockReason::Denylisted)
        );
        assert_eq!(
            check(Path::new("/usr/lib"), &limits),
            Err(BlockReason::Denylisted)
        );
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
            check(&file, &limits).unwrap(),
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
            check(&sibling, &limits(&root)),
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
            check(&file, &limits(dir.path())).unwrap(),
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
}
