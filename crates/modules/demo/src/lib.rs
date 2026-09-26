//! The demo cleanup module (phase 2b design, section 9): sample files it seeds in the app's
//! own data folder, so that the cleanup flow can be tried end to end without touching
//! anything of the user's. The registry registers it in debug builds only.
//!
//! It is also the reference for what a real module does — reads with `std::fs` under a path
//! from the context, typed facts, reasons with codes, plans as plain functions — and
//! `docs/modules/README.md` points at it. The one thing it does that no real module may is
//! write: it creates its sandbox, and nothing else.

mod seed;

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use storage_monitor_core::action::Mode;
use storage_monitor_core::module::{
    ActionOption, ActionSpec, Availability, Command, Context, Descriptor, Effect, Fact, FactValue,
    Item, Level, Module, ModuleError, Reason, Size, Step, Target, Verdict,
};
use storage_monitor_core::scan::NodeKind;

use crate::seed::KEEP_MARKER;

/// The module's id, and the prefix of every item id.
pub const ID: &str = "demo";

/// The sandbox, under the data dir.
const SANDBOX: &str = "demo";

/// What the housekeeping step of a folder touches after the folder went to the Trash — the
/// demo's `git worktree prune`.
const LAST_CLEANUP: &str = ".last-cleanup";

/// An object is a file with this extension; any other file of the sandbox is not an item.
const OBJECT_EXTENSION: &str = "object";

/// Untouched for this long, an item is Safe; modified more recently, Review. A setting in
/// phase 2c.
const STALE_AFTER_DAYS: i64 = 30;

/// The tools the plans run.
const TOOLS: [&str; 2] = ["rm", "touch"];

/// The demo module.
pub struct Demo;

impl Module for Demo {
    fn descriptor(&self) -> Descriptor {
        Descriptor {
            id: ID,
            name: "Demo",
            description: "Sample files in the app's data folder, to try cleaning without \
                          touching anything of yours. Debug builds only.",
            tools: &TOOLS,
        }
    }

    fn availability(&self, ctx: &Context) -> Availability {
        match TOOLS.iter().find(|tool| ctx.sys.locate(tool).is_none()) {
            Some(tool) => Availability::Unavailable {
                reason: format!("`{tool}` is not installed"),
            },
            None => Availability::Available,
        }
    }

    fn discover(&self, ctx: &Context) -> Result<Vec<Item>, ModuleError> {
        let sandbox = ctx.data_dir.join(SANDBOX);
        if !seed::is_seeded(&sandbox) {
            seed::seed(&sandbox, ctx.sys.now()).map_err(|source| ModuleError::Io {
                path: sandbox.clone(),
                source,
            })?;
        }
        // The spelling the guards resolve to, because the command that removes an object is
        // handed this path and resolves it itself: the engine refuses a `Removes` target
        // spelled any other way (ADR 0008).
        let sandbox = sandbox.canonicalize().map_err(io_at(&sandbox))?;
        let now = ctx.sys.now();
        let mut items = Vec::new();
        for entry in fs::read_dir(&sandbox).map_err(io_at(&sandbox))? {
            let entry = entry.map_err(io_at(&sandbox))?;
            let path = sandbox.join(entry.file_name());
            let name = entry.file_name().to_string_lossy().into_owned();
            let meta = fs::symlink_metadata(&path).map_err(io_at(&path))?;
            if meta.is_dir() {
                items.push(folder(&path, &name, now)?);
            } else if meta.is_file() && path.extension().is_some_and(|ext| ext == OBJECT_EXTENSION)
            {
                items.push(object(&path, &name, &meta, now)?);
            }
            // Anything else — the seeding marker, the housekeeping file — is not an item.
        }
        items.sort_by(|a, b| {
            b.size
                .bytes
                .cmp(&a.size.bytes)
                .then_with(|| a.title.cmp(&b.title))
        });
        Ok(items)
    }

