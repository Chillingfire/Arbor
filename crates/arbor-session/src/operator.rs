//! The Human Operator, as the Autopilot sees it: whoever decides at each Checkpoint and answers a
//! Tier's clarifying questions.

use std::future::Future;
use std::pin::Pin;

use arbor_core::{Plan, Replan, Revision, RevisionCause, Tier, WorkUnit};

/// A future the Operator resolves once the Human Operator has decided, e.g. in a TUI modal.
pub type Pending<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// A Checkpoint the Session waits at, with what the Human Operator reviews there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checkpoint {
    Plan {
        plan: Plan,
    },
    Scaffold {
        task: usize,
        work_units: Vec<WorkUnit>,
    },
    Revision {
        task: usize,
        revision: Revision,
    },
    /// The Scaffolder couldn't make the Task pass. The Human Operator fixes it by hand, sends it
    /// to the Planner, or stops here.
    Escalation {
        task: usize,
        cause: RevisionCause,
        /// Whether the Task may still go to the Planner: only one Replan per Task.
        may_replan: bool,
        /// The Checks' output when the Human Operator's last fix didn't pass them.
        failed_fix: Option<String>,
    },
    /// The Tasks the Planner would put in place of the failed one. Decided like a Plan.
    Replan {
        task: usize,
        replan: Replan,
    },
}

/// The Human Operator's choice at a [`Checkpoint`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Approve,
    /// At a Plan Checkpoint only: run these Tasks, by index into the Plan, in this order. Tasks
    /// left out are dropped.
    ApproveTasks(Vec<usize>),
    /// The same Tier runs again with this feedback.
    Reject(String),
    /// At an Escalation Checkpoint only: the Human Operator fixed the work by hand in the
    /// worktree, so Arbor checks the fix and commits it.
    Fixed,
    /// At an Escalation Checkpoint only: send the failed Task to the Planner.
    Replan,
    /// End the run here, keeping the branch and worktree as they are.
    Stop,
}

pub trait Operator {
    fn decide<'a>(&'a mut self, checkpoint: &'a Checkpoint) -> Pending<'a, Decision>;

    /// The answers to a Tier's clarifying questions, or `None` when nobody can answer them, which
    /// ends the run.
    fn answer<'a>(&'a mut self, tier: Tier, questions: &'a str) -> Pending<'a, Option<String>>;
}

/// Approves every Checkpoint as it is and answers no questions, for runs nobody is watching, such
/// as the end-to-end tests and `xtask eval`. It stops at an Escalation, which has nothing to
/// approve.
pub struct ApproveEverything;

impl Operator for ApproveEverything {
    fn decide<'a>(&'a mut self, checkpoint: &'a Checkpoint) -> Pending<'a, Decision> {
        let decision = match checkpoint {
            Checkpoint::Escalation { .. } => Decision::Stop,
            _ => Decision::Approve,
        };
        Box::pin(async move { decision })
    }

    fn answer<'a>(&'a mut self, _tier: Tier, _questions: &'a str) -> Pending<'a, Option<String>> {
        Box::pin(async { None })
    }
}
