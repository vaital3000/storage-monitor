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
/// Every path must be **absolute and free of `.` and `..` components**, the way a scan
/// produces them; anything else is [`SystemError::Rejected`] and nothing is touched. The
/// rule is not pedantry: the kernel resolves a trailing `..` before the syscall sees it,
/// so `remove("/a/b/c/..")` would delete the contents of `/a/b` — siblings the caller
/// never named — and report success.
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
        .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return reject("the path contains `.` or `..`");
    }
    Ok(())
}
