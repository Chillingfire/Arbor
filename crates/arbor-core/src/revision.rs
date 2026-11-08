use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::WorkUnit;

/// Revisions allowed for one failure before the Human Operator's Checkpoint, after which the work
/// goes to the Planner (grilling log, Round 2 decision 4 and Round 4 decision 4).
pub const MAX_REVISIONS: usize = 2;

/// What sent work back to the Scaffolder for a Revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RevisionCause {
    /// The full Unit Check failed after the Task's last Work Unit passed.
    TaskChecksFailed { output: String },
}

/// The Scaffolder's rework of failed Work Units, submitted with `submit_revision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Revision {
    /// Why the work failed, and what this Revision changes so it passes.
    pub diagnosis: String,
    /// The Work Units to write again, in order, each with its Region marked again. A revised
    /// Work Unit keeps its ID; a new one, e.g. from a split, takes an ID no Work Unit has used.
    pub work_units: Vec<WorkUnit>,
}

/// One Revision and the failure it answered, kept so a later Revision sees what was tried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionRound {
    pub cause: RevisionCause,
    pub revision: Revision,
}
