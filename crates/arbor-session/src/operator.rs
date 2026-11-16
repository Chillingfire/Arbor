//! The Human Operator, as the Autopilot sees it: whoever decides at each Checkpoint and answers a
//! Tier's clarifying questions.

use std::future::Future;
use std::pin::Pin;

use arbor_core::{LeftoverMarker, Plan, Replan, Revision, RevisionCause, Tier, Usd, WorkUnit};

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
        /// The titles of the Plan's Tasks after this one, which may depend on it if it is
        /// skipped.
        later_tasks: Vec<String>,
        /// The stronger model that can write a failed Work Unit again, or `None` after a failed
        /// Task Check, which has no single Work Unit to write.
        stronger_writer: Option<StrongerWriterOffer>,
    },
    /// The Tasks the Planner would put in place of the failed one. Decided like a Plan.
    Replan {
        task: usize,
        replan: Replan,
    },
    /// Markers are left in the branch's commits. The Human Operator approves them as they are,
    /// e.g. fixtures that only look like Markers, or removes them by hand.
    Markers {
        remaining: Vec<LeftoverMarker>,
    },
    /// The Session has spent its Budget Cap, so no new model call starts until the Human Operator
    /// raises the cap or stops.
    Budget {
        cap: Usd,
        spent: Usd,
    },
}

/// What a retry of the failed Work Unit with a stronger model would use and cost, shown before
/// the Human Operator chooses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrongerWriterOffer {
    pub model: String,
    /// Estimated from the Writer prompt with its Failure Brief and a typical reply.
    pub cost_per_attempt: Usd,
    pub max_attempts: usize,
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
    /// At an Escalation or Marker Checkpoint: the Human Operator fixed the work by hand in the
    /// worktree, so Arbor checks the fix and commits it.
    Fixed,
    /// At an Escalation Checkpoint only: send the failed Task to the Planner.
    Replan,
    /// At an Escalation Checkpoint only: run `tier` again with this hint. The Writer needs a
    /// failed Work Unit, and the Planner the Task's unused Replan.
    RetryWithHint {
        tier: Tier,
        hint: String,
    },
    /// At an Escalation Checkpoint only: write the failed Work Unit again with the offered
    /// stronger model.
    RetryWithStrongerWriter,
    /// At an Escalation Checkpoint only: drop the Task's work and go on with the next Task.
    Skip,
    /// At an Escalation Checkpoint only: end the Session for good, keeping the branch.
    Abort,
    /// End the run here, keeping the branch and worktree as they are. At the Budget Checkpoint
    /// this stops the Session for good.
    Stop,
    /// At the Budget Checkpoint only: carry on with this cap, which must be above what the
    /// Session has spent, or with no cap.
    SetBudgetCap(Option<Usd>),
}

pub trait Operator {
    fn decide<'a>(&'a mut self, checkpoint: &'a Checkpoint) -> Pending<'a, Decision>;

    /// The answers to a Tier's clarifying questions, or `None` when nobody can answer them, which
    /// ends the run.
    fn answer<'a>(&'a mut self, tier: Tier, questions: &'a str) -> Pending<'a, Option<String>>;
}

/// Approves every Checkpoint as it is and answers no questions, for runs nobody is watching, such
/// as the end-to-end tests and `xtask eval`. It stops at an Escalation, leftover Markers and the
/// Budget Cap, which only the Human Operator can settle.
pub struct ApproveEverything;

impl Operator for ApproveEverything {
    fn decide<'a>(&'a mut self, checkpoint: &'a Checkpoint) -> Pending<'a, Decision> {
        let decision = match checkpoint {
            Checkpoint::Escalation { .. }
            | Checkpoint::Markers { .. }
            | Checkpoint::Budget { .. } => Decision::Stop,
            _ => Decision::Approve,
        };
        Box::pin(async move { decision })
    }

    fn answer<'a>(&'a mut self, _tier: Tier, _questions: &'a str) -> Pending<'a, Option<String>> {
        Box::pin(async { None })
    }
}
