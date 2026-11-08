//! Git adapter: mainline, branches, worktrees, and commits.
//!
//! Every operation runs the `git` executable directly with an argv, never through a shell, so
//! the Human Operator's own identity and configuration apply.

mod command;
mod mainline;
mod repo;

pub use command::GitError;
pub use mainline::Mainline;
pub use repo::{Fetch, Repo};
