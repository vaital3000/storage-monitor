//! Volume usage through `statvfs`.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskUsage {
    pub path: PathBuf,
    pub total: u64,
    /// Bytes free for this user (`f_bavail`).
    pub available: u64,
    /// Bytes free overall (`f_bfree`).
    pub free: u64,
    pub used: u64,
}

pub fn disk_usage(path: &Path) -> io::Result<DiskUsage> {
    let stat = nix::sys::statvfs::statvfs(path).map_err(io::Error::from)?;
    let frag = stat.fragment_size() as u64;
    let total = stat.blocks() as u64 * frag;
    let free = stat.blocks_free() as u64 * frag;
    let available = stat.blocks_available() as u64 * frag;
    Ok(DiskUsage {
        path: path.to_path_buf(),
        total,
        available,
        free,
        used: total.saturating_sub(free),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_of_the_temp_dir_is_consistent() {
        let usage = disk_usage(std::env::temp_dir().as_path()).unwrap();
        assert!(usage.total > 0);
        assert!(usage.available <= usage.total);
        assert_eq!(usage.used + usage.free, usage.total);
    }

    #[test]
    fn usage_of_a_missing_path_is_an_error() {
        assert!(disk_usage(std::path::Path::new("/definitely/missing")).is_err());
    }
}
