//! Git adapter: mainline, branches, worktrees, and commits.
//!
//! Every operation runs the `git` executable directly with an argv, never through a shell, so
//! the Human Operator's own identity and configuration apply.
//!
//! Fetching is the only remote access that needs no approval. Anything that writes to a remote
//! takes a [`RemoteApproval`] (ADR-0006).

mod approval;
mod branch;
mod command;
mod commit;
mod mainline;
mod repo;
mod worktree;

pub use approval::RemoteApproval;
pub use branch::SessionBranch;
pub use command::GitError;
pub use commit::{CommitId, CommitMessage};
pub use mainline::Mainline;
pub use repo::{Fetch, Repo};
pub use worktree::Worktree;
