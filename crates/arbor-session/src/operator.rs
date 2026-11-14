//! The Human Operator, as the Autopilot sees it: whoever decides at each Checkpoint and answers a
//! Tier's clarifying questions.

use std::future::Future;
use std::pin::Pin;

use arbor_core::{Plan, Revision, Tier, WorkUnit};

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
}

pub trait Operator {
    fn decide<'a>(&'a mut self, checkpoint: &'a Checkpoint) -> Pending<'a, Decision>;

    /// The answers to a Tier's clarifying questions, or `None` when nobody can answer them, which
    /// ends the run.
    fn answer<'a>(&'a mut self, tier: Tier, questions: &'a str) -> Pending<'a, Option<String>>;
}

/// Approves every Checkpoint as it is and answers no questions, for runs nobody is watching, such
/// as the end-to-end tests and `xtask eval`.
pub struct ApproveEverything;

impl Operator for ApproveEverything {
    fn decide<'a>(&'a mut self, _checkpoint: &'a Checkpoint) -> Pending<'a, Decision> {
        Box::pin(async { Decision::Approve })
    }

    fn answer<'a>(&'a mut self, _tier: Tier, _questions: &'a str) -> Pending<'a, Option<String>> {
        Box::pin(async { None })
    }
}
