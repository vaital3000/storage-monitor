use std::fs::{self, Metadata};
use std::path::Path;

use chrono::{DateTime, Utc};
use trash::TrashContext;
#[cfg(target_os = "macos")]
use trash::macos::{DeleteMethod, TrashContextExtMacos};

use super::{System, SystemError};

/// The real machine.
#[derive(Debug, Clone, Default)]
pub struct RealSystem;

impl System for RealSystem {
    fn symlink_metadata(&self, path: &Path) -> Result<Metadata, SystemError> {
        fs::symlink_metadata(path).map_err(|source| match source.kind() {
            std::io::ErrorKind::NotFound => SystemError::Missing(path.to_path_buf()),
            _ => SystemError::Metadata {
                path: path.to_path_buf(),
                source,
            },
        })
    }

    fn move_to_trash(&self, path: &Path) -> Result<(), SystemError> {
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut ctx = TrashContext::default();
        // `Finder` is the crate's default and the only method that produces a reliable
        // "Put Back", but it drives Finder through `osascript` and needs an Automation
        // grant that an ad-hoc signed build loses on every rebuild. See the design,
        // section 11.
        #[cfg(target_os = "macos")]
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        ctx.delete(path).map_err(|err| SystemError::Trash {
            path: path.to_path_buf(),
            message: err.to_string(),
        })
    }

    fn remove(&self, path: &Path) -> Result<(), SystemError> {
        let meta = self.symlink_metadata(path)?;
        let result = if meta.is_dir() {
            // Not a symlink: `symlink_metadata` reports links as links, and std removes
            // the tree without following any link inside it.
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        result.map_err(|source| SystemError::Remove {
            path: path.to_path_buf(),
            source,
        })
    }

    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
