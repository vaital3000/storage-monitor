use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use tempfile::TempDir;

use super::{Invocation, Output, RealSystem, System, SystemError, check_path};

/// What an injected failure reports.
const INJECTED: &str = "the test system was told to fail";

/// What a scripted run answers, and what it does to the temporary tree when it happens.
///
/// A reply is used once. The effect is how a test gives a scripted `rm` the consequence the
/// real one has — without it the file is still there after a run that answered success, and
/// the next discovery finds an item the batch reported removed.
pub struct Reply {
    outcome: Scripted,
    effect: Option<Box<dyn FnOnce() + Send>>,
}

enum Scripted {
    Output(Output),
    TimedOut,
}

impl Reply {
    /// Exit 0, nothing written.
    pub fn ok() -> Self {
        Self::exit(0)
    }

    /// Exit `code`, nothing written.
    pub fn exit(code: i32) -> Self {
        Self::output(Some(code))
    }

    /// Ended by a signal: an `Output` whose status is `None`.
    pub fn killed() -> Self {
        Self::output(None)
    }

    fn output(status: Option<i32>) -> Self {
        Self {
            outcome: Scripted::Output(Output {
                status,
                ..Output::default()
            }),
            effect: None,
        }
    }

    /// The port's own failure: the program ran past its deadline.
    pub fn timeout() -> Self {
        Self {
            outcome: Scripted::TimedOut,
            effect: None,
        }
    }

    pub fn stdout(mut self, text: impl Into<Vec<u8>>) -> Self {
        if let Scripted::Output(output) = &mut self.outcome {
            output.stdout = text.into();
        }
        self
    }

    pub fn stderr(mut self, text: impl Into<Vec<u8>>) -> Self {
        if let Scripted::Output(output) = &mut self.outcome {
            output.stderr = text.into();
        }
        self
    }

    /// Runs `effect` when the scripted run happens — not when it is scripted.
    pub fn then(mut self, effect: impl FnOnce() + Send + 'static) -> Self {
        self.effect = Some(Box::new(effect));
        self
    }
}

/// One run recorded from a real machine: what `TestSystem::replay` reads.
#[derive(serde::Deserialize)]
struct Fixture {
    tool: String,
    args: Vec<String>,
    /// `null` when a signal ended the process.
    status: Option<i32>,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    stderr: String,
}

/// A script is found by the program's file name and the arguments: where the tool was
/// installed is the machine's business, and a fixture recorded on one Mac has to replay on
/// another and on Linux.
type ScriptKey = (OsString, Vec<OsString>);

/// A [`System`] confined to a temporary directory: the real filesystem for metadata and
/// deletion, a Trash that is just another folder, a clock that moves only when a test moves
/// it, and programs that never run — a test says which tools exist and what each run
/// answers. Everything it touches disappears when it is dropped.
///
/// Deleting outside that directory panics rather than fails, so a test of a broken guard
/// cannot quietly wipe the developer's `$HOME`. One instance per test thread: the search
/// for a free name in the Trash is a check followed by a rename, which two threads sharing
/// one instance could interleave.
pub struct TestSystem {
    // Owns the temporary tree; dropping it removes `root` and `trash`.
    _dir: TempDir,
    /// The canonical temporary directory: on macOS `TempDir::path` starts with `/var` and
    /// canonicalizing it yields `/private/var`, and confinement compares canonical paths.
    base: PathBuf,
    root: PathBuf,
    trash: PathBuf,
    clock: Mutex<DateTime<Utc>>,
    /// Paths whose next deletion fails; see [`TestSystem::fail_next`].
    failures: Mutex<HashSet<PathBuf>>,
    /// Where [`System::locate`] says the installed tools are. Never created: nothing runs.
    bin: PathBuf,
    /// The tools [`TestSystem::install`] made locatable.
    tools: Mutex<HashSet<String>>,
    /// Replies by program name and arguments, used in order.
    scripts: Mutex<HashMap<ScriptKey, VecDeque<Reply>>>,
    /// Every run, oldest first, as its program's file name and the arguments.
    ran: Mutex<Vec<Vec<String>>>,
}

