//! The one door to the outside world: filesystem operations that delete things, running a
//! program, and the clock. Everything the engines touch goes through this trait, so tests
//! run against a temporary filesystem and scripted programs on any platform.

mod process;
mod real;
#[cfg(any(test, feature = "testing"))]
mod test;

use std::ffi::OsString;
use std::fs::Metadata;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};

pub use real::RealSystem;
#[cfg(any(test, feature = "testing"))]
pub use test::{Reply, TestSystem};

#[derive(Debug, thiserror::Error)]
pub enum SystemError {
    #[error("{} no longer exists", .0.display())]
    Missing(PathBuf),
    #[error("cannot read {}: {source}", path.display())]
    Metadata {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot move {} to the Trash: {message}", path.display())]
    Trash { path: PathBuf, message: String },
    #[error("cannot delete {}: {source}", path.display())]
    Remove {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The path is not one the port accepts; nothing was touched.
    #[error("refusing {}: {reason}", path.display())]
    Rejected { path: PathBuf, reason: &'static str },
    /// The program could not be started at all.
    #[error("cannot start {}: {source}", program.display())]
    Spawn {
        program: PathBuf,
        source: std::io::Error,
    },
    /// The program started and could not be waited for.
    #[error("cannot wait for {}: {source}", program.display())]
    Wait {
        program: PathBuf,
        source: std::io::Error,
    },
    /// The program ran past its deadline and was killed, with every process it started.
    #[error("{} did not finish within {} s and was stopped", program.display(), after.as_secs_f64())]
    TimedOut { program: PathBuf, after: Duration },
    /// The program wrote more than the port keeps. It ran to its end; its output is lost,
    /// because half a document parses as garbage.
    #[error("{} wrote more output than the app reads", program.display())]
    OutputTooLarge { program: PathBuf },
}

/// A program to run: the argv, where, with what added to the environment, and for how long.
///
/// No shell ever sees it. The program and the arguments reach `execve` exactly as they are,
/// so an argument holding `*` or `$HOME` is that string and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// Absolute and in normal form, like every path the port takes. Nothing is resolved
    /// here: a bare name is refused rather than looked up against the working directory.
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Where it runs; the caller's working directory when `None`.
    pub cwd: Option<PathBuf>,
    /// Added to the environment the app inherited, never replacing it.
    pub env: Vec<(OsString, OsString)>,
    /// When the program and everything it started are killed.
    pub timeout: Duration,
}

impl Invocation {
    /// `program` with no arguments, run where the app runs, for `timeout`.
    pub fn new(program: impl Into<PathBuf>, timeout: Duration) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            timeout,
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

/// What a program did. A non-zero exit is an `Output` like any other: what it means is the
/// caller's business, and a caller that only wanted to read something may well expect one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    /// The exit code; `None` when a signal ended the process.
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    /// Exited with 0.
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }
}

/// Deleting, reading an entry without following it, and the clock.
///
/// Every path must be **absolute and in normal form**, the way a scan produces them: no
/// `..`, and byte for byte what `Path::components` rebuilds, so no trailing separator, no
/// `.` and no repeated separator. Anything else is [`SystemError::Rejected`] and nothing
/// is touched. The rule is not pedantry — the kernel reads the path the caller wrote, and
/// both endings name something other than the entry they seem to: `remove("/a/b/c/..")`
/// deletes the contents of `/a/b`, and `remove("/a/link/")` deletes what `link` points at.
pub trait System: Send + Sync {
    /// Metadata that does not follow symlinks.
    fn symlink_metadata(&self, path: &Path) -> Result<Metadata, SystemError>;
    /// Moves the entry to the Trash. A symlink is moved as a link.
    fn move_to_trash(&self, path: &Path) -> Result<(), SystemError>;
    /// Deletes a file, a symlink or a whole directory tree, permanently.
    ///
    /// Not atomic: a tree can be deleted in part and then fail, so a caller that cares
    /// about what survived has to look, not assume.
    fn remove(&self, path: &Path) -> Result<(), SystemError>;
    /// The current time. In the port because the action log is timestamped and tests
    /// compare those timestamps.
    fn now(&self) -> DateTime<Utc>;
    /// Where `tool` is installed: the first executable regular file of that name in the
    /// absolute entries of `PATH`, then in the folders where macOS and Homebrew put tools.
    /// A name, never a path: anything holding a `/` answers `None`. The answer is in the
    /// normal form [`System::run`] demands.
    ///
    /// Not every file found is safe to run. On a Mac without the Command Line Tools,
    /// `/usr/bin/git` and `/usr/bin/xcrun` are shims that open an installation dialog, and
    /// this finds them all the same; a module that runs them asks `xcode-select -p` first.
    fn locate(&self, tool: &str) -> Option<PathBuf>;
    /// Runs `invocation` to its end or to its timeout. Stdin is closed, stdout and stderr
    /// are captured, and at the deadline the program is killed together with every process
    /// it started. A non-zero exit is an [`Output`], not an error.
    fn run(&self, invocation: &Invocation) -> Result<Output, SystemError>;
}

/// Enforces the invariant documented on [`System`].
fn check_path(path: &Path) -> Result<(), SystemError> {
    let reject = |reason| {
        Err(SystemError::Rejected {
            path: path.to_path_buf(),
            reason,
        })
    };
    if !path.is_absolute() {
        return reject("the path is not absolute");
    }
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return reject("the path contains `..`");
    }
    // Byte comparison, not `==`: `Path` compares component-wise and calls `/a/b/` and
    // `/a/b` equal, which is exactly the difference that matters. `components()` drops a
    // trailing separator, a trailing `.` and a repeated one, but the raw string is what
    // reaches the syscall, and POSIX resolves a last component written as a directory by
    // following it — so `remove("/a/link/")` deletes what the link points at.
    let normal: PathBuf = path.components().collect();
    if normal.as_os_str() != path.as_os_str() {
        return reject("the path is not in normal form");
    }
    Ok(())
}
