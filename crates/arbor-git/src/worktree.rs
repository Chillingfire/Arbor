use std::path::{Path, PathBuf};

use crate::approval::RemoteApproval;
use crate::branch::PREFIX;
use crate::command::{Git, GitError, git, git_succeeds};
use crate::commit::{CommitId, CommitMessage};

/// A Session's dedicated git worktree, checked out on its Session branch.
#[derive(Debug, Clone)]
pub struct Worktree {
    path: PathBuf,
    branch: String,
}

impl Worktree {
    /// Opens an existing Session worktree, e.g. to resume a Session.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, GitError> {
        let path = PathBuf::from(git(path.as_ref(), &["rev-parse", "--show-toplevel"])?);
        let branch = git(&path, &["symbolic-ref", "--quiet", "--short", "HEAD"]).map_err(|_| {
            GitError::Refused(format!(
                "the worktree {} is not on a branch",
                path.display()
            ))
        })?;
        Ok(Self { path, branch })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The Session branch, e.g. `arbor/add-oauth-login`.
    pub fn branch(&self) -> &str {
        &self.branch
    }

    /// The commit the worktree is on.
    pub fn head(&self) -> Result<CommitId, GitError> {
        git(&self.path, &["rev-parse", "HEAD"]).map(CommitId::from)
    }

    /// Commits every change in the worktree, including new and deleted files, with the
    /// operator's git identity. Returns `None` without committing if nothing changed.
    pub fn commit(&self, message: &CommitMessage) -> Result<Option<CommitId>, GitError> {
        let dir = &self.path;
        git(dir, &["add", "--all"])?;
        if git_succeeds(dir, &["diff", "--cached", "--quiet"])? {
            return Ok(None);
        }
        // `whitespace` keeps description lines that start with `#`, which the default would drop.
        let text = message.text();
        Git::new(
            dir,
            &["commit", "--quiet", "--cleanup=whitespace", "--file=-"],
        )
        .input(text.as_bytes())
        .bytes()?;
        self.head().map(Some)
    }

    /// Resets the worktree to a recorded commit, for Rewind and Replan: the Session branch moves
    /// back to it and uncommitted changes and untracked files are discarded. Ignored files,
    /// such as build output, are kept.
    pub fn reset_to(&self, commit: &CommitId) -> Result<(), GitError> {
        let hash = commit.as_str();
        let is_hash = (7..=64).contains(&hash.len()) && hash.bytes().all(|b| b.is_ascii_hexdigit());
        let target = format!("{hash}^{{commit}}");
        if !is_hash || !git_succeeds(&self.path, &["rev-parse", "--verify", "--quiet", &target])? {
            return Err(GitError::Refused(format!(
                "{hash} is not a commit in this repository"
            )));
        }
        git(&self.path, &["reset", "--hard", "--quiet", hash])?;
        git(&self.path, &["clean", "-d", "--force", "--quiet"])?;
        Ok(())
    }

    /// Pushes the Session branch, and nothing else, to a branch of the same name on `origin`.
    /// This writes to the remote, so it uses up an approval the operator just gave (ADR-0006).
    pub fn push(&self, approval: RemoteApproval) -> Result<(), GitError> {
        let _approval = approval;
        if !self.branch.starts_with(PREFIX) {
            return Err(GitError::Refused(format!(
                "only Session branches can be pushed, not {}",
                self.branch
            )));
        }
        let refspec = format!("refs/heads/{0}:refs/heads/{0}", self.branch);
        git(&self.path, &["push", "--quiet", "origin", &refspec])?;
        Ok(())
    }
}
