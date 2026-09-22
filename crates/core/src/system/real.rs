use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use trash::TrashContext;
#[cfg(target_os = "macos")]
use trash::macos::{DeleteMethod, TrashContextExtMacos};

use super::process::{self, KNOWN_DIRS, OUTPUT_CAP};
use super::{Invocation, Output, System, SystemError, check_path};

/// The real machine.
#[derive(Debug, Clone, Default)]
pub struct RealSystem;

impl System for RealSystem {
    fn symlink_metadata(&self, path: &Path) -> Result<Metadata, SystemError> {
        check_path(path)?;
        fs::symlink_metadata(path).map_err(|source| match source.kind() {
            std::io::ErrorKind::NotFound => SystemError::Missing(path.to_path_buf()),
            _ => SystemError::Metadata {
                path: path.to_path_buf(),
                source,
            },
        })
    }

    fn move_to_trash(&self, path: &Path) -> Result<(), SystemError> {
        check_path(path)?;
        // An entry that vanished between the preview and now reads as `Missing`: the
        // Trash error for it is an opaque Cocoa message.
        self.symlink_metadata(path)?;
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
            message: trash_message(err),
        })
    }

    fn remove(&self, path: &Path) -> Result<(), SystemError> {
        check_path(path)?;
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

    fn locate(&self, tool: &str) -> Option<PathBuf> {
        process::locate_in(tool, std::env::var_os("PATH").as_deref(), &KNOWN_DIRS)
    }

    fn run(&self, invocation: &Invocation) -> Result<Output, SystemError> {
        // The program and the working directory, by the rule every other path of the port
        // keeps: a relative program would be looked up against the working directory, and a
        // relative directory would resolve against whatever the app's happens to be.
        check_path(&invocation.program)?;
        if let Some(cwd) = &invocation.cwd {
            check_path(cwd)?;
        }
        process::run_capped(invocation, OUTPUT_CAP)
    }
}

/// What to show the user. The crate's own `Display` is its `Debug` dump, and the message
/// ends up in the action log and on the Activity screen.
fn trash_message(err: trash::Error) -> String {
    match err {
        trash::Error::Unknown { description } => description,
        other => other.to_string(),
    }
}
