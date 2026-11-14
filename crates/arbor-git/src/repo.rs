use std::path::{Path, PathBuf};

use crate::branch::SessionBranch;
use crate::changes;
use crate::command::{GitError, git, git_succeeds};
use crate::mainline::Mainline;
use crate::worktree::Worktree;

/// The local mainline names tried, in order, when there is no remote and none was chosen.
const LOCAL_MAINLINE_GUESSES: &[&str] = &["main", "master"];

/// The Human Operator's repository: the checkout Arbor was started in.
#[derive(Debug, Clone)]
pub struct Repo {
    root: PathBuf,
}

/// What [`Repo::fetch`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fetch {
    /// `origin` was fetched.
    Origin,
    /// The repository has no `origin`, so the local mainline is used.
    NoRemote,
}

impl Repo {
    /// Opens the repository containing `path`, which may be any directory inside it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, GitError> {
        let root = git(path.as_ref(), &["rev-parse", "--show-toplevel"])?;
        Ok(Self {
            root: PathBuf::from(root),
        })
    }

    /// The top-level directory of the checkout.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The git directory every worktree of the repository shares, e.g. for `info/exclude`. From
    /// a linked worktree this is the main checkout's `.git`, not the worktree's own.
    pub fn common_dir(&self) -> Result<PathBuf, GitError> {
        let common_dir = git(
            &self.root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        Ok(PathBuf::from(common_dir))
    }

    /// Fetches `origin`. This is read-only for the remote, so it needs no approval (ADR-0006).
    /// It updates only remote-tracking branches, never the local mainline.
    pub fn fetch(&self) -> Result<Fetch, GitError> {
        if !self.has_origin()? {
            return Ok(Fetch::NoRemote);
        }
        git(&self.root, &["fetch", "--quiet", "--prune", "origin"])?;
        Ok(Fetch::Origin)
    }

    /// The branches the Human Operator may pick as mainline at Session start: `origin`'s
    /// branches, or the local ones when there is no remote. Call [`Self::fetch`] first.
    pub fn branches(&self) -> Result<Vec<String>, GitError> {
        let (prefix, strip) = if self.has_origin()? {
            ("refs/remotes/origin", 3)
        } else {
            ("refs/heads", 2)
        };
        let format = format!("--format=%(refname:lstrip={strip})");
        let listed = git(&self.root, &["for-each-ref", &format, prefix])?;
        Ok(listed
            .lines()
            .filter(|name| *name != "HEAD")
            .map(str::to_owned)
            .collect())
    }

    /// Resolves the mainline. `chosen` is the branch picked at Session start or set in
    /// `arbor.toml`; without it, the mainline is `origin/HEAD`. With no remote it is the local
    /// `main` or `master`. Call [`Self::fetch`] first.
    pub fn mainline(&self, chosen: Option<&str>) -> Result<Mainline, GitError> {
        let remote = self.has_origin()?;
        let name = match chosen {
            Some(name) => {
                self.check_branch_name(name)?;
                name.to_owned()
            }
            None if remote => self.origin_head()?,
            None => self.guess_local_mainline()?,
        };
        let mainline = if remote {
            Mainline::remote(name)
        } else {
            Mainline::local(name)
        };
        if !self.commit_exists(&mainline.start_ref())? {
            return Err(GitError::Refused(format!(
                "the mainline {} does not exist",
                mainline.start_point()
            )));
        }
        Ok(mainline)
    }

    /// Creates the Session branch from the mainline's start point and checks it out in a new
    /// worktree at `path`. Any uncommitted changes in the operator's checkout are left out of
    /// it. The local mainline and the operator's checkout are left alone, and the branch tracks
    /// nothing, so it can't be pushed onto the mainline by accident. Fails if the branch
    /// already exists.
    pub fn create_worktree(
        &self,
        mainline: &Mainline,
        branch: &SessionBranch,
        path: &Path,
    ) -> Result<Worktree, GitError> {
        self.check_branch_name(branch.as_str())?;
        self.add_worktree(branch, path, &mainline.start_ref())
    }

    /// Like [`Self::create_worktree`], but brings the operator's uncommitted changes in:
    /// modified, staged, deleted, renamed, and untracked files that aren't ignored. They become
    /// the Session branch's first commit, made with the operator's identity, which Rewind can
    /// reset to. With no changes, the branch starts at the mainline.
    ///
    /// The operator's checkout, index, and stashes are not touched. The changes are taken
    /// relative to the checkout's `HEAD`, which may be another branch or behind `origin`, and
    /// applied onto the mainline. If they don't apply cleanly, this fails without creating the
    /// branch or the worktree, and the operator should leave the changes out or abort.
    pub fn create_worktree_with_changes(
        &self,
        mainline: &Mainline,
        branch: &SessionBranch,
        path: &Path,
    ) -> Result<Worktree, GitError> {
        self.check_branch_name(branch.as_str())?;
        let start = git(
            &self.root,
            &[
                "rev-parse",
                "--verify",
                &format!("{}^{{commit}}", mainline.start_ref()),
            ],
        )?;
        let commit = changes::commit_onto(&self.root, &start, &mainline.start_point())?;
        self.add_worktree(branch, path, commit.as_deref().unwrap_or(&start))
    }

    fn add_worktree(
        &self,
        branch: &SessionBranch,
        path: &Path,
        start: &str,
    ) -> Result<Worktree, GitError> {
        let path_arg = path
            .to_str()
            .ok_or_else(|| GitError::Refused(format!("{} is not a UTF-8 path", path.display())))?;
        git(
            &self.root,
            &[
                "worktree",
                "add",
                "--quiet",
                "--no-track",
                "-b",
                branch.as_str(),
                path_arg,
                start,
            ],
        )?;
        Worktree::open(path)
    }

    /// The paths with uncommitted changes in the operator's checkout, including untracked
    /// files. Empty when the tree is clean. Ask the operator whether to bring them into the
    /// Session ([`Self::create_worktree_with_changes`]), leave them out
    /// ([`Self::create_worktree`]), or abort.
    pub fn uncommitted_changes(&self) -> Result<Vec<String>, GitError> {
        let status = git(
            &self.root,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        let mut paths = Vec::new();
        let mut entries = status.split('\0').filter(|entry| !entry.is_empty());
        while let Some(entry) = entries.next() {
            let (code, path) = entry.split_at(entry.len().min(3));
            if code.contains(['R', 'C']) {
                // A rename or copy is followed by the path it came from.
                entries.next();
            }
            paths.push(path.to_owned());
        }
        Ok(paths)
    }

    /// Refuses anything that isn't a plain branch name, such as `main~1` or `-x`.
    pub(crate) fn check_branch_name(&self, name: &str) -> Result<(), GitError> {
        let full = format!("refs/heads/{name}");
        if name.starts_with('-') || !git_succeeds(&self.root, &["check-ref-format", &full])? {
            return Err(GitError::Refused(format!(
                "{name} is not a valid branch name"
            )));
        }
        Ok(())
    }

    fn has_origin(&self) -> Result<bool, GitError> {
        let remotes = git(&self.root, &["remote"])?;
        Ok(remotes.lines().any(|remote| remote == "origin"))
    }

    /// `origin`'s default branch: from the local `origin/HEAD` if a clone set it, otherwise by
    /// asking `origin`.
    fn origin_head(&self) -> Result<String, GitError> {
        let local = git(
            &self.root,
            &[
                "symbolic-ref",
                "--quiet",
                "--short",
                "refs/remotes/origin/HEAD",
            ],
        );
        if let Ok(head) = local
            && let Some(name) = head.strip_prefix("origin/")
        {
            return Ok(name.to_owned());
        }
        let listed = git(&self.root, &["ls-remote", "--symref", "origin", "HEAD"])?;
        listed
            .lines()
            .find_map(|line| {
                let target = line.strip_prefix("ref: ")?.strip_suffix("\tHEAD")?;
                target.strip_prefix("refs/heads/")
            })
            .map(str::to_owned)
            .ok_or_else(|| {
                GitError::Refused(
                    "origin has no default branch; choose a mainline or set it in arbor.toml"
                        .to_owned(),
                )
            })
    }

    fn guess_local_mainline(&self) -> Result<String, GitError> {
        for name in LOCAL_MAINLINE_GUESSES {
            if self.commit_exists(&format!("refs/heads/{name}"))? {
                return Ok((*name).to_owned());
            }
        }
        Err(GitError::Refused(
            "the repository has no remote and no main or master branch; choose a mainline or \
             set it in arbor.toml"
                .to_owned(),
        ))
    }

    pub(crate) fn commit_exists(&self, reference: &str) -> Result<bool, GitError> {
        git_succeeds(
            &self.root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{reference}^{{commit}}"),
            ],
        )
    }
}
