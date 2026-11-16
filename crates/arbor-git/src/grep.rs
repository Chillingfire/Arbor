//! Searching the text of a repository's tracked files, as the Marker scan needs (ADR-0002).

use crate::command::{Git, GitError, git};
use crate::repo::Repo;

/// Which content of the repository a search reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeSource {
    /// The tracked files as they are on disk, including uncommitted edits.
    WorkingTree,
    /// What is staged for the next commit.
    Staged,
    /// The tree of a commit, e.g. `HEAD` or a branch tip.
    Revision(String),
}

/// One line a search found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMatch {
    /// Relative to the repository's top level, with `/` separators.
    pub file: String,
    /// 1-based.
    pub line: usize,
    pub text: String,
}

impl Repo {
    /// The lines of tracked text files in `source` that match `pattern`, a POSIX extended
    /// regular expression. Binary files are skipped.
    pub fn grep(&self, pattern: &str, source: &TreeSource) -> Result<Vec<LineMatch>, GitError> {
        let tree;
        // The operator's grep config could add columns or quote paths, so it is pinned here.
        let mut grep_args = vec![
            "-c",
            "grep.column=false",
            "-c",
            "grep.lineNumber=true",
            "-c",
            "core.quotePath=false",
            "grep",
            "-z",
            "-n",
            "-I",
            "--no-color",
            "--full-name",
            "--no-recurse-submodules",
            "-E",
            "-e",
            pattern,
        ];
        let path_prefix = match source {
            TreeSource::WorkingTree => String::new(),
            TreeSource::Staged => {
                grep_args.push("--cached");
                String::new()
            }
            TreeSource::Revision(revision) => {
                tree = self.tree_of(revision)?;
                grep_args.push(&tree);
                format!("{tree}:")
            }
        };
        grep_args.push("--");
        let grep_output = Git::new(self.root(), &grep_args).run()?;
        match grep_output.status.code() {
            Some(0) => {}
            Some(1) => return Ok(Vec::new()),
            _ => {
                return Err(GitError::Failed {
                    command: format!("git {}", grep_args.join(" ")),
                    stderr: String::from_utf8_lossy(&grep_output.stderr)
                        .trim()
                        .to_owned(),
                });
            }
        }
        let stdout = String::from_utf8_lossy(&grep_output.stdout);
        Ok(stdout
            .split('\n')
            .filter_map(|grep_line| parse_match(grep_line, &path_prefix))
            .collect())
    }

    /// The tree hash of `revision`, so a name the operator typed is never passed to git as an
    /// option.
    fn tree_of(&self, revision: &str) -> Result<String, GitError> {
        let not_a_commit = || GitError::Refused(format!("{revision} is not a commit"));
        if revision.is_empty() || revision.starts_with('-') {
            return Err(not_a_commit());
        }
        git(
            self.root(),
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &format!("{revision}^{{tree}}"),
            ],
        )
        .map_err(|_| not_a_commit())
    }
}

/// Parses one `path\0line\0text` line of `git grep -z -n` output.
fn parse_match(grep_line: &str, path_prefix: &str) -> Option<LineMatch> {
    let mut fields = grep_line.splitn(3, '\0');
    let path = fields.next()?;
    let line = fields.next()?.parse().ok()?;
    let text = fields.next()?;
    Some(LineMatch {
        file: path.strip_prefix(path_prefix).unwrap_or(path).to_owned(),
        line,
        text: text.trim_end_matches('\r').to_owned(),
    })
}
