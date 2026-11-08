use std::path::{Path, PathBuf};

use crate::approval::RemoteApproval;
use crate::branch::PREFIX;
use crate::command::{Git, GitError, git, git_succeeds};
use crate::commit::{CommitId, CommitMessage};
use crate::mainline::Mainline;

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
        let hash = self.verified(commit)?;
        git(&self.path, &["reset", "--hard", "--quiet", hash])?;
        git(&self.path, &["clean", "-d", "--force", "--quiet"])?;
        Ok(())
    }

    /// The commit `commit` was made on top of, e.g. where a Task started given its first commit.
    pub fn parent_of(&self, commit: &CommitId) -> Result<CommitId, GitError> {
        let parent = format!("{}^", self.verified(commit)?);
        git(&self.path, &["rev-parse", "--verify", "--quiet", &parent]).map(CommitId::from)
    }

    /// Where the Session branch left the mainline: the merge base of `HEAD` and its start point.
    pub fn fork_point(&self, mainline: &Mainline) -> Result<CommitId, GitError> {
        git(&self.path, &["merge-base", "HEAD", &mainline.start_ref()]).map(CommitId::from)
    }

    /// A unified diff from `base` to the files in the worktree, committed or not. Untracked files
    /// are left out.
    pub fn diff_from(&self, base: &CommitId) -> Result<String, GitError> {
        let hash = self.verified(base)?;
        // Every option the operator's diff config could change is pinned, so the text is the
        // same on every machine.
        let diff = Git::new(
            &self.path,
            &[
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--no-relative",
                "--no-renames",
                "--unified=3",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                hash,
                "--",
            ],
        )
        .bytes()?;
        Ok(String::from_utf8_lossy(&diff).into_owned())
    }

    /// The commit's hash, once it is known to name a commit here. The hash may come from the
    /// event log, so it is never passed to git as an option or a range.
    fn verified<'a>(&self, commit: &'a CommitId) -> Result<&'a str, GitError> {
        let hash = commit.as_str();
        let is_hash = (7..=64).contains(&hash.len()) && hash.bytes().all(|b| b.is_ascii_hexdigit());
        let target = format!("{hash}^{{commit}}");
        if !is_hash || !git_succeeds(&self.path, &["rev-parse", "--verify", "--quiet", &target])? {
            return Err(GitError::Refused(format!(
                "{hash} is not a commit in this repository"
            )));
        }
        Ok(hash)
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
