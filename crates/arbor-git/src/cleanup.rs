//! Removing what a Session leaves behind: its worktree when it is pruned or deleted, and its
//! branch only when the Human Operator deletes the Session.

use std::path::Path;

use crate::branch::PREFIX;
use crate::command::{GitError, git};
use crate::commit::CommitId;
use crate::repo::Repo;
use crate::worktree::Worktree;

impl Repo {
    /// Removes the worktree at `path` and git's record of it. Check
    /// [`Worktree::has_uncommitted_changes`] first: anything uncommitted in it is lost. A worktree
    /// whose folder is already gone only has its record dropped.
    pub fn remove_worktree(&self, path: &Path) -> Result<(), GitError> {
        if path.exists() {
            let path_arg = path.to_str().ok_or_else(|| {
                GitError::Refused(format!("{} is not a UTF-8 path", path.display()))
            })?;
            git(self.root(), &["worktree", "remove", "--force", path_arg])?;
        }
        git(self.root(), &["worktree", "prune"])?;
        Ok(())
    }

    /// Deletes a Session branch, merged or not. Only `arbor/` branches can be deleted.
    pub fn delete_session_branch(&self, branch: &str) -> Result<(), GitError> {
        if !branch.starts_with(PREFIX) {
            return Err(GitError::Refused(format!(
                "only Session branches can be deleted, not {branch}"
            )));
        }
        self.check_branch_name(branch)?;
        git(self.root(), &["branch", "--quiet", "-D", branch])?;
        Ok(())
    }

    pub fn branch_exists(&self, branch: &str) -> Result<bool, GitError> {
        self.check_branch_name(branch)?;
        self.commit_exists(&format!("refs/heads/{branch}"))
    }
}

impl Worktree {
    /// Whether any file differs from `HEAD`, untracked files included. Ignored files don't count.
    pub fn has_uncommitted_changes(&self) -> Result<bool, GitError> {
        let status = git(
            self.path(),
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )?;
        Ok(!status.is_empty())
    }

    /// Moves the branch back to `head` with the files as they are in `edits`, left uncommitted:
    /// how the worktree stood at a Checkpoint that reviewed edits `edits` later committed.
    pub fn reset_with_uncommitted(
        &self,
        head: &CommitId,
        edits: &CommitId,
    ) -> Result<(), GitError> {
        self.reset_to(edits)?;
        let head_hash = self.verified(head)?;
        git(self.path(), &["reset", "--mixed", "--quiet", head_hash])?;
        Ok(())
    }
}
