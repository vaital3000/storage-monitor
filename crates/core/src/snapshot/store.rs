use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::model::Snapshot;

/// Metadata written next to each snapshot so listing never decodes payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMeta {
    #[serde(skip)]
    pub path: PathBuf,
    pub taken_at: DateTime<Utc>,
    pub root: PathBuf,
    pub total_bytes: u64,
    pub file_count: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Codec(#[from] super::model::CodecError),
    #[error("meta: {0}")]
    Meta(#[from] serde_json::Error),
}

/// A directory of `<stamp>.snap` (payload) and `<stamp>.json` (meta) pairs.
#[derive(Debug, Clone)]
pub struct SnapshotStore {
    dir: PathBuf,
}

impl SnapshotStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn save(&self, snapshot: &Snapshot) -> Result<PathBuf, StoreError> {
        fs::create_dir_all(&self.dir)?;
        let stamp = snapshot.taken_at.format("%Y%m%dT%H%M%S%.3fZ").to_string();
        let path = self.dir.join(format!("{stamp}.snap"));
        fs::write(&path, snapshot.encode()?)?;
        let meta = SnapshotMeta {
            path: path.clone(),
            taken_at: snapshot.taken_at,
            root: snapshot.root.clone(),
            total_bytes: snapshot.total_bytes,
            file_count: snapshot.file_count,
        };
        fs::write(
            path.with_extension("json"),
            serde_json::to_vec_pretty(&meta)?,
        )?;
        Ok(path)
    }

    /// Newest first.
    pub fn list(&self) -> Result<Vec<SnapshotMeta>, StoreError> {
        let mut metas = Vec::new();
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(metas),
            Err(err) => return Err(err.into()),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let snap_path = path.with_extension("snap");
            if !snap_path.exists() {
                continue;
            }
            let mut meta: SnapshotMeta = serde_json::from_slice(&fs::read(&path)?)?;
            meta.path = snap_path;
            metas.push(meta);
        }
        metas.sort_by(|a, b| b.taken_at.cmp(&a.taken_at));
        Ok(metas)
    }

    pub fn load(&self, path: &Path) -> Result<Snapshot, StoreError> {
        Ok(Snapshot::decode(&fs::read(path)?)?)
    }

    /// The newest snapshot of any root; use [`SnapshotStore::latest_for`] to compare
    /// scans of the same folder.
    pub fn latest(&self) -> Result<Option<Snapshot>, StoreError> {
        match self.list()?.first() {
            Some(meta) => Ok(Some(self.load(&meta.path)?)),
            None => Ok(None),
        }
    }

    /// The newest snapshot taken of `root` (compared as written, without canonicalization).
    pub fn latest_for(&self, root: &Path) -> Result<Option<Snapshot>, StoreError> {
        match self.list()?.into_iter().find(|meta| meta.root == root) {
            Some(meta) => Ok(Some(self.load(&meta.path)?)),
            None => Ok(None),
        }
    }

    /// Deletes everything but the `keep` newest snapshots, counted across all roots (frequent
    /// scans of one folder push out the snapshots of another); returns how many were removed.
    pub fn prune(&self, keep: usize) -> Result<usize, StoreError> {
        let list = self.list()?;
        let mut removed = 0;
        for meta in list.iter().skip(keep) {
            fs::remove_file(&meta.path)?;
            let _ = fs::remove_file(meta.path.with_extension("json"));
            removed += 1;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::NodeKind;
    use crate::snapshot::model::{Snapshot, SnapshotEntry};

    fn snapshot(taken_at: chrono::DateTime<chrono::Utc>, total: u64) -> Snapshot {
        snapshot_of("/home", taken_at, total)
    }

    fn snapshot_of(root: &str, taken_at: chrono::DateTime<chrono::Utc>, total: u64) -> Snapshot {
        Snapshot {
            format: super::super::model::SNAPSHOT_FORMAT,
            taken_at,
            root: root.into(),
            total_bytes: total,
            file_count: 1,
            file_threshold: 0,
            entries: vec![SnapshotEntry {
                path: root.into(),
                kind: NodeKind::Dir,
                size: total,
                file_count: 1,
                mtime: 0,
            }],
        }
    }

    #[test]
    fn save_list_load_and_prune() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().join("snapshots"));
        let t0 = chrono::Utc::now() - chrono::Duration::minutes(3);
        for i in 0..3u64 {
            store
                .save(&snapshot(t0 + chrono::Duration::minutes(i as i64), i))
                .unwrap();
        }
        let list = store.list().unwrap();
        assert_eq!(list.len(), 3);
        assert!(list[0].taken_at > list[1].taken_at, "newest first");
        assert_eq!(list[0].total_bytes, 2);
        let latest = store.latest().unwrap().unwrap();
        assert_eq!(latest.total_bytes, 2);
        assert_eq!(store.load(&list[2].path).unwrap().total_bytes, 0);
        assert_eq!(store.prune(2).unwrap(), 1);
        assert_eq!(store.list().unwrap().len(), 2);
        assert_eq!(store.list().unwrap()[1].total_bytes, 1);
    }

    #[test]
    fn empty_store_lists_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().join("missing"));
        assert!(store.list().unwrap().is_empty());
        assert!(store.latest().unwrap().is_none());
        assert!(store.latest_for(Path::new("/home")).unwrap().is_none());
    }

    #[test]
    fn latest_for_picks_the_newest_snapshot_of_that_root() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().join("snapshots"));
        let t0 = chrono::Utc::now() - chrono::Duration::minutes(3);
        store.save(&snapshot_of("/home", t0, 1)).unwrap();
        store
            .save(&snapshot_of("/work", t0 + chrono::Duration::minutes(1), 2))
            .unwrap();
        store
            .save(&snapshot_of("/home", t0 + chrono::Duration::minutes(2), 3))
            .unwrap();
        assert_eq!(store.latest().unwrap().unwrap().total_bytes, 3);
        let latest = |root: &str| store.latest_for(Path::new(root)).unwrap();
        assert_eq!(latest("/home").unwrap().total_bytes, 3);
        assert_eq!(latest("/work").unwrap().total_bytes, 2);
        assert!(latest("/elsewhere").is_none());
    }
}
