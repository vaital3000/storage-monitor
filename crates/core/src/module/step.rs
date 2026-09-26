//! What an action does, as a module plans it and the core runs it (ADR 0008).
//!
//! A module never deletes and never runs anything that changes something: it answers
//! [`super::Module::plan`] with these, and the cleanup engine executes them through the
//! guards. Nothing here derives `Deserialize`, on purpose: a step comes out of a module's
//! plan in the same process that runs it, and never from anywhere else.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::scan::NodeKind;

/// One thing an action does, in its order among the others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Moves the path to the Trash or deletes it for good, whichever the batch's mode says —
    /// through the same port and the same guards as a row of the Explorer.
    Delete(Target),
    /// Runs a program, the same way in both modes.
    Run { command: Command, effect: Effect },
}

/// A path a step deletes, and what the module saw there. The kind is checked against the
/// disk in the preview and again right before the step: a file where the module saw a
/// folder is not the thing the user was shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    pub kind: NodeKind,
}

impl Target {
    pub fn new(path: impl Into<PathBuf>, kind: NodeKind) -> Self {
        Self {
            path: path.into(),
            kind,
        }
    }
}

/// What a `Run` step changes, as the module declares it. The core reads it for two things:
/// whether the step needs the guards, and whether the Trash can undo it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Bookkeeping nobody could miss: `git worktree prune` once the folder is gone.
    Housekeeping,
    /// Destroys something outside the filesystem the guards see: an image, a simulator.
    Destroys,
    /// Deletes this path, by other means than the port — judged by the guards like a
    /// [`Step::Delete`], and never undone by the Trash.
    Removes(Target),
}

/// A program to run, by the name of its tool. The core locates the tool when the step runs
/// — not when it is planned, since planning cannot look — and a tool that has gone by then
/// fails its entry with a sentence that names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub tool: String,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    /// Added to the environment the app inherited.
    pub env: Vec<(OsString, OsString)>,
}

impl Command {
    pub fn new(tool: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
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

    /// The tool and its arguments as text, for the record: one string per word, lossy where
    /// a word is not UTF-8.
    pub fn argv(&self) -> Vec<String> {
        std::iter::once(self.tool.clone())
            .chain(
                self.args
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned()),
            )
            .collect()
    }

    /// The command as a person reads it; see [`command_line`].
    pub fn line(&self) -> String {
        command_line(&self.argv())
    }
}

/// Words joined into one line, quoted so that a path with a space in it reads as one
/// argument: `rm '/Users/me/Application Support/x'`.
///
/// For reading only — the preview, the report, a failure message. Nothing parses it back:
/// the process receives the argv, and a line that looks like it could be pasted into a
/// shell is not a promise that it can be.
pub fn command_line(words: &[String]) -> String {
    words
        .iter()
        .map(|word| quote(word))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A word bare when it could not be misread, and in single quotes otherwise, with a quote
/// inside written as `'\''`.
fn quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./:=@%+,-".contains(c));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn a_plain_command_line_is_left_bare() {
        assert_eq!(
            command_line(&words(&["git", "-C", "/h/repo", "worktree", "prune"])),
            "git -C /h/repo worktree prune"
        );
    }

    #[test]
    fn a_word_with_a_space_reads_as_one_argument() {
        assert_eq!(
            command_line(&words(&["rm", "/h/Application Support/x.object"])),
            "rm '/h/Application Support/x.object'"
        );
    }

    #[test]
    fn a_quote_inside_a_word_is_escaped() {
        assert_eq!(command_line(&words(&["echo", "it's"])), r"echo 'it'\''s'");
    }

    #[test]
    fn an_empty_word_is_still_a_word() {
        assert_eq!(command_line(&words(&["tool", ""])), "tool ''");
    }

    #[test]
    fn a_word_a_shell_would_expand_is_quoted() {
        assert_eq!(
            command_line(&words(&["echo", "$HOME", "*", "a;b"])),
            "echo '$HOME' '*' 'a;b'"
        );
    }

    #[test]
    fn a_command_knows_its_argv_and_its_line() {
        let command = Command::new("rm").arg("/h/a b");
        assert_eq!(command.argv(), words(&["rm", "/h/a b"]));
        assert_eq!(command.line(), "rm '/h/a b'");
    }
}
