use std::ffi::OsStr;
use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use tempfile::TempDir;

use super::{RealSystem, System, SystemError};

/// A [`System`] confined to a temporary directory: the real filesystem for metadata and
/// deletion, a Trash that is just another folder, and a clock that moves only when a test
/// moves it. Everything it touches disappears when it is dropped.
pub struct TestSystem {
    // Owns the temporary tree; dropping it removes `root` and `trash`.
    _dir: TempDir,
    root: PathBuf,
    trash: PathBuf,
    clock: Mutex<DateTime<Utc>>,
}

impl TestSystem {
    /// Creates `root/` and `trash/` in a fresh temporary directory.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("create the temporary directory");
        let root = dir.path().join("root");
        let trash = dir.path().join("trash");
        fs::create_dir(&root).expect("create the root directory");
        fs::create_dir(&trash).expect("create the trash directory");
        Self {
            _dir: dir,
            root,
            trash,
            clock: Mutex::new(
                Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                    .single()
                    .expect("a valid start instant"),
            ),
        }
    }

    /// The directory a test puts its files in.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where [`System::move_to_trash`] moves them.
    pub fn trash_dir(&self) -> &Path {
        &self.trash
    }

    /// Moves the clock forward.
    pub fn advance(&self, delta: TimeDelta) {
        *self.clock.lock().expect("an unpoisoned clock") += delta;
    }

    /// The name the entry gets in the Trash: the macOS Trash keeps both entries when a
    /// name repeats, and so does this one.
    fn trash_target(&self, name: &OsStr) -> PathBuf {
        let mut target = self.trash.join(name);
        let mut suffix = 2;
        // Not `exists`, which follows symlinks: a dangling link in the Trash is still a
        // name that is taken.
        while target.symlink_metadata().is_ok() {
            let mut taken = name.to_os_string();
            taken.push(format!(" {suffix}"));
            target = self.trash.join(taken);
            suffix += 1;
        }
        target
    }
}

impl Default for TestSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl System for TestSystem {
    fn symlink_metadata(&self, path: &Path) -> Result<Metadata, SystemError> {
        RealSystem.symlink_metadata(path)
    }

    fn move_to_trash(&self, path: &Path) -> Result<(), SystemError> {
        let name = path.file_name().ok_or_else(|| SystemError::Trash {
            path: path.to_path_buf(),
            message: "the path has no file name".to_owned(),
        })?;
        // `rename` moves a symlink as a link, the way the macOS Trash does.
        fs::rename(path, self.trash_target(name)).map_err(|source| SystemError::Trash {
            path: path.to_path_buf(),
            message: source.to_string(),
        })
    }

    fn remove(&self, path: &Path) -> Result<(), SystemError> {
        RealSystem.remove(path)
    }

    fn now(&self) -> DateTime<Utc> {
        *self.clock.lock().expect("an unpoisoned clock")
    }
}

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
        let names = fs::read_dir(sys.trash_dir()).unwrap().count();
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
        assert!(
            fs::symlink_metadata(sys.trash_dir().join("link"))
                .unwrap()
                .is_symlink()
        );
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
