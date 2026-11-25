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

/// Commits the edits of a Revision that was accepted, by review or at its Checkpoint, and returns
/// `RevisionCommitted`.
pub fn commit_revision(worktree: &Worktree, task: &Task) -> Result<Event, GitError> {
    let commit = commit_all(worktree, &revision_subject(task))?;
    Ok(Event::RevisionCommitted { commit })
}

/// Undoes the edits of a Revision that wasn't accepted and returns `RevisionDiscarded`. `HEAD`
/// holds the Task's last accepted state, since the Skeleton and every accepted Revision are
/// committed.
pub fn discard_revision(worktree: &Worktree) -> Result<Event, GitError> {
    worktree.reset_to(&worktree.head()?)?;
    Ok(Event::RevisionDiscarded)
}

/// Like every commit, it doesn't mention Arbor (ADR-0009).
pub fn skeleton_subject(task: &Task) -> String {
    format!("{} (structure and tests)", title(task))
}

pub fn revision_subject(task: &Task) -> String {
    format!("{} (revised structure and tests)", title(task))
}

fn title(task: &Task) -> &str {
    task.title.trim().trim_end_matches('.')
}

fn commit_all(worktree: &Worktree, subject: &str) -> Result<Option<String>, GitError> {
    let commit = worktree.commit(&CommitMessage::new(subject)?)?;
    Ok(commit.map(|hash| hash.as_str().to_owned()))
}