    fn plan(&self, item: &Item, action: &ActionSpec, _options: &[String], mode: Mode) -> Vec<Step> {
        // The force option changes nothing here: it only lifts the Keep verdict, which is the
        // core's rule, not the module's.
        let Some(path) = &item.path else {
            return Vec::new();
        };
        match (item.kind.as_str(), action.id.as_str()) {
            ("folder", "delete") => {
                let delete = Step::Delete(Target::new(path, NodeKind::Dir));
                match (mode, path.parent()) {
                    // Two steps against one, the way a worktree goes: the folder to the
                    // Trash, then the bookkeeping the permanent removal does by itself.
                    (Mode::Trash, Some(sandbox)) => vec![
                        delete,
                        Step::Run {
                            command: Command::new("touch").arg(sandbox.join(LAST_CLEANUP)),
                            effect: Effect::Housekeeping,
                        },
                    ],
                    _ => vec![delete],
                }
            }
            // The same command in both modes, the way a Docker image goes.
            ("object", "remove") => vec![Step::Run {
                command: Command::new("rm").arg(path),
                effect: Effect::Removes(Target::new(path, NodeKind::File)),
            }],
            _ => Vec::new(),
        }
    }
}

/// A folder of the sandbox, judged by its age and its mark.
fn folder(path: &Path, name: &str, now: DateTime<Utc>) -> Result<Item, ModuleError> {
    let usage = usage(path)?;
    let keep = fs::symlink_metadata(path.join(KEEP_MARKER)).is_ok();
    let options = if keep {
        vec![ActionOption {
            id: "force".to_owned(),
            label: "Delete it although it is marked keep".to_owned(),
            default: false,
            force: true,
        }]
    } else {
        Vec::new()
    };
    Ok(Item {
        id: format!("{ID}:{name}"),
        module: ID.to_owned(),
        kind: "folder".to_owned(),
        title: name.to_owned(),
        subtitle: Some(count(usage.files, "file")),
        path: Some(path.to_path_buf()),
        size: Size::exact(usage.bytes),
        last_used: Some(usage.newest),
        verdict: verdict((now - usage.newest).num_days(), keep),
        facts: vec![
            Fact::new("files", "Files", FactValue::Count(usage.files)),
            Fact::new("modified", "Modified", FactValue::Date(usage.newest)),
            Fact::new("keep", "Marked keep", FactValue::Flag(keep)),
        ],
        actions: vec![ActionSpec {
            id: "delete".to_owned(),
            label: "Delete folder".to_owned(),
            estimated_free: usage.bytes,
            options,
        }],
    })
}

/// A file standing for an object a tool manages. Its size is marked estimated, the way the
/// size of a Docker image that shares layers is.
fn object(
    path: &Path,
    name: &str,
    meta: &fs::Metadata,
    now: DateTime<Utc>,
) -> Result<Item, ModuleError> {
    let modified = modified(meta, path)?;
    let bytes = meta.blocks() * 512;
    Ok(Item {
        id: format!("{ID}:{name}"),
        module: ID.to_owned(),
        kind: "object".to_owned(),
        title: name.to_owned(),
        subtitle: Some("Stored object".to_owned()),
        path: Some(path.to_path_buf()),
        size: Size::estimated(bytes),
        last_used: Some(modified),
        verdict: verdict((now - modified).num_days(), false),
        facts: vec![
            Fact::new("modified", "Modified", FactValue::Date(modified)),
            Fact::new("logical", "Logical size", FactValue::Bytes(meta.len())),
            Fact::new(
                "format",
                "Format",
                FactValue::Text("Demo object".to_owned()),
            ),
        ],
        actions: vec![ActionSpec {
            id: "remove".to_owned(),
            label: "Remove object".to_owned(),
            estimated_free: bytes,
            options: Vec::new(),
        }],
    })
}

/// A mark wins over any age; then 30 days untouched is Safe and anything newer is Review.
fn verdict(age_days: i64, keep: bool) -> Verdict {
    if keep {
        return Verdict::new(
            Level::Keep,
            vec![Reason::new("marked-keep", "It holds a KEEP file")],
        );
    }
    if age_days >= STALE_AFTER_DAYS {
        return Verdict::new(
            Level::Safe,
            vec![Reason::new(
                "stale",
                format!("Not modified for {}", count(age_days.unsigned_abs(), "day")),
            )],
        );
    }
    let when = if age_days < 1 {
        "Modified today".to_owned()
    } else {
        format!("Modified {} ago", count(age_days.unsigned_abs(), "day"))
    };
    Verdict::new(Level::Review, vec![Reason::new("recent", when)])
}

