//! The contract a cleanup module implements (ADR 0008).
//!
//! A module finds things and judges them; it never removes anything. Asked how to remove an
//! item, it answers with [`Step`]s, and the cleanup engine runs those through the guards —
//! so no module can reach a path the guards refuse, run a command the user was not shown,
//! or skip the record. Modules live in crates under `crates/modules/`, where a
//! `clippy.toml` refuses the calls of the standard library that delete or start a process.

mod item;
mod step;

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::action::Mode;
use crate::system::{Invocation, Output, System, SystemError};

pub use item::{ActionOption, ActionSpec, Fact, FactValue, Item, Level, Reason, Size, Verdict};
pub use step::{Command, Effect, Step, Target, command_line};

/// How long a command a module runs while looking around may take. Discovery reads; a
/// minute is long enough for `docker system df -v` on a busy engine and short enough that a
/// hung daemon does not leave the Cleanup screen saying "Discovering…" for ever.
pub const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(60);

/// A cleanup module.
///
/// Synchronous (ADR 0008): the core is synchronous throughout and the CLI has no async
/// runtime, and a module's discovery is a few commands in sequence — the app runs every
/// module on a thread of its own instead.
pub trait Module: Send + Sync {
    fn descriptor(&self) -> Descriptor;

    /// Whether the module can work on this machine at all: its tools are there, its daemon
    /// answers. Cheap, and asked before every discovery.
    fn availability(&self, ctx: &Context) -> Availability;

    /// Everything the module finds. Reads only — with `std::fs` under the paths of the
    /// context, and through [`Context::run`] for tools.
    fn discover(&self, ctx: &Context) -> Result<Vec<Item>, ModuleError>;

    /// The steps that carry out `action` on `item` in `mode`, with the ids of the options
    /// turned on.
    ///
    /// A plain function of its arguments: no `System`, no clock, nothing to look at. The
    /// core has already checked that the item offers the action and that every option
    /// belongs to it. An empty plan is a bug in the module, and the core refuses the entry
    /// rather than calling it done.
    fn plan(&self, item: &Item, action: &ActionSpec, options: &[String], mode: Mode) -> Vec<Step>;
}

/// Who a module is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Descriptor {
    /// The prefix of its item ids, and its key everywhere else.
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// The tools [`Module::availability`] looks for, listed by `storage-monitor modules
    /// list`.
    pub tools: &'static [&'static str],
}

/// Whether a module can work here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Available,
    /// With a sentence for the user: "`docker` is not installed".
    Unavailable {
        reason: String,
    },
}

/// What a module gets to look around with. The home folder and the data dir are here
/// rather than read from `paths::`, so that a test hands a temporary tree to both.
#[derive(Clone, Copy)]
pub struct Context<'a> {
    pub sys: &'a dyn System,
    pub home: &'a Path,
    /// The app's own data dir, beside `snapshots/` and `actions.jsonl`.
    pub data_dir: &'a Path,
}

impl Context<'_> {
    /// Runs `command` for discovery: the tool located now, the [`DISCOVERY_TIMEOUT`], and a
    /// missing tool as an error that names it. A non-zero exit is an [`Output`] like any
    /// other; [`Context::run_ok`] is the one that minds.
    pub fn run(&self, command: &Command) -> Result<Output, ModuleError> {
        let program = self
            .sys
            .locate(&command.tool)
            .ok_or_else(|| ModuleError::ToolMissing(command.tool.clone()))?;
        let invocation = Invocation {
            program,
            args: command.args.clone(),
            cwd: command.cwd.clone(),
            env: command.env.clone(),
            timeout: DISCOVERY_TIMEOUT,
        };
        Ok(self.sys.run(&invocation)?)
    }

    /// [`Context::run`], with anything but exit 0 turned into an error that carries the
    /// command, how it ended and the end of what it wrote to stderr.
    pub fn run_ok(&self, command: &Command) -> Result<Output, ModuleError> {
        let output = self.run(command)?;
        if output.success() {
            return Ok(output);
        }
        Err(ModuleError::Failed {
            command: command.line(),
            ending: ending(output.status),
            stderr: stderr_tail(&output.stderr),
        })
    }
}

