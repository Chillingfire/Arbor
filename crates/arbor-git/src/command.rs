use std::fmt;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};

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

/// One git invocation. It runs `git` with an argv, never through a shell.
pub(crate) struct Git<'a> {
    dir: &'a Path,
    git_args: &'a [&'a str],
    index: Option<&'a Path>,
    input: Option<&'a [u8]>,
}

impl<'a> Git<'a> {
    pub(crate) fn new(dir: &'a Path, git_args: &'a [&'a str]) -> Self {
        Self {
            dir,
            git_args,
            index: None,
            input: None,
        }
    }

    /// Uses `index` instead of the repository's own index, which is left untouched.
    pub(crate) fn index(mut self, index: &'a Path) -> Self {
        self.index = Some(index);
        self
    }

    pub(crate) fn input(mut self, input: &'a [u8]) -> Self {
        self.input = Some(input);
        self
    }

    /// Runs git and returns its raw stdout, or an error if it failed.
    pub(crate) fn bytes(self) -> Result<Vec<u8>, GitError> {
        let git_args = self.git_args;
        let git_output = self.run()?;
        if !git_output.status.success() {
            return Err(GitError::Failed {
                command: format!("git {}", git_args.join(" ")),
                stderr: String::from_utf8_lossy(&git_output.stderr)
                    .trim()
                    .to_owned(),
            });
        }
        Ok(git_output.stdout)
    }

    /// Runs git and returns its stdout with trailing whitespace removed.
    pub(crate) fn text(self) -> Result<String, GitError> {
        let stdout = self.bytes()?;
        Ok(String::from_utf8_lossy(&stdout).trim_end().to_owned())
    }

    /// Runs git and reports only whether it succeeded, for queries whose failure means "no".
    pub(crate) fn succeeds(self) -> Result<bool, GitError> {
        Ok(self.run()?.status.success())
    }

    fn run(self) -> Result<Output, GitError> {
        let mut command = Command::new("git");
        command
            .args(self.git_args)
            .current_dir(self.dir)
            // Arbor runs under a TUI, so git must never wait for a password on the terminal.
            .env("GIT_TERMINAL_PROMPT", "0")
            // Queries such as `git status` must not rewrite the operator's index.
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(if self.input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(index) = self.index {
            command.env("GIT_INDEX_FILE", index);
        }
        let mut child = command.spawn().map_err(GitError::Spawn)?;
        let Some(input) = self.input else {
            return child.wait_with_output().map_err(GitError::Spawn);
        };
        let mut stdin = child.stdin.take().expect("stdin is piped");
        // Write from another thread so a large input can't deadlock against git's output.
        std::thread::scope(|scope| {
            let writer = scope.spawn(move || stdin.write_all(input));
            let git_output = child.wait_with_output().map_err(GitError::Spawn)?;
            match writer.join().expect("the stdin writer does not panic") {
                // git may exit before reading everything, e.g. on an error it reports itself.
                Err(err) if err.kind() != io::ErrorKind::BrokenPipe => Err(GitError::Spawn(err)),
                _ => Ok(git_output),
            }
        })
    }
}

/// Runs git in `dir` and returns its stdout with trailing whitespace removed.
pub(crate) fn git(dir: &Path, git_args: &[&str]) -> Result<String, GitError> {
    Git::new(dir, git_args).text()
}

/// Runs git and reports only whether it succeeded, for queries whose failure means "no".
pub(crate) fn git_succeeds(dir: &Path, git_args: &[&str]) -> Result<bool, GitError> {
    Git::new(dir, git_args).succeeds()
}