impl TestSystem {
    /// Creates `root/` and `trash/` in a fresh temporary directory.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("create the temporary directory");
        let base = dir
            .path()
            .canonicalize()
            .expect("canonicalize the temporary directory");
        let root = base.join("root");
        let trash = base.join("trash");
        let bin = base.join("bin");
        fs::create_dir(&root).expect("create the root directory");
        fs::create_dir(&trash).expect("create the trash directory");
        Self {
            _dir: dir,
            base,
            root,
            trash,
            clock: Mutex::new(
                Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                    .single()
                    .expect("a valid start instant"),
            ),
            failures: Mutex::new(HashSet::new()),
            bin,
            tools: Mutex::new(HashSet::new()),
            scripts: Mutex::new(HashMap::new()),
            ran: Mutex::new(Vec::new()),
        }
    }

    /// The directory a test puts its files in.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where [`System::move_to_trash`] moves them.
    pub fn trash_dir(&self) -> &Path {
        &self.trash
    }

    /// Moves the clock forward.
    pub fn advance(&self, delta: TimeDelta) {
        *self.clock.lock().expect("an unpoisoned clock") += delta;
    }

    /// Makes the next [`System::move_to_trash`] or [`System::remove`] of `path` fail,
    /// leaving the entry in place. For the batch tests, where one entry has to fail
    /// without stopping the rest.
    pub fn fail_next(&self, path: &Path) {
        self.failures
            .lock()
            .expect("an unpoisoned failure list")
            .insert(path.to_path_buf());
    }

    /// Makes `tool` locatable, at `<base>/bin/<tool>`. Nothing is created there: a run is
    /// answered by its script, never by a file.
    pub fn install(&self, tool: &str) {
        self.tools
            .lock()
            .expect("an unpoisoned tool list")
            .insert(tool.to_owned());
    }

    /// The next run of `tool` with exactly these arguments answers `reply`. Scripts for one
    /// argv are used in the order they were given, each once.
    pub fn script<S: AsRef<OsStr>>(&self, tool: &str, args: &[S], reply: Reply) {
        let key = (
            OsString::from(tool),
            args.iter().map(|arg| arg.as_ref().to_owned()).collect(),
        );
        self.scripts
            .lock()
            .expect("an unpoisoned script")
            .entry(key)
            .or_default()
            .push_back(reply);
    }

    /// Scripts the run recorded in `fixture`: `{ "tool", "args", "status", "stdout",
    /// "stderr" }`, with `status` null for a process a signal ended.
    pub fn replay(&self, fixture: &Path) {
        let text = fs::read_to_string(fixture)
            .unwrap_or_else(|err| panic!("cannot read {}: {err}", fixture.display()));
        let recorded: Fixture = serde_json::from_str(&text)
            .unwrap_or_else(|err| panic!("{} is not a fixture: {err}", fixture.display()));
        let reply = Reply::output(recorded.status)
            .stdout(recorded.stdout)
            .stderr(recorded.stderr);
        self.script(&recorded.tool, &recorded.args, reply);
    }

    /// Every run so far, oldest first: the program's file name, then the arguments.
    pub fn ran(&self) -> Vec<Vec<String>> {
        self.ran.lock().expect("an unpoisoned record").clone()
    }

    /// Panics unless the path is inside the temporary directory. A panic, not an error:
    /// the guards of the action engine are developed test-first, and a test that hands
    /// `$HOME` to a guard that does not work yet must stop, not record an expected
    /// failure.
    fn assert_confined(&self, path: &Path) {
        // The parent is resolved, not the entry itself: canonicalizing the entry would
        // follow a symlink that is about to be deleted and answer with its target. A
        // parent that cannot be resolved leaves the path as it is; it is compared
        // literally then, and the deletion fails on its own.
        let resolved = match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => match parent.canonicalize() {
                Ok(parent) => parent.join(name),
                Err(_) => path.to_path_buf(),
            },
            _ => path.to_path_buf(),
        };
        assert!(
            resolved.starts_with(&self.base),
            "TestSystem refuses to touch {}, which is outside {}",
            path.display(),
            self.base.display(),
        );
    }

    /// Whether this deletion is one a test asked to fail, consuming the request.
    fn told_to_fail(&self, path: &Path) -> bool {
        self.failures
            .lock()
            .expect("an unpoisoned failure list")
            .remove(path)
    }

    /// The name the entry gets in the Trash: like the macOS Trash, a repeated name keeps
    /// both entries. Only the intent matches, not the spelling — macOS puts the counter
    /// before the extension and this does not, so no test should depend on the exact name.
    fn trash_target(&self, name: &OsStr) -> PathBuf {
        let mut target = self.trash.join(name);
        let mut suffix = 2;
        // Not `exists`, which follows symlinks: a dangling link in the Trash is still a
        // name that is taken.
        while target.symlink_metadata().is_ok() {
            let mut taken = name.to_os_string();
            taken.push(format!(" {suffix}"));
            target = self.trash.join(taken);
            suffix += 1;
        }
        target
    }
}

