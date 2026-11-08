use std::path::{Path, PathBuf};

use crate::command::{GitError, git};
use crate::commit::CommitId;

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
}
