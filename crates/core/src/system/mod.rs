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