impl Default for TestSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl System for TestSystem {
    fn symlink_metadata(&self, path: &Path) -> Result<Metadata, SystemError> {
        RealSystem.symlink_metadata(path)
    }

    fn move_to_trash(&self, path: &Path) -> Result<(), SystemError> {
        check_path(path)?;
        self.assert_confined(path);
        if self.told_to_fail(path) {
            return Err(SystemError::Trash {
                path: path.to_path_buf(),
                message: INJECTED.to_owned(),
            });
        }
        // A safety net, not a live branch: only `/` has no file name by now, and it does
        // not get past `assert_confined`.
        let name = path.file_name().ok_or_else(|| SystemError::Trash {
            path: path.to_path_buf(),
            message: "the path has no file name".to_owned(),
        })?;
        // `rename` moves a symlink as a link, the way the macOS Trash does.
        fs::rename(path, self.trash_target(name)).map_err(|source| match source.kind() {
            io::ErrorKind::NotFound => SystemError::Missing(path.to_path_buf()),
            _ => SystemError::Trash {
                path: path.to_path_buf(),
                message: source.to_string(),
            },
        })
    }

    fn remove(&self, path: &Path) -> Result<(), SystemError> {
        check_path(path)?;
        self.assert_confined(path);
        if self.told_to_fail(path) {
            return Err(SystemError::Remove {
                path: path.to_path_buf(),
                source: io::Error::new(io::ErrorKind::PermissionDenied, INJECTED),
            });
        }
        RealSystem.remove(path)
    }

    fn now(&self) -> DateTime<Utc> {
        *self.clock.lock().expect("an unpoisoned clock")
    }

    fn locate(&self, tool: &str) -> Option<PathBuf> {
        self.tools
            .lock()
            .expect("an unpoisoned tool list")
            .contains(tool)
            .then(|| self.bin.join(tool))
    }

