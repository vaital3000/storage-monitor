//! The one door to the outside world: filesystem operations that delete things, and the
//! clock. Everything the action engine touches goes through this trait, so tests run
//! against a temporary filesystem on any platform. Process execution joins it in phase 2b.

mod real;
#[cfg(any(test, feature = "testing"))]
mod test;

use std::fs::Metadata;
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};

pub use real::RealSystem;
#[cfg(any(test, feature = "testing"))]
pub use test::TestSystem;

#[derive(Debug, thiserror::Error)]
pub enum SystemError {
    #[error("{} no longer exists", .0.display())]
    Missing(PathBuf),
    #[error("cannot read {}: {source}", path.display())]
    Metadata {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot move {} to the Trash: {message}", path.display())]
    Trash { path: PathBuf, message: String },
    #[error("cannot delete {}: {source}", path.display())]
    Remove {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The path is not one the port accepts; nothing was touched.
    #[error("refusing {}: {reason}", path.display())]
    Rejected { path: PathBuf, reason: &'static str },
}

/// Deleting, reading an entry without following it, and the clock.
///
/// Every path must be **absolute and in normal form**, the way a scan produces them: no
/// `..`, and byte for byte what `Path::components` rebuilds, so no trailing separator, no
/// `.` and no repeated separator. Anything else is [`SystemError::Rejected`] and nothing
/// is touched. The rule is not pedantry — the kernel reads the path the caller wrote, and
/// both endings name something other than the entry they seem to: `remove("/a/b/c/..")`
/// deletes the contents of `/a/b`, and `remove("/a/link/")` deletes what `link` points at.
pub trait System: Send + Sync {
    /// Metadata that does not follow symlinks.
    fn symlink_metadata(&self, path: &Path) -> Result<Metadata, SystemError>;
    /// Moves the entry to the Trash. A symlink is moved as a link.
    fn move_to_trash(&self, path: &Path) -> Result<(), SystemError>;
    /// Deletes a file, a symlink or a whole directory tree, permanently.
    ///
    /// Not atomic: a tree can be deleted in part and then fail, so a caller that cares
    /// about what survived has to look, not assume.
    fn remove(&self, path: &Path) -> Result<(), SystemError>;
    /// The current time. In the port because the action log is timestamped and tests
    /// compare those timestamps.
    fn now(&self) -> DateTime<Utc>;
}

/// Enforces the invariant documented on [`System`].
fn check_path(path: &Path) -> Result<(), SystemError> {
    let reject = |reason| {
        Err(SystemError::Rejected {
            path: path.to_path_buf(),
            reason,
        })
    };
    if !path.is_absolute() {
        return reject("the path is not absolute");
    }
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return reject("the path contains `..`");
    }
    // Byte comparison, not `==`: `Path` compares component-wise and calls `/a/b/` and
    // `/a/b` equal, which is exactly the difference that matters. `components()` drops a
    // trailing separator, a trailing `.` and a repeated one, but the raw string is what
    // reaches the syscall, and POSIX resolves a last component written as a directory by
    // following it — so `remove("/a/link/")` deletes what the link points at.
    let normal: PathBuf = path.components().collect();
    if normal.as_os_str() != path.as_os_str() {
        return reject("the path is not in normal form");
    }
    Ok(())
}
