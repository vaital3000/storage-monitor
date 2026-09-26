//! Running a program with a deadline and a bounded appetite, for [`super::RealSystem`].
//!
//! The standard library starts a process and waits for it, and offers nothing else this
//! port needs: no timeout, no limit on what it reads, no way to stop what the process
//! itself started. The three are built here from `try_wait`, two reader threads and a
//! process group.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

use super::{Invocation, Output, SystemError};

/// What the port keeps of one stream. The largest thing a v1 module reads is the JSON of
/// `xcrun simctl list`, around a megabyte; this is room for thirty of them and still a
/// bound on what a runaway program can make the app hold.
pub(super) const OUTPUT_CAP: usize = 32 << 20;

/// Where tools are looked for after the absolute entries of `PATH`: Homebrew on Apple
/// Silicon and on Intel — where `gh` lives, and where Docker Desktop and OrbStack link
/// `docker` — then the system's own folders, which a GUI app's `PATH` holds anyway and a
/// process started with an empty environment does not.
pub(super) const KNOWN_DIRS: [&str; 6] = [
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

/// How often a running program is asked whether it is done.
const POLL: Duration = Duration::from_millis(10);

/// How long the readers get, once everything was killed, to hand over what they read.
const DRAIN: Duration = Duration::from_secs(1);

/// What one reader thread hands back: the bytes it kept, and whether there were more.
#[derive(Default)]
struct Captured {
    bytes: Vec<u8>,
    overflowed: bool,
}

/// Runs `invocation`, keeping at most `cap` bytes of each stream.
///
/// The deadline covers the program and its output alike. A program that exits while
/// something it started still holds its stdout — `tool &` in a script — is not finished
/// until that holder lets go, because the output is not complete until then; at the
/// deadline the holder is killed with the rest of the group and the answer is
/// [`SystemError::TimedOut`].
pub(super) fn run_capped(invocation: &Invocation, cap: usize) -> Result<Output, SystemError> {
    let program = &invocation.program;
    let mut command = Command::new(program);
    command
        .args(&invocation.args)
        .envs(invocation.env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A group of its own, led by the child, so that the deadline reaches everything it
        // started. A tool that forked leaves the grandchild holding the pipes, and killing
        // the child alone would leave the readers waiting for the grandchild instead.
        .process_group(0);
    if let Some(cwd) = &invocation.cwd {
        command.current_dir(cwd);
    }
    let mut child = command.spawn().map_err(|source| SystemError::Spawn {
        program: program.clone(),
        source,
    })?;
    let deadline = Instant::now() + invocation.timeout;
    // Both streams at once, each on a thread of its own: a program blocked writing to a full
    // pipe nobody reads never exits, and it can fill either one.
    let readers = [
        read(child.stdout.take(), cap),
        read(child.stderr.take(), cap),
    ];
    let timed_out = || SystemError::TimedOut {
        program: program.clone(),
        after: invocation.timeout,
    };

    let status = match wait_until(&mut child, deadline) {
        Ok(Some(status)) => status,
        Ok(None) => {
            stop(&mut child, readers);
            return Err(timed_out());
        }
        Err(source) => {
            stop(&mut child, readers);
            return Err(SystemError::Wait {
                program: program.clone(),
                source,
            });
        }
    };
    let Some([stdout, stderr]) = collect(readers, deadline) else {
        // `collect` hands the readers back when they are still blocked; nothing left to
        // wait for but whatever holds the pipes.
        kill_group(&child);
        return Err(timed_out());
    };
    if stdout.overflowed || stderr.overflowed {
        return Err(SystemError::OutputTooLarge {
            program: program.clone(),
        });
    }
    Ok(Output {
        status: status.code(),
        stdout: stdout.bytes,
        stderr: stderr.bytes,
    })
}

/// Waits for the program until `deadline`; `None` when it is still running then.
fn wait_until(child: &mut Child, deadline: Instant) -> io::Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(POLL);
    }
}

/// Kills the program and everything it started, reaps it, and gives the readers a moment to
/// see their pipes close. A reader that is still blocked after that is left behind: its
/// pipe is held by a process that left the group, and the thread ends when that one does.
fn stop(child: &mut Child, readers: [JoinHandle<Captured>; 2]) {
    kill_group(child);
    let _ = child.wait();
    let _ = collect(readers, Instant::now() + DRAIN);
}