    /// Answers the script for this argv, and panics when there is none — a panic and not an
    /// error for the reason [`TestSystem::assert_confined`] gives: a test of a module whose
    /// plan went wrong must stop, not record an expected failure. Nothing is ever started.
    fn run(&self, invocation: &Invocation) -> Result<Output, SystemError> {
        check_path(&invocation.program)?;
        if let Some(cwd) = &invocation.cwd {
            check_path(cwd)?;
        }
        let name = invocation
            .program
            .file_name()
            .map(OsStr::to_owned)
            .unwrap_or_default();
        let argv: Vec<String> = std::iter::once(name.to_string_lossy().into_owned())
            .chain(
                invocation
                    .args
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned()),
            )
            .collect();
        self.ran
            .lock()
            .expect("an unpoisoned record")
            .push(argv.clone());
        let key = (name, invocation.args.clone());
        // Taken out before the effect runs, so that an effect may script or run again.
        let reply = self
            .scripts
            .lock()
            .expect("an unpoisoned script")
            .get_mut(&key)
            .and_then(VecDeque::pop_front);
        let Some(reply) = reply else {
            panic!("TestSystem has no script for `{}`", argv.join(" "));
        };
        if let Some(effect) = reply.effect {
            effect();
        }
        match reply.outcome {
            Scripted::Output(output) => Ok(output),
            Scripted::TimedOut => Err(SystemError::TimedOut {
                program: invocation.program.clone(),
                after: invocation.timeout,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    /// A symlink inside `root` pointing at a directory outside the temporary tree, with a
    /// file in it. The shape in which a path that looks confined deletes something else.
    fn door_to_the_outside(sys: &TestSystem) -> (TempDir, PathBuf, PathBuf) {
        let outside = tempfile::tempdir().unwrap();
        let precious = outside.path().join("precious.bin");
        fs::write(&precious, b"keep").unwrap();
        let door = sys.root().join("door");
        std::os::unix::fs::symlink(outside.path(), &door).unwrap();
        (outside, door, precious)
    }

    /// Appends raw bytes to a path, so a test states the dangerous form literally.
    /// `join` would build the same string for `"."` and `""`, but reading it would leave
    /// the reader guessing which of the two is the point.
    fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
        let mut raw = path.to_path_buf().into_os_string();
        raw.push(suffix);
        PathBuf::from(raw)
    }

    #[test]
    fn trash_moves_the_entry_into_the_trash_dir() {
        let sys = TestSystem::new();
        let file = sys.root().join("a.bin");
        fs::write(&file, b"xxx").unwrap();
        sys.move_to_trash(&file).unwrap();
        assert!(fs::symlink_metadata(&file).is_err());
        assert_eq!(fs::read(sys.trash_dir().join("a.bin")).unwrap(), b"xxx");
    }

    #[test]
    fn trash_keeps_both_entries_when_the_name_repeats() {
        let sys = TestSystem::new();
        for content in [b"one", b"two"] {
            let file = sys.root().join("same.bin");
            fs::write(&file, content).unwrap();
            sys.move_to_trash(&file).unwrap();
        }
        let names = fs::read_dir(sys.trash_dir()).unwrap().count();
        assert_eq!(names, 2, "the second entry must not overwrite the first");
    }

    #[test]
    fn trash_moves_a_directory_with_its_contents() {
        let sys = TestSystem::new();
        let dir = sys.root().join("tree");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("f.bin"), b"x").unwrap();
        sys.move_to_trash(&dir).unwrap();
        assert!(fs::symlink_metadata(&dir).is_err());
        assert_eq!(fs::read(sys.trash_dir().join("tree/f.bin")).unwrap(), b"x");
    }

    #[test]
    fn remove_deletes_a_directory_tree() {
        let sys = TestSystem::new();
        let dir = sys.root().join("tree/inner");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("f.bin"), b"x").unwrap();
        sys.remove(&sys.root().join("tree")).unwrap();
        assert!(fs::symlink_metadata(sys.root().join("tree")).is_err());
    }

    #[test]
    fn remove_deletes_a_symlink_without_touching_its_target() {
        let sys = TestSystem::new();
        let target = sys.root().join("target.bin");
        fs::write(&target, b"keep").unwrap();
        let link = sys.root().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        sys.remove(&link).unwrap();
        // Not `exists`, which follows the link: it would also pass if `remove` had deleted
        // the target and left the link behind.
        assert!(fs::symlink_metadata(&link).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
    }

    #[test]
    fn remove_deletes_a_symlink_to_a_directory_without_touching_the_directory() {
        let sys = TestSystem::new();
        let target = sys.root().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("inside.bin"), b"keep").unwrap();
        let link = sys.root().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        sys.remove(&link).unwrap();
        assert!(fs::symlink_metadata(&link).is_err());
        assert_eq!(fs::read(target.join("inside.bin")).unwrap(), b"keep");
    }

