use std::fmt;

/// The branch a Session's branch starts from: normally `origin/<name>`, or the local branch
/// when the repository has no remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mainline {
    name: String,
    remote: bool,
}

impl Mainline {
    pub(crate) fn remote(name: String) -> Self {
        Self { name, remote: true }
    }

    pub(crate) fn local(name: String) -> Self {
        Self {
            name,
            remote: false,
        }
    }

    /// The branch name without any remote, e.g. `main`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// False when the repository has no remote and the local branch is used instead. The
    /// Human Operator should be told when that happens.
    pub fn is_remote(&self) -> bool {
        self.remote
    }

    /// The revision a Session branch is created from, e.g. `origin/main`.
    pub fn start_point(&self) -> String {
        if self.remote {
            format!("origin/{}", self.name)
        } else {
            self.name.clone()
        }
    }

    /// The full ref name of [`Self::start_point`].
    pub(crate) fn start_ref(&self) -> String {
        if self.remote {
            format!("refs/remotes/origin/{}", self.name)
        } else {
            format!("refs/heads/{}", self.name)
        }
    }
}

impl fmt::Display for Mainline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.remote {
            write!(f, "origin/{}", self.name)
        } else {
            write!(f, "{} (local; the repository has no remote)", self.name)
        }
    }
}
