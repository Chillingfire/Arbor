use serde::{Deserialize, Serialize};

use crate::cost::{Usage, Usd};
use crate::revision::{Revision, WorkUnitFailure};
use crate::types::{LeftoverMarker, Plan, Request, Tier, WorkUnit, WorkUnitId};

/// Something that happened in a Session. The event log is the Session's source of truth (ADR-0004).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    SessionStarted {
        request: Request,
    },
    PlanSubmitted {
        plan: Plan,
    },
    /// The Human Operator approved the Plan. `task_order` lists the Plan's Task indices to run,
    /// in order, so Tasks can be dropped or reordered.
    PlanApproved {
        task_order: Vec<usize>,
    },
    /// The Human Operator rejected the open Checkpoint. The same Tier runs again with the feedback.
    CheckpointRejected {
        feedback: String,
    },
    WorkUnitsSubmitted {
        work_units: Vec<WorkUnit>,
    },
    ScaffoldChecksPassed,
    ScaffoldApproved,
    /// The Writer's code was spliced and its Work Unit's Checks passed.
    WorkUnitPassed {
        id: WorkUnitId,
    },
    /// The Writer failed its Work Unit: it used up its retries, or sent a Missing Response.
    WorkUnitFailed {
        failure: WorkUnitFailure,
    },
    WorkUnitCommitted {
        id: WorkUnitId,
        commit: String,
    },
    TaskChecksPassed,
    /// The Task's full Unit Check failed after its last Work Unit.
    TaskChecksFailed {
        output: String,
    },
    /// The Scaffolder's Revision passed its submit checks. Its Work Units are written next, with
    /// no Checkpoint in between.
    RevisionSubmitted {
        revision: Revision,
    },
    /// The Human Operator fixed the problem at the open Checkpoint by hand, so Arbor checks again.
    /// `commit` is the fix Arbor committed, or `None` when there was nothing to commit.
    OperatorFixed {
        commit: Option<String>,
    },
    /// The end-of-session Marker check ran over the branch's commits. With Markers `remaining`,
    /// the Session waits at the Marker Checkpoint.
    MarkersChecked {
        remaining: Vec<LeftoverMarker>,
    },
    /// The Human Operator approved the remaining Markers as they are, e.g. because they are
    /// fixtures or docs that only look like Markers. The Session finishes.
    MarkersApproved,
    /// One LLM call finished and its provider reported this usage. Recorded for every call,
    /// including calls redone after a resume, since each one was paid for. Written after the
    /// call's outcome events, so a call that reaches the Budget Cap still lands its result.
    UsageRecorded {
        tier: Tier,
        usage: Usage,
    },
    /// The Human Operator set the Session's Budget Cap, or removed it with `None`. At a budget
    /// Checkpoint, this is the "raise the cap" choice.
    BudgetCapSet {
        #[serde(rename = "cap_nano_usd")]
        cap: Option<Usd>,
    },
    /// The Human Operator chose to stop at a budget Checkpoint.
    SessionStopped,
}