    #[test]
    fn removing_a_missing_path_is_reported_as_missing() {
        let sys = TestSystem::new();
        let err = sys.remove(&sys.root().join("nope")).unwrap_err();
        assert!(matches!(err, SystemError::Missing(_)), "got {err:?}");
    }

    #[test]
    fn trashing_a_missing_path_is_reported_as_missing() {
        let sys = TestSystem::new();
        let err = sys.move_to_trash(&sys.root().join("nope")).unwrap_err();
        assert!(matches!(err, SystemError::Missing(_)), "got {err:?}");
    }

    #[test]
    fn trash_moves_a_symlink_without_following_it() {
        let sys = TestSystem::new();
        let target = sys.root().join("target.bin");
        fs::write(&target, b"keep").unwrap();
        let link = sys.root().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        sys.move_to_trash(&link).unwrap();
        assert!(
            fs::symlink_metadata(sys.trash_dir().join("link"))
                .unwrap()
                .is_symlink()
        );
        assert_eq!(fs::read(&target).unwrap(), b"keep");
    }

    #[test]
    fn symlink_metadata_reports_the_link_not_the_target() {
        let sys = TestSystem::new();
        let dir = sys.root().join("dir");
        fs::create_dir(&dir).unwrap();
        let link = sys.root().join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(sys.symlink_metadata(&link).unwrap().is_symlink());
    }

    #[test]
    fn missing_path_is_reported_as_missing() {
        let sys = TestSystem::new();
        let err = sys.symlink_metadata(&sys.root().join("nope")).unwrap_err();
        assert!(matches!(err, SystemError::Missing(_)), "got {err:?}");
    }

    #[test]
    fn the_clock_is_fixed_and_can_be_moved() {
        let sys = TestSystem::new();
        let first = sys.now();
        assert_eq!(sys.now(), first, "the test clock does not drift");
        sys.advance(chrono::Duration::seconds(5));
        assert_eq!(sys.now(), first + chrono::Duration::seconds(5));
    }

    #[test]
    fn a_trailing_parent_component_is_rejected() {
        let sys = TestSystem::new();
        let inner = sys.root().join("b/c");
        fs::create_dir_all(&inner).unwrap();
        let sibling = sys.root().join("b/keep.bin");
        fs::write(&sibling, b"keep").unwrap();
        // The kernel resolves the `..`, so this would delete the contents of `b`.
        let err = sys.remove(&inner.join("..")).unwrap_err();
        assert!(matches!(err, SystemError::Rejected { .. }), "got {err:?}");
        assert_eq!(fs::read(&sibling).unwrap(), b"keep");
        assert!(inner.is_dir());
    }

    #[test]
    fn a_parent_component_in_the_middle_is_rejected() {
        let sys = TestSystem::new();
        let file = sys.root().join("a.bin");
        fs::write(&file, b"keep").unwrap();
        fs::create_dir(sys.root().join("b")).unwrap();
        let err = sys
            .move_to_trash(&sys.root().join("b/../a.bin"))
            .unwrap_err();
        assert!(matches!(err, SystemError::Rejected { .. }), "got {err:?}");
        assert_eq!(fs::read(&file).unwrap(), b"keep");
    }

    #[test]
    fn a_trailing_separator_is_rejected() {
        let sys = TestSystem::new();
        let (_outside, door, precious) = door_to_the_outside(&sys);
        // A last component written as a directory is resolved as one, which follows the
        // link: both of these would delete the directory outside the temporary tree.
        let err = sys.remove(&with_suffix(&door, "/")).unwrap_err();
        assert!(matches!(err, SystemError::Rejected { .. }), "got {err:?}");
        let err = sys.move_to_trash(&with_suffix(&door, "/")).unwrap_err();
        assert!(matches!(err, SystemError::Rejected { .. }), "got {err:?}");
        assert_eq!(fs::read(&precious).unwrap(), b"keep");
        assert!(fs::symlink_metadata(&door).unwrap().is_symlink());
    }

