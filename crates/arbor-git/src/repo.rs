use std::path::{Path, PathBuf};

use crate::branch::SessionBranch;
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
    /// worktree at `path`. The local mainline and the operator's checkout are left alone, and
    /// the branch tracks nothing, so it can't be pushed onto the mainline by accident. Fails if
    /// the branch already exists.
    pub fn create_worktree(
        &self,
        mainline: &Mainline,
        branch: &SessionBranch,
        path: &Path,
    ) -> Result<Worktree, GitError> {
        self.check_branch_name(branch.as_str())?;
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
                &mainline.start_ref(),
            ],
        )?;
        Worktree::open(path)
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
