use std::fmt;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};

/// A git operation that failed or that Arbor refused to perform.
#[derive(Debug)]
pub enum GitError {
    /// The `git` executable couldn't be started.
    Spawn(io::Error),
    /// git ran and reported an error.
    Failed { command: String, stderr: String },
    /// Arbor refused before running git, e.g. an unknown mainline or an empty commit subject.
    Refused(String),
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(err) => write!(f, "could not run git: {err}"),
            Self::Failed { command, stderr } => write!(f, "{command} failed: {stderr}"),
            Self::Refused(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for GitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn(err) => Some(err),
            _ => None,
        }
    }
}

/// Runs git in `dir` with `args` as its argv, never through a shell, and returns its stdout
/// with trailing whitespace removed.
pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String, GitError> {
    git_with_input(dir, args, None)
}

/// Like [`git`], writing `input` to git's stdin.
pub(crate) fn git_with_input(
    dir: &Path,
    args: &[&str],
    input: Option<&str>,
) -> Result<String, GitError> {
    let output = run(dir, args, input)?;
    if !output.status.success() {
        return Err(GitError::Failed {
            command: format!("git {}", args.join(" ")),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_owned())
}

/// Runs git and reports only whether it succeeded, for queries whose failure means "no".
pub(crate) fn git_succeeds(dir: &Path, args: &[&str]) -> Result<bool, GitError> {
    Ok(run(dir, args, None)?.status.success())
}

fn run(dir: &Path, args: &[&str], input: Option<&str>) -> Result<std::process::Output, GitError> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(dir)
        // Arbor runs under a TUI, so git must never wait for a password on the terminal.
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(GitError::Spawn)?;
    if let Some(input) = input {
        let mut stdin = child.stdin.take().expect("stdin is piped");
        stdin.write_all(input.as_bytes()).map_err(GitError::Spawn)?;
    }
    child.wait_with_output().map_err(GitError::Spawn)
}