    #[test]
    fn a_trailing_dot_is_rejected() {
        let sys = TestSystem::new();
        let (_outside, door, precious) = door_to_the_outside(&sys);
        let err = sys.remove(&with_suffix(&door, "/.")).unwrap_err();
        assert!(matches!(err, SystemError::Rejected { .. }), "got {err:?}");
        assert_eq!(fs::read(&precious).unwrap(), b"keep");
        assert!(fs::symlink_metadata(&door).unwrap().is_symlink());
    }

    #[test]
    fn a_relative_path_is_rejected() {
        let sys = TestSystem::new();
        let err = sys.remove(Path::new("relative-only.bin")).unwrap_err();
        assert!(matches!(err, SystemError::Rejected { .. }), "got {err:?}");
    }

    #[test]
    #[should_panic(expected = "refuses to touch")]
    fn removing_outside_the_temporary_directory_panics() {
        let sys = TestSystem::new();
        let elsewhere = tempfile::tempdir().unwrap();
        let file = elsewhere.path().join("precious.bin");
        fs::write(&file, b"keep").unwrap();
        let _ = sys.remove(&file);
    }

    #[test]
    #[should_panic(expected = "refuses to touch")]
    fn trashing_outside_the_temporary_directory_panics() {
        let sys = TestSystem::new();
        let elsewhere = tempfile::tempdir().unwrap();
        let file = elsewhere.path().join("precious.bin");
        fs::write(&file, b"keep").unwrap();
        let _ = sys.move_to_trash(&file);
    }

