use serde::{Deserialize, Serialize};

use crate::cost::{Usage, Usd};
use crate::revision::{Revision, WorkUnitFailure};
use crate::types::{
    ClarificationMode, LeftoverMarker, Plan, Replan, Request, Tier, WorkUnit, WorkUnitId,
};

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
    /// in order, so Tasks can be dropped or reordered. At the Replan's Checkpoint it indexes the
    /// Replan's Tasks instead.
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
    /// The approved Skeleton was committed, or `None` when the Scaffolder changed nothing.
    SkeletonCommitted {
        commit: Option<String>,
    },
    /// The Writer's code was spliced and its Work Unit's Checks passed. `model` is the model that
    /// wrote it, so the log shows which Work Units a stronger model wrote.
    WorkUnitPassed {
        id: WorkUnitId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
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
    /// Arbor rejected the Revision, e.g. because it only reworded the spec. It counts toward the
    /// Revision limit.
    RevisionRejected {
        revision: Revision,
        reason: String,
    },
    /// The Human Operator switched the Clarification Mode mid-Session. Tier runs from here on use
    /// it instead of the configured one.
    ClarificationModeSet {
        mode: ClarificationMode,
    },
    /// Turns the Checkpoint after each Revision on or off. Off by default.
    RevisionCheckpointsSet {
        enabled: bool,
    },
    RevisionApproved,
    /// The accepted Revision's edits were committed, or `None` when it changed no file.
    RevisionCommitted {
        commit: Option<String>,
    },
    /// The worktree is back at `HEAD`, without the edits of the Revision that wasn't accepted.
    RevisionDiscarded,
    /// At the escalation Checkpoint, the Human Operator sends the failed Task to the Planner.
    ReplanRequested,
    /// At the Escalation Checkpoint, the Human Operator runs `tier` again with a hint: the Writer
    /// writes the failed Work Unit again, the Scaffolder makes one more Revision, or the Planner
    /// makes the Task's Replan.
    RetryWithHint {
        tier: Tier,
        hint: String,
    },
    /// At the Escalation Checkpoint, the Human Operator has the failed Work Unit written again in
    /// the Writer role by `model`, with the same Context Pack and a Failure Brief.
    StrongerWriterRequested {
        model: String,
    },
    /// At the Escalation Checkpoint, the Human Operator drops the failed Task: its work is reset
    /// and the Session goes on with the next Task.
    TaskSkipped,
    /// At the Escalation Checkpoint, the Human Operator ends the Session for good. The branch and
    /// worktree are kept as they are.
    SessionAborted,
    ReplanSubmitted {
        replan: Replan,
    },
    /// The worktree was reset to where the Task started, after its Replan was approved.
    TaskReset,
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
