//! Volume usage through `statvfs`.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Space on the volume that contains a path.
///
/// `used = total - free` is the container-level figure: on APFS all volumes of a container
/// (System, VM, Preboot, the data volume) share one pool, so `used` includes them and can
/// exceed what a scan of the data volume finds. `available` is what the user can still
/// write; `free` also includes the blocks reserved for the superuser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskUsage {
    pub path: PathBuf,
    pub total: u64,
    /// Bytes free for this user (`f_bavail`).
    pub available: u64,
    /// Bytes free overall (`f_bfree`).
    pub free: u64,
    /// `total - free`.
    pub used: u64,
}

/// Usage of the volume that contains `path`.
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
        assert!(usage.free <= usage.total, "{usage:?}");
        assert!(usage.available <= usage.free, "{usage:?}");
        assert!(usage.used <= usage.total, "{usage:?}");
    }

    #[test]
    fn usage_of_a_missing_path_is_an_error() {
        assert!(disk_usage(std::path::Path::new("/definitely/missing")).is_err());
    }
}
