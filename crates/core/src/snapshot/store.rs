use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::model::{SNAPSHOT_FORMAT, Snapshot};

/// Metadata written next to each snapshot so listing never decodes payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMeta {
    #[serde(skip)]
    pub path: PathBuf,
    /// [`Snapshot::format`] of the payload.
    pub format: u32,
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

/// A directory of `<stamp>.snap` (payload) and `<stamp>.json` (meta) pairs. Files are
/// written to `*.tmp` and renamed into place, the sidecar last, so a snapshot is listed
/// only once it is complete; damaged files are ignored and removed by [`SnapshotStore::prune`].
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
        write_atomically(&path, &snapshot.encode()?)?;
        let meta = SnapshotMeta {
            path: path.clone(),
            format: snapshot.format,
            taken_at: snapshot.taken_at,
            root: snapshot.root.clone(),
            total_bytes: snapshot.total_bytes,
            file_count: snapshot.file_count,
        };
        write_atomically(
            &path.with_extension("json"),
            &serde_json::to_vec_pretty(&meta)?,
        )?;
        Ok(path)
    }

    /// Newest first. Snapshots whose sidecar is missing, damaged or of another format are
    /// left out.
    pub fn list(&self) -> Result<Vec<SnapshotMeta>, StoreError> {
        let mut metas = Vec::new();
        for path in self.paths()? {
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let snap_path = path.with_extension("snap");
            if !snap_path.exists() {
                continue;
            }
            let Some(mut meta) = read_meta(&path) else {
                continue;
            };
            if meta.format != SNAPSHOT_FORMAT {
                continue;
            }
            meta.path = snap_path;
            metas.push(meta);
        }
        metas.sort_by_key(|meta| std::cmp::Reverse(meta.taken_at));
        Ok(metas)
    }

    /// Every path in the store directory; none when the directory does not exist yet.
    fn paths(&self) -> io::Result<Vec<PathBuf>> {
        match fs::read_dir(&self.dir) {
            Ok(entries) => entries.map(|e| e.map(|e| e.path())).collect(),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(err) => Err(err),
        }
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
    /// scans of one folder push out the snapshots of another), and cleans up damage: payloads
    /// without a sidecar, sidecars without a payload, unparsable sidecars (with their payload)
    /// and leftover `*.tmp` files. Snapshots of another format are left alone. Returns how
    /// many snapshots and stray files were removed.
    pub fn prune(&self, keep: usize) -> Result<usize, StoreError> {
        let mut removed = 0;
        for meta in self.list()?.iter().skip(keep) {
            remove_if_present(&meta.path)?;
            remove_if_present(&meta.path.with_extension("json"))?;
            removed += 1;
        }
        let paths = self.paths()?;
        let present = |path: &Path| paths.iter().any(|p| p == path);
        for path in &paths {
            let stray = match path.extension().and_then(|e| e.to_str()) {
                Some("tmp") => true,
                Some("snap") => !present(&path.with_extension("json")),
                Some("json") if read_meta(path).is_none() => {
                    // A damaged sidecar takes its payload with it.
                    remove_if_present(&path.with_extension("snap"))?;
                    true
                }
                Some("json") => !present(&path.with_extension("snap")),
                _ => false,
            };
            if stray {
                remove_if_present(path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// `None` when the sidecar cannot be read or parsed.
fn read_meta(path: &Path) -> Option<SnapshotMeta> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// Writes `<path>.tmp` and renames it into place, so readers never see a partial file.
fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::NodeKind;
    use crate::snapshot::model::{SNAPSHOT_FORMAT, Snapshot, SnapshotEntry};

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn snapshot(taken_at: chrono::DateTime<chrono::Utc>, total: u64) -> Snapshot {
        snapshot_of("/home", taken_at, total)
    }

    fn snapshot_of(root: &str, taken_at: chrono::DateTime<chrono::Utc>, total: u64) -> Snapshot {
        Snapshot {
            format: SNAPSHOT_FORMAT,
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
    fn damaged_files_are_ignored_by_list_and_removed_by_prune() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().join("snapshots"));
        let t0 = chrono::Utc::now() - chrono::Duration::minutes(3);
        let oldest = store.save(&snapshot(t0, 1)).unwrap();
        let newest = store
            .save(&snapshot(t0 + chrono::Duration::minutes(1), 2))
            .unwrap();
        let mut expected: Vec<String> = [&oldest, &newest]
            .into_iter()
            .flat_map(|p| [p.to_path_buf(), p.with_extension("json")])
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        expected.sort();
        assert_eq!(
            file_names(store.dir()),
            expected,
            "save leaves no temporary files"
        );

        let sdir = store.dir();
        fs::write(
            sdir.join("truncated.json"),
            br#"{"format": 1, "takenAt": "2026-"#,
        )
        .unwrap();
        fs::write(sdir.join("truncated.snap"), b"payload").unwrap();
        fs::write(sdir.join("orphan.snap"), b"payload").unwrap();
        fs::copy(newest.with_extension("json"), sdir.join("lonely.json")).unwrap();
        fs::write(sdir.join("stale.snap.tmp"), b"half").unwrap();
        fs::write(sdir.join("stale.json.tmp"), b"half").unwrap();

        let list = store.list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].format, SNAPSHOT_FORMAT);
        assert_eq!(store.latest().unwrap().unwrap().total_bytes, 2);

        assert_eq!(
            store.prune(10).unwrap(),
            5,
            "truncated pair, orphan payload, lonely sidecar, two temporary files"
        );
        assert_eq!(file_names(store.dir()), expected);
        assert_eq!(store.list().unwrap().len(), 2);
    }

    #[test]
    fn sidecar_of_another_format_is_ignored_but_kept() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().join("snapshots"));
        let good = store.save(&snapshot(chrono::Utc::now(), 1)).unwrap();
        let sdir = store.dir();
        let mut meta: serde_json::Value =
            serde_json::from_slice(&fs::read(good.with_extension("json")).unwrap()).unwrap();
        meta["format"] = 99.into();
        meta["totalBytes"] = 99.into();
        fs::write(sdir.join("future.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
        fs::copy(&good, sdir.join("future.snap")).unwrap();

        assert_eq!(store.list().unwrap().len(), 1);
        assert_eq!(store.latest().unwrap().unwrap().total_bytes, 1);
        assert_eq!(store.prune(10).unwrap(), 0, "another format is not damage");
        assert!(sdir.join("future.snap").exists());
        assert!(sdir.join("future.json").exists());
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