/// Kills the process group the child leads.
///
/// The child's pid is the group's id. Once the child has been reaped that number could in
/// principle name another group — but not while any member of this one is alive, and a group
/// with no members left is one there is nothing to kill in (`ESRCH`, ignored).
fn kill_group(child: &Child) {
    // A pid always fits: `pid_t` is an `i32`, and std widened it to hand it out as `u32`.
    let _ = killpg(Pid::from_raw(child.id() as i32), Signal::SIGKILL);
}

/// Reads `pipe` to its end on a thread, keeping at most `cap` bytes and draining the rest.
fn read(pipe: Option<impl Read + Send + 'static>, cap: usize) -> JoinHandle<Captured> {
    thread::spawn(move || {
        let mut captured = Captured::default();
        let Some(mut pipe) = pipe else {
            return captured;
        };
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    let room = cap.saturating_sub(captured.bytes.len());
                    if read > room {
                        captured.overflowed = true;
                    }
                    captured.bytes.extend_from_slice(&chunk[..read.min(room)]);
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                // Nothing more can be read; what was read is all there is.
                Err(_) => break,
            }
        }
        captured
    })
}

/// Both readers' results, or `None` when either is still reading at `deadline`.
fn collect(readers: [JoinHandle<Captured>; 2], deadline: Instant) -> Option<[Captured; 2]> {
    while !readers.iter().all(JoinHandle::is_finished) {
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(POLL);
    }
    // A reader that panicked read nothing anyone can use; an empty stream says as much.
    Some(readers.map(|reader| reader.join().unwrap_or_default()))
}

/// The first executable regular file named `tool` in the absolute entries of `path_var`,
/// then in `known`, in the port's normal form.
///
/// A relative entry of `PATH` is skipped rather than resolved: it would name a different
/// folder for every working directory, and a tool found through one is a tool nobody chose.
/// `tool` is a name, so anything that could make it a path — a `/`, or being `.` or `..` —
/// answers `None`.
pub(super) fn locate_in(tool: &str, path_var: Option<&OsStr>, known: &[&str]) -> Option<PathBuf> {
    if tool.is_empty() || tool.contains('/') || tool == "." || tool == ".." {
        return None;
    }
    let from_path = path_var.into_iter().flat_map(std::env::split_paths);
    from_path
        .chain(known.iter().map(PathBuf::from))
        .filter_map(|dir| normal_dir(&dir))
        .map(|dir| dir.join(tool))
        .find(|candidate| is_executable(candidate))
}

/// `dir` in normal form, or `None` when it cannot be one: relative, or holding `..`, which
/// only the kernel could resolve and which the port refuses anyway.
fn normal_dir(dir: &Path) -> Option<PathBuf> {
    if !dir.is_absolute() || dir.components().any(|part| part == Component::ParentDir) {
        return None;
    }
    Some(dir.components().collect())
}

