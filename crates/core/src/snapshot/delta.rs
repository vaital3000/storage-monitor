use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::model::Snapshot;
use crate::scan::NodeKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delta {
    pub path: String,
    pub kind: NodeKind,
    pub before: u64,
    pub after: u64,
    pub delta: i64,
}

/// One entry per path present in either snapshot; absent paths count as 0.
pub fn deltas(previous: &Snapshot, current: &Snapshot) -> Vec<Delta> {
    let mut before: HashMap<&str, (NodeKind, u64)> = previous
        .entries
        .iter()
        .map(|e| (e.path.as_str(), (e.kind, e.size)))
        .collect();
    let mut out = Vec::with_capacity(current.entries.len());
    for entry in &current.entries {
        let b = before
            .remove(entry.path.as_str())
            .map(|(_, s)| s)
            .unwrap_or(0);
        out.push(Delta {
            path: entry.path.clone(),
            kind: entry.kind,
            before: b,
            after: entry.size,
            delta: entry.size as i64 - b as i64,
        });
    }
    for (path, (kind, size)) in before {
        out.push(Delta {
            path: path.to_owned(),
            kind,
            before: size,
            after: 0,
            delta: -(size as i64),
        });
    }
    out
}

/// Growing directories, largest first, skipping a directory when one child explains
/// at least 80% of its growth (the child is listed instead).
pub fn top_growers(deltas: &[Delta], limit: usize) -> Vec<Delta> {
    let mut max_child_delta: HashMap<&str, i64> = HashMap::new();
    for d in deltas
        .iter()
        .filter(|d| d.kind == NodeKind::Dir && d.delta > 0)
    {
        if let Some(parent) = Path::new(&d.path).parent().and_then(|p| p.to_str()) {
            let slot = max_child_delta.entry(parent).or_insert(0);
            *slot = (*slot).max(d.delta);
        }
    }
    let mut growers: Vec<Delta> = deltas
        .iter()
        .filter(|d| d.kind == NodeKind::Dir && d.delta > 0)
        .filter(|d| {
            let biggest_child = max_child_delta.get(d.path.as_str()).copied().unwrap_or(0);
            (biggest_child as f64) < 0.8 * d.delta as f64
        })
        .cloned()
        .collect();
    growers.sort_by(|a, b| b.delta.cmp(&a.delta).then_with(|| a.path.cmp(&b.path)));
    growers.truncate(limit);
    growers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::NodeKind;
    use crate::snapshot::model::{SNAPSHOT_FORMAT, Snapshot, SnapshotEntry};

    fn snap(entries: &[(&str, NodeKind, u64)]) -> Snapshot {
        Snapshot {
            format: SNAPSHOT_FORMAT,
            taken_at: chrono::Utc::now(),
            root: "/h".into(),
            total_bytes: 0,
            file_count: 0,
            file_threshold: 0,
            entries: entries
                .iter()
                .map(|(p, k, s)| SnapshotEntry {
                    path: (*p).into(),
                    kind: *k,
                    size: *s,
                    file_count: 0,
                    mtime: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn deltas_cover_added_removed_and_changed_paths() {
        let before = snap(&[
            ("/h", NodeKind::Dir, 100),
            ("/h/a", NodeKind::Dir, 60),
            ("/h/gone", NodeKind::Dir, 40),
        ]);
        let after = snap(&[
            ("/h", NodeKind::Dir, 150),
            ("/h/a", NodeKind::Dir, 110),
            ("/h/new", NodeKind::Dir, 40),
        ]);
        let d = deltas(&before, &after);
        let find = |p: &str| d.iter().find(|x| x.path == p).unwrap();
        assert_eq!(find("/h").delta, 50);
        assert_eq!(find("/h/a").delta, 50);
        assert_eq!(find("/h/gone").delta, -40);
        assert_eq!(find("/h/new").delta, 40);
        assert_eq!(find("/h/new").before, 0);
    }

    #[test]
    fn top_growers_prefer_the_child_that_explains_the_growth() {
        let before = snap(&[
            ("/h", NodeKind::Dir, 100),
            ("/h/lib", NodeKind::Dir, 50),
            ("/h/lib/xcode", NodeKind::Dir, 40),
            ("/h/docs", NodeKind::Dir, 50),
        ]);
        let after = snap(&[
            ("/h", NodeKind::Dir, 200),
            ("/h/lib", NodeKind::Dir, 140),
            ("/h/lib/xcode", NodeKind::Dir, 125),
            ("/h/docs", NodeKind::Dir, 60),
        ]);
        let top = top_growers(&deltas(&before, &after), 10);
        let paths: Vec<&str> = top.iter().map(|d| d.path.as_str()).collect();
        // /h (+100) is explained by /h/lib (+90 >= 80%); /h/lib (+90) by /h/lib/xcode (+85); /h/docs (+10) stands alone
        assert_eq!(paths, vec!["/h/lib/xcode", "/h/docs"]);
    }

    #[test]
    fn top_growers_ignores_shrinking_and_files_and_respects_limit() {
        let before = snap(&[
            ("/h", NodeKind::Dir, 100),
            ("/h/a", NodeKind::Dir, 10),
            ("/h/b", NodeKind::Dir, 10),
            ("/h/f.iso", NodeKind::File, 80),
        ]);
        let after = snap(&[
            ("/h", NodeKind::Dir, 90),
            ("/h/a", NodeKind::Dir, 30),
            ("/h/b", NodeKind::Dir, 20),
            ("/h/f.iso", NodeKind::File, 40),
        ]);
        let top = top_growers(&deltas(&before, &after), 1);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].path, "/h/a");
    }
}