/// Why a discovery could not finish.
#[derive(Debug, thiserror::Error)]
pub enum ModuleError {
    #[error("`{0}` is not installed")]
    ToolMissing(String),
    #[error("`{command}` {ending}: {stderr}")]
    Failed {
        command: String,
        /// "exited with 3", "was killed by a signal".
        ending: String,
        stderr: String,
    },
    #[error(transparent)]
    System(#[from] SystemError),
    #[error("cannot read {}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{0}")]
    Other(String),
}

/// How a program ended, in words: "exited with 3", or "was killed by a signal" when no code
/// came back.
pub fn ending(status: Option<i32>) -> String {
    match status {
        Some(code) => format!("exited with {code}"),
        None => "was killed by a signal".to_owned(),
    }
}

/// What a failure message quotes of stderr: the last three lines that say anything, joined,
/// at most 500 characters. The end, because that is where a tool says what went wrong.
pub fn stderr_tail(stderr: &[u8]) -> String {
    const LINES: usize = 3;
    const CHARS: usize = 500;
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let tail = lines[lines.len().saturating_sub(LINES)..].join(" / ");
    let count = tail.chars().count();
    if count <= CHARS {
        return tail;
    }
    // The end is kept, not the start: the last words are the ones that name the problem.
    let skip = count - CHARS;
    format!("…{}", tail.chars().skip(skip + 1).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::{Reply, TestSystem};

    fn context(sys: &TestSystem) -> Context<'_> {
        Context {
            sys,
            home: sys.root(),
            data_dir: sys.root(),
        }
    }

    #[test]
    fn context_run_locates_the_tool_and_runs_it() {
        let sys = TestSystem::new();
        sys.install("git");
        sys.script("git", &["status"], Reply::ok().stdout("clean"));
        let output = context(&sys)
            .run(&Command::new("git").arg("status"))
            .unwrap();
        assert_eq!(output.stdout, b"clean");
        assert_eq!(sys.ran(), vec![vec!["git".to_owned(), "status".to_owned()]]);
    }

    #[test]
    fn context_run_names_a_tool_that_is_not_installed() {
        let sys = TestSystem::new();
        let err = context(&sys)
            .run(&Command::new("docker").arg("ps"))
            .unwrap_err();
        assert!(matches!(&err, ModuleError::ToolMissing(tool) if tool == "docker"));
        assert_eq!(err.to_string(), "`docker` is not installed");
        assert!(sys.ran().is_empty(), "nothing ran");
    }

    #[test]
    fn context_run_leaves_a_non_zero_exit_to_the_caller() {
        let sys = TestSystem::new();
        sys.install("git");
        sys.script("git", &["status"], Reply::exit(128));
        let output = context(&sys)
            .run(&Command::new("git").arg("status"))
            .unwrap();
        assert_eq!(output.status, Some(128));
    }

    #[test]
    fn context_run_ok_turns_a_non_zero_exit_into_an_error() {
        let sys = TestSystem::new();
        sys.install("git");
        sys.script(
            "git",
            &["-C", "/h/my repo", "status"],
            Reply::exit(128).stderr("warning: noise\nfatal: not a git repository\n"),
        );
        let err = context(&sys)
            .run_ok(&Command::new("git").args(["-C", "/h/my repo", "status"]))
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "`git -C '/h/my repo' status` exited with 128: warning: noise / fatal: not a git repository"
        );
    }

    #[test]
    fn a_port_failure_is_a_module_error() {
        let sys = TestSystem::new();
        sys.install("gh");
        sys.script("gh", &["pr", "list"], Reply::timeout());
        let err = context(&sys)
            .run(&Command::new("gh").args(["pr", "list"]))
            .unwrap_err();
        assert!(matches!(
            err,
            ModuleError::System(SystemError::TimedOut { .. })
        ));
    }

    #[test]
    fn an_ending_says_how_the_program_stopped() {
        assert_eq!(ending(Some(3)), "exited with 3");
        assert_eq!(ending(None), "was killed by a signal");
    }

    #[test]
    fn the_tail_of_stderr_keeps_the_last_lines_that_say_something() {
        assert_eq!(
            stderr_tail(b"one\n\ntwo\n  \nthree\nfour\n"),
            "two / three / four"
        );
        assert_eq!(stderr_tail(b""), "");
    }

    #[test]
    fn a_long_tail_keeps_its_end() {
        let long = format!("{}END", "x".repeat(2000));
        let tail = stderr_tail(long.as_bytes());
        assert!(tail.starts_with('…'));
        assert!(tail.ends_with("END"));
        assert_eq!(tail.chars().count(), 500);
    }
}
