//! The sandbox the demo module cleans, written on its first discovery (phase 2b design,
//! section 9).
//!
//! Every write here is idempotent — the same names, the same bytes, the same times — and
//! the marker goes last. A seeding that stopped halfway leaves no marker, so the next
//! discovery seeds again and completes it; a finished one is never repeated, which is what
//! keeps a cleaned item cleaned. Deleting the whole sandbox is how a user asks for it back.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, TimeDelta, Utc};

/// Written last. A sandbox without it is one whose seeding did not finish.
pub(crate) const SEEDED: &str = ".seeded";

/// A folder holding a file of this name is marked keep.
pub(crate) const KEEP_MARKER: &str = "KEEP";

/// A folder of the sandbox: its files, and how long ago all of it was last modified.
struct Folder {
    name: &'static str,
    files: &'static [(&'static str, u64)],
    age_days: i64,
    keep: bool,
}

/// A file of the sandbox that stands for an object a tool manages, the way Docker manages an
/// image: removed by a command rather than through the port.
struct Object {
    name: &'static str,
    bytes: u64,
    age_days: i64,
}

/// Chosen so that every part of the module contract the v1 modules need runs at least once:
/// a stale folder and a recent one, a folder marked keep — older than both, because the mark
/// has to win over the age — an old object and a fresh one. The design's table has the
/// reasons.
const FOLDERS: [Folder; 3] = [
    Folder {
        name: "build-cache",
        files: &[
            ("a.o", 500_000),
            ("b.o", 500_000),
            ("c.o", 500_000),
            ("d.o", 500_000),
        ],
        age_days: 45,
        keep: false,
    },
    Folder {
        name: "logs",
        files: &[("app.log", 800_000), ("app.1.log", 400_000)],
        age_days: 3,
        keep: false,
    },
    Folder {
        name: "keepsake",
        files: &[("notes.txt", 500_000)],
        age_days: 200,
        keep: true,
    },
];

const OBJECTS: [Object; 2] = [
    Object {
        name: "old.object",
        bytes: 1_000_000,
        age_days: 60,
    },
    Object {
        name: "fresh.object",
        bytes: 3_000_000,
        age_days: 1,
    },
];

/// Whether `sandbox` holds a finished seeding.
pub(crate) fn is_seeded(sandbox: &Path) -> bool {
    fs::symlink_metadata(sandbox.join(SEEDED)).is_ok()
}

/// Writes the sandbox, dated from `now`: the clock of the port, so that a test decides how
/// old everything is.
pub(crate) fn seed(sandbox: &Path, now: DateTime<Utc>) -> io::Result<()> {
    let ago = |days: i64| SystemTime::from(now - TimeDelta::days(days));
    fs::create_dir_all(sandbox)?;
    for folder in &FOLDERS {
        let dir = sandbox.join(folder.name);
        fs::create_dir_all(&dir)?;
        let time = ago(folder.age_days);
        for (name, bytes) in folder.files {
            write(&dir.join(name), *bytes, time)?;
        }
        if folder.keep {
            write(&dir.join(KEEP_MARKER), 64, time)?;
        }
        // Last, because creating a file inside a folder moves the folder's own time.
        File::open(&dir)?.set_modified(time)?;
    }
    for object in &OBJECTS {
        write(
            &sandbox.join(object.name),
            object.bytes,
            ago(object.age_days),
        )?;
    }
    write(&sandbox.join(SEEDED), 0, SystemTime::from(now))
}

/// `bytes` of real data, dated `time`. Real, because a sparse file allocates nothing and
/// would be worth nothing to clean.
fn write(path: &Path, bytes: u64, time: SystemTime) -> io::Result<()> {
    let mut file = File::create(path)?;
    let chunk = vec![0u8; 64 * 1024];
    let mut left = bytes;
    while left > 0 {
        let now = left.min(chunk.len() as u64);
        file.write_all(&chunk[..now as usize])?;
        left -= now;
    }
    file.set_modified(time)
}
