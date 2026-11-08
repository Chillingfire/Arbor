/// The Human Operator's approval, given just now, for one remote-writing action (ADR-0006).
///
/// Every operation in this crate that writes to a remote takes one by value, so it is used up
/// by that one action. It can't be cloned, copied, defaulted, or built by any means other than
/// [`RemoteApproval::granted_by_operator`]:
///
/// ```compile_fail
/// let approval = arbor_git::RemoteApproval { _private: () };
/// ```
///
/// ```compile_fail
/// fn copy(approval: &arbor_git::RemoteApproval) -> arbor_git::RemoteApproval {
///     approval.clone()
/// }
/// ```
///
/// A push can't happen without one:
///
/// ```compile_fail
/// fn push(worktree: &arbor_git::Worktree) {
///     let _ = worktree.push();
/// }
/// ```
///
/// And one approval can't cover two pushes:
///
/// ```compile_fail
/// fn push_twice(worktree: &arbor_git::Worktree, approval: arbor_git::RemoteApproval) {
///     let _ = worktree.push(approval);
///     let _ = worktree.push(approval);
/// }
/// ```
#[derive(Debug)]
pub struct RemoteApproval {
    _private: (),
}

impl RemoteApproval {
    /// Call this only at the moment the Human Operator approves this specific action, and
    /// never store the result for later.
    pub fn granted_by_operator() -> Self {
        Self { _private: () }
    }
}