/// "1 file", "4 files".
fn count(n: u64, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// What a folder holds: its allocated bytes (its own blocks included, since deleting it
/// frees them too), the files in it, and the newest time anything in it was modified.
struct Usage {
    bytes: u64,
    files: u64,
    newest: DateTime<Utc>,
}

/// Walks `path` without following a symlink.
fn usage(path: &Path) -> Result<Usage, ModuleError> {
    let meta = fs::symlink_metadata(path).map_err(io_at(path))?;
    let mut total = Usage {
        bytes: meta.blocks() * 512,
        files: 0,
        newest: modified(&meta, path)?,
    };
    if !meta.is_dir() {
        total.files = 1;
        return Ok(total);
    }
    for entry in fs::read_dir(path).map_err(io_at(path))? {
        let child: PathBuf = path.join(entry.map_err(io_at(path))?.file_name());
        let inner = usage(&child)?;
        total.bytes += inner.bytes;
        total.files += inner.files;
        total.newest = total.newest.max(inner.newest);
    }
    Ok(total)
}

fn modified(meta: &fs::Metadata, path: &Path) -> Result<DateTime<Utc>, ModuleError> {
    meta.modified().map(DateTime::from).map_err(io_at(path))
}

/// An I/O error, with the path it was about.
fn io_at(path: &Path) -> impl Fn(std::io::Error) -> ModuleError + '_ {
    move |source| ModuleError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use storage_monitor_core::action::{EntryResult, Limits};
    use storage_monitor_core::cleanup::{self, Held, Request};
    use storage_monitor_core::system::{Reply, System, TestSystem};

    /// The data dir where the app keeps it, inside the temporary home: under `~/Library/
    /// Application Support`, whose contents the guards judge one by one (ADR 0007).
    fn data_dir(sys: &TestSystem) -> PathBuf {
        sys.root()
            .join("Library/Application Support/storage-monitor")
    }

    fn discover(sys: &TestSystem) -> Vec<Item> {
        let data = data_dir(sys);
        Demo.discover(&Context {
            sys,
            home: sys.root(),
            data_dir: &data,
        })
        .unwrap()
    }

    fn by_title<'a>(items: &'a [Item], title: &str) -> &'a Item {
        items
            .iter()
            .find(|item| item.title == title)
            .unwrap_or_else(|| panic!("no item {title}"))
    }

    fn codes(item: &Item) -> Vec<&str> {
        item.verdict
            .reasons
            .iter()
            .map(|reason| reason.code.as_str())
            .collect()
    }

    #[test]
    fn the_first_discovery_seeds_the_sandbox() {
        let sys = TestSystem::new();
        let items = discover(&sys);
        let titles: Vec<&str> = items.iter().map(|item| item.title.as_str()).collect();
        assert_eq!(
            titles,
            vec![
                "fresh.object",
                "build-cache",
                "logs",
                "old.object",
                "keepsake"
            ],
            "largest first"
        );
        assert!(data_dir(&sys).join("demo/.seeded").is_file());
        for item in &items {
            assert_eq!(item.id, format!("demo:{}", item.title));
            assert!(item.size.bytes > 0, "{} allocates something", item.title);
        }
        // Real bytes, not holes: at least what was written.
        assert!(by_title(&items, "build-cache").size.bytes >= 2_000_000);
        assert!(by_title(&items, "fresh.object").size.bytes >= 3_000_000);
    }

    #[test]
    fn a_later_discovery_does_not_seed_again() {
        let sys = TestSystem::new();
        let items = discover(&sys);
        let old = by_title(&items, "old.object").path.clone().unwrap();
        // Through the port, as the engine would have removed it.
        sys.remove(&old).unwrap();
        let again = discover(&sys);
        assert_eq!(again.len(), 4);
        assert!(again.iter().all(|item| item.title != "old.object"));
    }

    #[test]
    fn an_unfinished_seeding_is_finished_by_the_next_discovery() {
        let sys = TestSystem::new();
        discover(&sys);
        let sandbox = data_dir(&sys).join("demo");
        sys.remove(&sandbox.join(".seeded")).unwrap();
        sys.remove(&sandbox.join("logs")).unwrap();
        assert_eq!(
            discover(&sys).len(),
            5,
            "seeded again, the missing folder included"
        );
    }

    #[test]
    fn deleting_the_sandbox_brings_it_back() {
        let sys = TestSystem::new();
        discover(&sys);
        sys.remove(&data_dir(&sys).join("demo")).unwrap();
        assert_eq!(discover(&sys).len(), 5);
    }

    #[test]
    fn verdicts_follow_age_and_the_keep_marker() {
        let sys = TestSystem::new();
        let items = discover(&sys);
        let build = by_title(&items, "build-cache");
        assert_eq!(build.verdict.level, Level::Safe);
        assert_eq!(codes(build), vec!["stale"]);
        assert_eq!(build.verdict.reasons[0].text, "Not modified for 45 days");
        let logs = by_title(&items, "logs");
        assert_eq!(logs.verdict.level, Level::Review);
        assert_eq!(logs.verdict.reasons[0].text, "Modified 3 days ago");
        // Two hundred days old and still Keep: the mark wins over the age.
        let keepsake = by_title(&items, "keepsake");
        assert_eq!(keepsake.verdict.level, Level::Keep);
        assert_eq!(codes(keepsake), vec!["marked-keep"]);
        assert_eq!(by_title(&items, "old.object").verdict.level, Level::Safe);
        let fresh = by_title(&items, "fresh.object");
        assert_eq!(fresh.verdict.level, Level::Review);
        assert_eq!(fresh.verdict.reasons[0].text, "Modified 1 day ago");
    }

    #[test]
    fn folders_report_their_files_and_when_they_changed() {
        let sys = TestSystem::new();
        let items = discover(&sys);
        let build = by_title(&items, "build-cache");
        assert_eq!(build.kind, "folder");
        assert_eq!(build.subtitle.as_deref(), Some("4 files"));
        assert_eq!(build.facts[0].value, FactValue::Count(4));
        let forty_five_days_ago = sys.now() - chrono::TimeDelta::days(45);
        assert_eq!(build.facts[1].value, FactValue::Date(forty_five_days_ago));
        assert_eq!(build.last_used, Some(forty_five_days_ago));
        assert_eq!(build.facts[2].value, FactValue::Flag(false));
        assert!(!build.size.estimated);
        // The mark is a file of the folder like any other.
        assert_eq!(
            by_title(&items, "keepsake").subtitle.as_deref(),
            Some("2 files")
        );
    }

    #[test]
    fn objects_report_an_estimated_size() {
        let sys = TestSystem::new();
        let items = discover(&sys);
        let object = by_title(&items, "old.object");
        assert_eq!(object.kind, "object");
        assert!(object.size.estimated);
        assert_eq!(object.facts[1].value, FactValue::Bytes(1_000_000));
        assert_eq!(object.actions[0].estimated_free, object.size.bytes);
    }

    #[test]
    fn only_a_keep_folder_offers_a_force_option() {
        let sys = TestSystem::new();
        let items = discover(&sys);
        let force = &by_title(&items, "keepsake").actions[0].options;
        assert_eq!(force.len(), 1);
        assert!(force[0].force && !force[0].default);
        assert!(
            by_title(&items, "build-cache").actions[0]
                .options
                .is_empty()
        );
    }

    #[test]
    fn a_folder_plans_two_steps_in_trash_mode_and_one_in_permanent() {
        let sys = TestSystem::new();
        let items = discover(&sys);
        let build = by_title(&items, "build-cache");
        let path = build.path.clone().unwrap();
        let action = &build.actions[0];
        let trash = Demo.plan(build, action, &[], Mode::Trash);
        assert_eq!(
            trash,
            vec![
                Step::Delete(Target::new(&path, NodeKind::Dir)),
                Step::Run {
                    command: Command::new("touch")
                        .arg(path.parent().unwrap().join(".last-cleanup")),
                    effect: Effect::Housekeeping,
                },
            ]
        );
        assert_eq!(
            Demo.plan(build, action, &[], Mode::Permanent),
            vec![Step::Delete(Target::new(&path, NodeKind::Dir))]
        );
    }

    #[test]
    fn an_object_plans_rm_in_both_modes() {
        let sys = TestSystem::new();
        let items = discover(&sys);
        let object = by_title(&items, "old.object");
        let path = object.path.clone().unwrap();
        let expected = vec![Step::Run {
            command: Command::new("rm").arg(&path),
            effect: Effect::Removes(Target::new(&path, NodeKind::File)),
        }];
        for mode in [Mode::Trash, Mode::Permanent] {
            assert_eq!(Demo.plan(object, &object.actions[0], &[], mode), expected);
        }
    }

    #[test]
    fn a_plan_for_an_action_it_does_not_know_is_empty() {
        let sys = TestSystem::new();
        let items = discover(&sys);
        let folder = by_title(&items, "logs");
        let other = ActionSpec {
            id: "remove".to_owned(),
            ..folder.actions[0].clone()
        };
        assert!(Demo.plan(folder, &other, &[], Mode::Trash).is_empty());
    }

    #[test]
    fn available_when_rm_and_touch_are_installed() {
        let sys = TestSystem::new();
        sys.install("rm");
        sys.install("touch");
        let data = data_dir(&sys);
        let ctx = Context {
            sys: &sys,
            home: sys.root(),
            data_dir: &data,
        };
        assert_eq!(Demo.availability(&ctx), Availability::Available);
    }

    #[test]
    fn unavailable_names_the_missing_tool() {
        let sys = TestSystem::new();
        sys.install("rm");
        let data = data_dir(&sys);
        let ctx = Context {
            sys: &sys,
            home: sys.root(),
            data_dir: &data,
        };
        assert_eq!(
            Demo.availability(&ctx),
            Availability::Unavailable {
                reason: "`touch` is not installed".to_owned()
            }
        );
    }

    /// The exit criterion of phase 2b in the core's terms: discovered, cleaned through the
    /// engine in Trash mode, discovered again.
    #[test]
    fn the_demo_cleans_end_to_end_through_the_engine() {
        let sys = Arc::new(TestSystem::new());
        sys.install("rm");
        sys.install("touch");
        let items = discover(&sys);
        let build = by_title(&items, "build-cache").path.clone().unwrap();
        let old = by_title(&items, "old.object").path.clone().unwrap();
        let sandbox = build.parent().unwrap().to_path_buf();
        sys.script(
            "touch",
            &[sandbox.join(".last-cleanup").as_os_str()],
            Reply::ok(),
        );
        // The scripted `rm` does what the real one does, through the port.
        let port = Arc::clone(&sys);
        let doomed = old.clone();
        sys.script(
            "rm",
            &[old.as_os_str()],
            Reply::ok().then(move || port.remove(&doomed).unwrap()),
        );

        let requests = [
            Request {
                item: "demo:build-cache".to_owned(),
                action: "delete".to_owned(),
                options: Vec::new(),
            },
            Request {
                item: "demo:old.object".to_owned(),
                action: "remove".to_owned(),
                options: Vec::new(),
            },
            // Kept: asked for without its force option.
            Request {
                item: "demo:keepsake".to_owned(),
                action: "delete".to_owned(),
                options: Vec::new(),
            },
        ];
        let held = Held::new([(&Demo as &dyn Module, items.as_slice())]);
        let limits = Limits::for_home(sys.root().to_path_buf());
        let outcome = cleanup::execute(&requests, &held, &limits, &*sys, Mode::Trash, &mut |_| {});

        assert!(matches!(
            outcome.entries[0].result,
            EntryResult::Removed { .. }
        ));
        assert!(matches!(
            outcome.entries[1].result,
            EntryResult::Removed { .. }
        ));
        assert!(matches!(
            outcome.entries[2].result,
            EntryResult::Skipped { .. }
        ));
        assert!(sys.trash_dir().join("build-cache").is_dir(), "in the Trash");
        assert!(!old.exists(), "removed by its command");
        assert_eq!(outcome.entries[0].mode, Mode::Trash);
        assert_eq!(outcome.entries[1].mode, Mode::Permanent);

        let after: Vec<String> = discover(&sys).into_iter().map(|item| item.title).collect();
        assert_eq!(after, vec!["fresh.object", "logs", "keepsake"]);
    }
}
