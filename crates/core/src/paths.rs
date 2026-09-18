//! Where the app keeps its data.

use std::path::{Path, PathBuf};

pub const DATA_DIR_ENV: &str = "STORAGE_MONITOR_DATA_DIR";

/// `$STORAGE_MONITOR_DATA_DIR` or `~/Library/Application Support/storage-monitor`.
pub fn data_dir() -> PathBuf {
    with_data_dir_override(std::env::var_os(DATA_DIR_ENV).map(PathBuf::from))
}

pub(crate) fn with_data_dir_override(override_dir: Option<impl Into<PathBuf>>) -> PathBuf {
    match override_dir {
        Some(dir) => dir.into(),
        None => dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("storage-monitor"),
    }
}

pub fn snapshots_dir() -> PathBuf {
    snapshots_dir_in(&data_dir())
}

pub(crate) fn snapshots_dir_in(data_dir: &Path) -> PathBuf {
    data_dir.join("snapshots")
}

pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_honors_the_environment_override() {
        let dir = with_data_dir_override(Some("/tmp/sm-test-data"));
        assert_eq!(dir, std::path::PathBuf::from("/tmp/sm-test-data"));
    }

    #[test]
    fn snapshots_dir_is_inside_the_data_dir() {
        let dir = with_data_dir_override(Some("/tmp/sm-test-data"));
        assert_eq!(
            snapshots_dir_in(&dir),
            std::path::PathBuf::from("/tmp/sm-test-data/snapshots")
        );
    }
}
