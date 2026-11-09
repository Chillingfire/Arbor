use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::{WorkUnit, WorkUnitId};

/// Revisions allowed for one failure before the Human Operator's Checkpoint, after which the work
/// goes to the Planner (grilling log, Round 2 decision 4 and Round 4 decision 4). A failed Work
/// Unit and a failed Task Check each count their own.
pub const MAX_REVISIONS: usize = 2;

/// What sent work back to the Scaffolder for a Revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RevisionCause {
    /// The full Unit Check failed after the Task's last Work Unit passed.
    TaskChecksFailed { output: String },
    /// A Writer used up its retries on a Work Unit, or sent a Missing Response.
    WorkUnitFailed(WorkUnitFailure),
}

impl RevisionCause {
    /// The Work Unit that failed, when the failure is a single Work Unit's.
    pub fn work_unit(&self) -> Option<WorkUnitId> {
        match self {
            Self::TaskChecksFailed { .. } => None,
            Self::WorkUnitFailed(failure) => Some(failure.id),
        }
    }
}

/// How the Writer failed a Work Unit: every failed attempt, and the Missing Response that ended
/// the run early, if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkUnitFailure {
    pub id: WorkUnitId,
    /// Oldest first.
    pub attempts: Vec<FailedAttempt>,
    /// What the Writer's Missing Response said its Context Pack lacked, one item per entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_response: Option<Vec<String>>,
}

/// One Writer attempt that failed: its reply and the error it got back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedAttempt {
    pub reply: String,
    /// Why the reply was rejected, or the Checks' output.
    pub error: String,
}

/// The Scaffolder's rework of failed Work Units, submitted with `submit_revision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Revision {
    /// Why the work failed, and what this Revision changes so it passes.
    pub diagnosis: String,
    /// The Work Units to write again, in order, each with its Region marked again. A revised
    /// Work Unit keeps its ID; a new one, e.g. from a split, takes an ID no Work Unit has used.
    /// Empty when `shared_change` is set.
    pub work_units: Vec<WorkUnit>,
    /// Set, with no Work Units, when the fix needs a signature or type that other Work Units
    /// depend on: it names that change, and the failure goes to the Planner instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_change: Option<String>,
}

/// One Revision and the failure it answered, kept so a later Revision, and a Replan, see what was
/// tried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionRound {
    pub cause: RevisionCause,
    pub revision: Revision,
    /// Why it was rejected, e.g. it only reworded the spec. It still counts toward
    /// [`MAX_REVISIONS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection: Option<String>,
}

impl RevisionRound {
    /// Whether this round answered the same failure as `cause`: the same Task Check, or the same
    /// Work Unit, including Work Units it split off.
    fn answers(&self, cause: &RevisionCause) -> bool {
        match (&self.cause, cause) {
            (RevisionCause::TaskChecksFailed { .. }, RevisionCause::TaskChecksFailed { .. }) => {
                true
            }
            (RevisionCause::WorkUnitFailed(earlier), RevisionCause::WorkUnitFailed(failure)) => {
                earlier.id == failure.id
                    || self
                        .revision
                        .work_units
                        .iter()
                        .any(|unit| unit.id == failure.id)
            }
            _ => false,
        }
    }
}

/// The Revisions in `rounds` that count toward [`MAX_REVISIONS`] for `cause`.
pub(crate) fn revisions_for(rounds: &[RevisionRound], cause: &RevisionCause) -> usize {
    rounds.iter().filter(|round| round.answers(cause)).count()
}
