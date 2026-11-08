use crate::commit::CommitMessage;

/// What to do when the operator's checkout has uncommitted changes at Session start. The
/// Session worktree starts from the mainline, so none of these carry the changes into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirtyTreeAction {
    /// Stash the changes, including untracked files, and proceed.
    Stash,
    /// Commit the changes on the checkout's current branch and proceed. Refused when that
    /// branch is the local mainline, which Arbor never modifies.
    Commit(CommitMessage),
    /// Leave the changes where they are and proceed.
    ProceedWithout,
    /// Don't start the Session.
    Abort,
}

/// Whether the Session may start after a [`DirtyTreeAction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirtyTreeOutcome {
    Proceed,
    Abort,
}
