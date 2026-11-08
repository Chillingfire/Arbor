//! Git adapter: mainline, branches, worktrees, commits, and reading GitHub issues.
//!
//! Every operation runs the `git` (or `gh`) executable directly with an argv, never through a
//! shell, so the Human Operator's own identity and configuration apply.
//!
//! Fetching and reading a GitHub issue are the only remote access that needs no approval. Anything that writes to a remote
//! takes a [`RemoteApproval`] (ADR-0006).

mod approval;
mod branch;
mod changes;
mod command;
mod commit;
mod github;
mod mainline;
mod repo;
mod worktree;

pub use approval::RemoteApproval;
pub use branch::SessionBranch;
pub use command::GitError;
pub use commit::{CommitId, CommitMessage};
pub use github::{GhCli, GhOutput, GhRunner, IssueError, issue_request};
pub use mainline::Mainline;
pub use repo::{Fetch, Repo};
pub use worktree::Worktree;
