//! Commits each state of a Task that the Session accepts, so `HEAD` is always the state a
//! rejected Revision is discarded back to. These commits hold Markers and Placeholders, like the
//! Writer's, and are squashed at merge time.

use arbor_core::{Event, Task};
use arbor_git::{CommitMessage, GitError, Worktree};

/// Commits the Skeleton approved at the scaffold Checkpoint and returns `SkeletonCommitted`.
pub fn commit_skeleton(worktree: &Worktree, task: &Task) -> Result<Event, GitError> {
    let commit = commit_all(worktree, &skeleton_subject(task))?;
    Ok(Event::SkeletonCommitted { commit })
}

/// Like every commit, it doesn't mention Arbor (ADR-0009).
fn skeleton_subject(task: &Task) -> String {
    format!("{} (structure and tests)", title(task))
}

fn title(task: &Task) -> &str {
    task.title.trim().trim_end_matches('.')
}

fn commit_all(worktree: &Worktree, subject: &str) -> Result<Option<String>, GitError> {
    let commit = worktree.commit(&CommitMessage::new(subject)?)?;
    Ok(commit.map(|hash| hash.as_str().to_owned()))
}