/// A regular file with any execute bit. Symlinks are followed: `/usr/local/bin/docker` is a
/// link into Docker Desktop's bundle, and it is the target that runs.
fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::{RealSystem, System};
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;

    const TIMEOUT: Duration = Duration::from_secs(10);

    fn run(invocation: &Invocation) -> Result<Output, SystemError> {
        RealSystem.run(invocation)
    }

    fn text(bytes: &[u8]) -> &str {
        std::str::from_utf8(bytes).unwrap()
    }

    #[test]
    fn run_captures_stdout_and_the_exit_status() {
        let output = run(&Invocation::new("/bin/echo", TIMEOUT).arg("hello")).unwrap();
        assert_eq!(output.status, Some(0));
        assert!(output.success());
        assert_eq!(text(&output.stdout), "hello\n");
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn a_non_zero_exit_is_an_output_not_an_error() {
        // `sh` because it is the plainest way to stage both an exit code and a line on
        // stderr. The rule against shell strings is about the commands the app runs for the
        // user; this is the port's own test of what it hands back.
        let output = run(&Invocation::new("/bin/sh", TIMEOUT)
            .arg("-c")
            .arg("echo no >&2; exit 3"))
        .unwrap();
        assert_eq!(output.status, Some(3));
        assert!(!output.success());
        assert_eq!(text(&output.stderr), "no\n");
    }

    #[test]
    fn run_passes_arguments_without_a_shell() {
        let output =
            run(&Invocation::new("/bin/echo", TIMEOUT).args(["$HOME", "*", "a b"])).unwrap();
        assert_eq!(text(&output.stdout), "$HOME * a b\n");
    }

    #[test]
    fn run_uses_the_working_directory() {
        let dir = tempfile::tempdir().unwrap();
        // Canonical, because `pwd` answers the physical path: `/private/var/…` on macOS.
        let dir = dir.path().canonicalize().unwrap();
        let output = run(&Invocation::new("/bin/pwd", TIMEOUT).cwd(&dir)).unwrap();
        assert_eq!(text(&output.stdout).trim_end(), dir.to_str().unwrap());
    }

    #[test]
    fn run_adds_to_the_environment() {
        let output = run(&Invocation::new("/usr/bin/env", TIMEOUT).env("SM_PROBE", "1")).unwrap();
        let listed = text(&output.stdout);
        assert!(
            listed.lines().any(|line| line == "SM_PROBE=1"),
            "got {listed}"
        );
        // Added to what the app inherited, not in place of it.
        assert!(
            listed.lines().any(|line| line.starts_with("PATH=")),
            "got {listed}"
        );
    }

    #[test]
    fn stdin_is_closed() {
        // `cat` with nothing to read ends at once; with an inherited terminal it would wait.
        let output = run(&Invocation::new("/bin/cat", TIMEOUT)).unwrap();
        assert_eq!(output.status, Some(0));
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn a_timeout_kills_the_program() {
        let started = Instant::now();
        let err =
            run(&Invocation::new("/bin/sleep", Duration::from_millis(100)).arg("10")).unwrap_err();
        assert!(matches!(err, SystemError::TimedOut { .. }), "got {err:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
    }

    /// Whether the process `pid` is gone, waiting up to two seconds for it to be: a killed
    /// process lingers until its new parent reaps it.
    fn gone(pid: i32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if nix::sys::signal::kill(Pid::from_raw(pid), None).is_err() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(POLL);
        }
    }

    /// Runs `script` under `sh` with a short deadline; the script writes the pid of what it
    /// leaves running to `$1`. Answers the error and that pid.
    fn leave_behind(script: &str, timeout: Duration) -> (SystemError, i32) {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let started = Instant::now();
        let err = run(&Invocation::new("/bin/sh", timeout)
            .arg("-c")
            .arg(script)
            .arg("sh")
            .arg(&pid_file))
        .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
        let pid = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        (err, pid)
    }

    #[test]
    fn a_timeout_kills_what_the_program_started() {
        // The shell waits for the `sleep` it started. Killing the shell alone would answer
        // just as fast — the readers are given up on — and leave `sleep` running for half a
        // minute holding the pipes, which is why this asks the process table and not a clock.
        let (err, sleeper) = leave_behind(
            "sleep 30 & echo $! > \"$1\"; wait",
            Duration::from_millis(300),
        );
        assert!(matches!(err, SystemError::TimedOut { .. }), "got {err:?}");
        assert!(
            gone(sleeper),
            "the process the program started is still running"
        );
    }

    #[test]
    fn a_process_left_holding_the_output_is_stopped_at_the_deadline() {
        // The shell exits at once; the `sleep` it left in the background holds stdout, so
        // the output is not complete until the deadline stops it.
        let (err, sleeper) = leave_behind(
            "sleep 30 & echo $! > \"$1\"; echo done",
            Duration::from_millis(300),
        );
        assert!(matches!(err, SystemError::TimedOut { .. }), "got {err:?}");
        assert!(gone(sleeper), "the process left behind is still running");
    }

    #[test]
    fn output_past_the_cap_is_an_error() {
        let invocation =
            Invocation::new("/usr/bin/head", TIMEOUT).args(["-c", "4096", "/dev/zero"]);
        let err = run_capped(&invocation, 1024).unwrap_err();
        assert!(
            matches!(err, SystemError::OutputTooLarge { .. }),
            "got {err:?}"
        );
        // At the cap exactly is still whole.
        let output = run_capped(&invocation, 4096).unwrap();
        assert_eq!(output.stdout.len(), 4096);
    }

    #[test]
    fn a_relative_program_is_rejected_before_anything_runs() {
        let err = run(&Invocation::new("echo", TIMEOUT)).unwrap_err();
        assert!(matches!(err, SystemError::Rejected { .. }), "got {err:?}");
    }

    #[test]
    fn a_program_in_another_form_is_rejected() {
        let err = run(&Invocation::new("/bin/../bin/echo", TIMEOUT)).unwrap_err();
        assert!(matches!(err, SystemError::Rejected { .. }), "got {err:?}");
    }

    #[test]
    fn a_relative_working_directory_is_rejected() {
        let err = run(&Invocation::new("/bin/pwd", TIMEOUT).cwd("tmp")).unwrap_err();
        assert!(matches!(err, SystemError::Rejected { .. }), "got {err:?}");
    }

    #[test]
    fn a_missing_program_is_a_spawn_error() {
        let err = run(&Invocation::new("/nonexistent/tool", TIMEOUT)).unwrap_err();
        assert!(matches!(err, SystemError::Spawn { .. }), "got {err:?}");
    }

    /// An executable file named `name` in `dir`.
    fn tool(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn path_of(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    #[test]
    fn locate_answers_the_first_executable_in_order() {
        let (first, second) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        tool(first.path(), "probe");
        tool(second.path(), "probe");
        let path = path_of(&[first.path(), second.path()]);
        assert_eq!(
            locate_in("probe", Some(&path), &[]),
            Some(first.path().join("probe"))
        );
    }

    #[test]
    fn locate_skips_a_file_that_cannot_run() {
        let (first, second) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        fs::write(first.path().join("probe"), b"not a program").unwrap();
        tool(second.path(), "probe");
        let path = path_of(&[first.path(), second.path()]);
        assert_eq!(
            locate_in("probe", Some(&path), &[]),
            Some(second.path().join("probe"))
        );
    }

    #[test]
    fn locate_skips_a_directory_of_that_name() {
        let (first, second) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        fs::create_dir(first.path().join("probe")).unwrap();
        fs::set_permissions(
            first.path().join("probe"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        tool(second.path(), "probe");
        let path = path_of(&[first.path(), second.path()]);
        assert_eq!(
            locate_in("probe", Some(&path), &[]),
            Some(second.path().join("probe"))
        );
    }

    #[test]
    fn locate_ignores_relative_entries_of_path() {
        let dir = tempfile::tempdir().unwrap();
        tool(dir.path(), "probe");
        let relative = OsString::from("relative/bin");
        assert_eq!(locate_in("probe", Some(&relative), &[]), None);
    }

    #[test]
    fn locate_falls_back_to_the_known_folders() {
        let dir = tempfile::tempdir().unwrap();
        tool(dir.path(), "probe");
        let known = dir.path().to_str().unwrap();
        assert_eq!(
            locate_in("probe", None, &[known]),
            Some(dir.path().join("probe"))
        );
    }

    #[test]
    fn locate_answers_in_normal_form() {
        let dir = tempfile::tempdir().unwrap();
        tool(dir.path(), "probe");
        let mut spelled = dir.path().as_os_str().to_owned();
        spelled.push("/./");
        let found = locate_in("probe", Some(&spelled), &[]).unwrap();
        assert_eq!(found, dir.path().join("probe"));
        let normal: PathBuf = found.components().collect();
        assert_eq!(normal.as_os_str(), found.as_os_str());
    }

    #[test]
    fn locate_takes_a_name_and_never_a_path() {
        let dir = tempfile::tempdir().unwrap();
        tool(dir.path(), "probe");
        let path = path_of(&[dir.path()]);
        for name in ["", ".", "..", "bin/probe", "/bin/sh", "../probe"] {
            assert_eq!(locate_in(name, Some(&path), &[]), None, "{name:?}");
        }
    }

    #[test]
    fn the_real_system_finds_a_tool_every_machine_has() {
        let found = RealSystem.locate("sh").expect("sh is everywhere");
        assert!(found.is_absolute());
        assert!(found.ends_with("sh"));
    }
}