    #[test]
    fn escaping_through_a_symlinked_parent_panics_too() {
        let sys = TestSystem::new();
        let (_outside, door, precious) = door_to_the_outside(&sys);
        let escaped = door.join("precious.bin");
        let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = sys.remove(&escaped);
        }))
        .expect_err("the path resolves outside the temporary directory");
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("<not a string>");
        assert!(message.contains("refuses to touch"), "got {message:?}");
        assert_eq!(fs::read(&precious).unwrap(), b"keep");
    }

    /// A run of `tool` from the fake bin, as a module's step would hand it over.
    fn invocation(sys: &TestSystem, tool: &str, args: &[&str]) -> Invocation {
        Invocation::new(sys.bin.join(tool), Duration::from_secs(1)).args(args.iter().copied())
    }

    #[test]
    fn an_installed_tool_is_located_in_the_fake_bin() {
        let sys = TestSystem::new();
        sys.install("rm");
        assert_eq!(sys.locate("rm"), Some(sys.bin.join("rm")));
        assert_eq!(sys.locate("git"), None);
    }

    #[test]
    fn a_scripted_run_answers_its_reply() {
        let sys = TestSystem::new();
        sys.script(
            "git",
            &["status"],
            Reply::exit(1).stdout("out").stderr("err"),
        );
        let output = sys.run(&invocation(&sys, "git", &["status"])).unwrap();
        assert_eq!(output.status, Some(1));
        assert_eq!(output.stdout, b"out");
        assert_eq!(output.stderr, b"err");
    }

    #[test]
    #[should_panic(expected = "no script for `rm /x`")]
    fn a_reply_is_used_once() {
        let sys = TestSystem::new();
        sys.script("rm", &["/x"], Reply::ok());
        sys.run(&invocation(&sys, "rm", &["/x"])).unwrap();
        let _ = sys.run(&invocation(&sys, "rm", &["/x"]));
    }

    #[test]
    fn scripts_match_on_the_file_name_and_the_arguments() {
        let sys = TestSystem::new();
        sys.script("rm", &["/x"], Reply::exit(7));
        // Wherever the tool was found, the script is about the tool.
        let elsewhere = Invocation::new("/opt/homebrew/bin/rm", Duration::from_secs(1)).arg("/x");
        assert_eq!(sys.run(&elsewhere).unwrap().status, Some(7));
    }

    #[test]
    #[should_panic(expected = "no script for `rm /y`")]
    fn different_arguments_do_not_match() {
        let sys = TestSystem::new();
        sys.script("rm", &["/x"], Reply::ok());
        let _ = sys.run(&invocation(&sys, "rm", &["/y"]));
    }

    #[test]
    fn replies_for_one_argv_are_used_in_order() {
        let sys = TestSystem::new();
        sys.script("docker", &["ps"], Reply::exit(1));
        sys.script("docker", &["ps"], Reply::exit(2));
        let first = sys.run(&invocation(&sys, "docker", &["ps"])).unwrap();
        let second = sys.run(&invocation(&sys, "docker", &["ps"])).unwrap();
        assert_eq!((first.status, second.status), (Some(1), Some(2)));
    }

    #[test]
    fn a_reply_acts_on_the_tree_when_the_run_happens() {
        let sys = TestSystem::new();
        let file = sys.root().join("old.object");
        fs::write(&file, b"x").unwrap();
        let target = file.clone();
        sys.script(
            "rm",
            &[file.as_os_str()],
            Reply::ok().then(move || fs::remove_file(target).unwrap()),
        );
        assert!(file.exists(), "scripting a reply changes nothing yet");
        sys.run(&invocation(&sys, "rm", &[file.to_str().unwrap()]))
            .unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn a_reply_can_fail_like_the_port() {
        let sys = TestSystem::new();
        sys.script("gh", &["pr", "list"], Reply::timeout());
        let err = sys
            .run(&invocation(&sys, "gh", &["pr", "list"]))
            .unwrap_err();
        assert!(matches!(err, SystemError::TimedOut { .. }), "got {err:?}");
    }

    #[test]
    fn a_killed_process_has_no_status() {
        let sys = TestSystem::new();
        sys.script("xcrun", &["simctl"], Reply::killed());
        let output = sys.run(&invocation(&sys, "xcrun", &["simctl"])).unwrap();
        assert_eq!(output.status, None);
    }

    #[test]
    fn every_run_is_recorded() {
        let sys = TestSystem::new();
        sys.script("git", &["status"], Reply::ok());
        sys.script("rm", &["/x"], Reply::ok());
        sys.run(&invocation(&sys, "git", &["status"])).unwrap();
        sys.run(&invocation(&sys, "rm", &["/x"])).unwrap();
        assert_eq!(
            sys.ran(),
            vec![
                vec!["git".to_owned(), "status".to_owned()],
                vec!["rm".to_owned(), "/x".to_owned()],
            ]
        );
    }

    #[test]
    fn a_fixture_file_scripts_a_run() {
        let sys = TestSystem::new();
        let fixture = sys.root().join("docker-ps.json");
        fs::write(
            &fixture,
            r#"{ "tool": "docker", "args": ["ps", "--format", "json"], "status": 0, "stdout": "[]\n" }"#,
        )
        .unwrap();
        sys.replay(&fixture);
        let output = sys
            .run(&invocation(&sys, "docker", &["ps", "--format", "json"]))
            .unwrap();
        assert!(output.success());
        assert_eq!(output.stdout, b"[]\n");
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn run_still_refuses_a_relative_program() {
        let sys = TestSystem::new();
        sys.script("rm", &["/x"], Reply::ok());
        let err = sys
            .run(&Invocation::new("rm", Duration::from_secs(1)).arg("/x"))
            .unwrap_err();
        assert!(matches!(err, SystemError::Rejected { .. }), "got {err:?}");
        assert!(sys.ran().is_empty(), "a refused run is not a run");
    }

    #[test]
    fn an_injected_failure_applies_once_to_the_named_path() {
        let sys = TestSystem::new();
        let trashed = sys.root().join("trashed.bin");
        let removed = sys.root().join("removed.bin");
        fs::write(&trashed, b"x").unwrap();
        fs::write(&removed, b"x").unwrap();
        sys.fail_next(&trashed);
        sys.fail_next(&removed);
        assert!(sys.move_to_trash(&trashed).is_err());
        assert!(sys.remove(&removed).is_err());
        assert!(trashed.exists() && removed.exists(), "nothing was touched");
        sys.move_to_trash(&trashed).unwrap();
        sys.remove(&removed).unwrap();
        assert!(!trashed.exists() && !removed.exists());
    }
}
