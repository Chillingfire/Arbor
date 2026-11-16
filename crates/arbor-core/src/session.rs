use std::collections::BTreeSet;
use std::fmt;

use crate::cost::{UsageTotals, Usd};
use crate::event::Event;
use crate::revision::{
    MAX_REVISIONS, Revision, RevisionCause, RevisionRound, WorkUnitFailure, revisions_for,
};
use crate::types::{
    ClarificationMode, CommitOrigin, LeftoverMarker, Plan, Replan, Request, SessionCommit, Tier,
    WorkUnit, WorkUnitId,
};

/// What the outer crates should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    RunPlanner {
        feedback: Option<String>,
    },
    ShowPlanCheckpoint,
    RunScaffolder {
        task: usize,
        feedback: Option<String>,
    },
    RunScaffoldChecks {
        task: usize,
    },
    ShowScaffoldCheckpoint {
        task: usize,
    },
    /// Commit the approved Skeleton, so a rejected Revision can be discarded by resetting to
    /// `HEAD`.
    CommitSkeleton {
        task: usize,
    },
    /// Call the Writer, splice its code, and run the Work Unit's Checks.
    RunWriter {
        id: WorkUnitId,
    },
    CommitWorkUnit {
        id: WorkUnitId,
    },
    RunTaskChecks {
        task: usize,
    },
    /// Run a Scaffolder Revision for `cause`. It is given the Task diff and the Task's earlier
    /// [`Session::revisions`].
    RunRevision {
        task: usize,
        cause: RevisionCause,
    },
    /// Only with Revision Checkpoints turned on (`RevisionCheckpointsSet`).
    ShowRevisionCheckpoint {
        task: usize,
        revision: Revision,
    },
    /// Commit the accepted Revision's edits, so a later rejected one is discarded back to them.
    CommitRevision {
        task: usize,
    },
    /// Put the worktree back to `HEAD`, undoing the edits of a Revision that wasn't accepted.
    DiscardRevision {
        task: usize,
    },
    /// The Revisions are used up, or a Revision needs a shared signature changed, so the Human
    /// Operator decides before anything goes to the Planner.
    ShowEscalationCheckpoint {
        task: usize,
        cause: RevisionCause,
    },
    /// Run the Planner on the failed Task with its failure chain: `cause` and the Task's
    /// [`Session::revisions`].
    RunReplan {
        task: usize,
        cause: RevisionCause,
        feedback: Option<String>,
    },
    /// The plan Checkpoint for the Tasks that would replace the failed one.
    ShowReplanCheckpoint {
        task: usize,
        replan: Replan,
    },
    /// Reset the worktree to the Task's start, discarding its commits and uncommitted changes.
    ResetTask {
        task: usize,
    },
    /// Every Task is done: look for Markers left in the branch diff before finishing.
    CheckMarkers,
    /// Markers remain, so the Session can't finish until the Human Operator removes them or
    /// approves them as they are.
    ShowMarkerCheckpoint {
        remaining: Vec<LeftoverMarker>,
    },
    /// The Budget Cap was reached: start no new LLM call, and ask the Human Operator to raise the
    /// cap or stop. A call already in flight is left to finish and its usage recorded. Once the
    /// cap is raised, the phase's own effect comes back; if it is the `Run*` effect of a run that
    /// was cut off between calls, the runner continues that run rather than restarting it.
    ShowBudgetCheckpoint,
    EndSession,
}

/// Where a Session is. `task` indexes the approved Plan's Tasks. `unit` indexes the Work Units
/// being written in this pass: all of the Task's, or after a Revision only the revised ones.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    NotStarted,
    Planning {
        feedback: Option<String>,
    },
    PlanCheckpoint,
    Scaffolding {
        task: usize,
        feedback: Option<String>,
    },
    ScaffoldChecking {
        task: usize,
    },
    ScaffoldCheckpoint {
        task: usize,
    },
    CommittingSkeleton {
        task: usize,
    },
    Writing {
        task: usize,
        unit: usize,
    },
    Committing {
        task: usize,
        unit: usize,
    },
    TaskChecking {
        task: usize,
    },
    Revising {
        task: usize,
        cause: RevisionCause,
    },
    RevisionCheckpoint {
        task: usize,
        cause: RevisionCause,
        revision: Revision,
    },
    CommittingRevision {
        task: usize,
    },
    DiscardingRevision {
        task: usize,
        cause: RevisionCause,
    },
    EscalationCheckpoint {
        task: usize,
        cause: RevisionCause,
    },
    Replanning {
        task: usize,
        cause: RevisionCause,
        feedback: Option<String>,
    },
    ReplanCheckpoint {
        task: usize,
        cause: RevisionCause,
        replan: Replan,
    },
    ResettingTask {
        task: usize,
    },
    /// The Human Operator skipped the Task; its work is reset before the next Task starts.
    SkippingTask {
        task: usize,
    },
    CheckingMarkers,
    MarkerCheckpoint {
        remaining: Vec<LeftoverMarker>,
    },
    Done,
    /// The Human Operator stopped the Session at a budget Checkpoint.
    Stopped,
    /// The Human Operator aborted the Session at the Escalation Checkpoint.
    Aborted,
}

/// An event that doesn't fit the Session's current state. The state is left unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    Unexpected {
        phase: Box<Phase>,
        event: Box<Event>,
    },
    /// Empty, out of range, or naming a Task twice.
    InvalidTaskOrder {
        task_order: Vec<usize>,
    },
    DuplicateWorkUnit {
        id: WorkUnitId,
    },
    /// A Revision gives a new Work Unit an ID that an earlier one already used.
    WorkUnitIdTaken {
        id: WorkUnitId,
    },
    /// A Revision of one failed Work Unit may revise only that Work Unit and add new ones.
    OutsideRevisionScope {
        id: WorkUnitId,
    },
    /// The event names a Work Unit other than the one in progress.
    WrongWorkUnit {
        expected: WorkUnitId,
        got: WorkUnitId,
    },
    /// The Task already came from a Replan; each Task gets one, then only the Human Operator.
    ReplanUsed {
        task: usize,
    },
    /// The Escalation is for a failed Task Check, so there is no failed Work Unit to write again.
    NoFailedWorkUnit,
    /// A Budget Cap at or below what the Session has already spent, which would pause at once.
    BudgetCapReached {
        cap: Usd,
        spent: Usd,
    },
    /// The budget Checkpoint is open, so only `BudgetCapSet`, `SessionStopped` and
    /// `UsageRecorded` are accepted until it is resolved.
    AtBudgetCheckpoint {
        event: Box<Event>,
    },
    /// A `Rewound` event needs the events before it, so only [`Session::replay`] applies it.
    RewindNeedsLog,
    /// A Rewind that names no earlier Checkpoint in the log.
    InvalidRewind {
        event_count: usize,
    },
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unexpected { phase, event } => {
                write!(f, "unexpected event {event:?} in phase {phase:?}")
            }
            Self::InvalidTaskOrder { task_order } => {
                write!(f, "invalid task order {task_order:?}")
            }
            Self::DuplicateWorkUnit { id } => write!(f, "duplicate work unit {id}"),
            Self::WorkUnitIdTaken { id } => {
                write!(f, "{id} was already used by an earlier work unit")
            }
            Self::OutsideRevisionScope { id } => {
                write!(
                    f,
                    "{id} is outside the Revision's scope of one failed Work Unit"
                )
            }
            Self::WrongWorkUnit { expected, got } => {
                write!(f, "event for {got} while {expected} is in progress")
            }
            Self::ReplanUsed { task } => {
                write!(f, "Task {} has already been replanned once", task + 1)
            }
            Self::NoFailedWorkUnit => {
                write!(f, "the Escalation has no failed Work Unit to write again")
            }
            Self::BudgetCapReached { cap, spent } => write!(
                f,
                "budget cap ${:.4} is not above the ${:.4} already spent",
                cap.dollars(),
                spent.dollars()
            ),
            Self::AtBudgetCheckpoint { event } => {
                write!(f, "event {event:?} refused: the budget checkpoint is open")
            }
            Self::RewindNeedsLog => f.write_str("a Rewind is applied by replaying the event log"),
            Self::InvalidRewind { event_count } => write!(
                f,
                "a Rewind to event {event_count} names no earlier Checkpoint in the log"
            ),
        }
    }
}

impl std::error::Error for ApplyError {}

/// A Session's state, built only by applying events.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Session {
    phase: Phase,
    request: Option<Request>,
    /// The submitted Plan, replaced by the approved one on approval.
    plan: Option<Plan>,
    /// The current Task's Work Units, as last revised.
    work_units: Vec<WorkUnit>,
    /// The Work Units this pass writes, in order.
    write_queue: Vec<WorkUnitId>,
    /// The highest Work Unit ID submitted in the Session.
    highest_id: Option<WorkUnitId>,
    commits: Vec<SessionCommit>,
    /// Where the current Task's commits start in `commits`.
    task_commits_start: usize,
    /// The current Task's Revisions.
    revisions: Vec<RevisionRound>,
    /// The Work Units whose Writer has sent its one Missing Response.
    missing_responses: BTreeSet<WorkUnitId>,
    revision_checkpoints: bool,
    /// Per approved Task: whether it came from a Replan, which uses up the Replan it would get.
    replanned: Vec<bool>,
    usage: UsageTotals,
    budget_cap: Option<Usd>,
    /// The Human Operator's hint from the current Task's last Escalation.
    hint: Option<OperatorHint>,
    stronger_writer: Option<StrongerWriter>,
    /// Indices of the approved Plan's Tasks the Human Operator skipped.
    skipped_tasks: Vec<usize>,
    /// Set once the Human Operator switches the mode; until then the configured one holds.
    clarification_mode: Option<ClarificationMode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OperatorHint {
    tier: Tier,
    /// The failed Work Unit, which is the only one a Writer hint is for.
    work_unit: Option<WorkUnitId>,
    text: String,
}

/// A failed Work Unit the Human Operator had written again by a stronger model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrongerWriter {
    pub work_unit: WorkUnitId,
    pub model: String,
    /// The failure that sent it to the Escalation Checkpoint, which its Failure Brief sums up.
    pub failure: WorkUnitFailure,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuilds a Session from its event log, Rewinds included. Call `pending` afterwards to redo
    /// in-flight work.
    pub fn replay(events: &[Event]) -> Result<Self, ApplyError> {
        let mut session = Self::new();
        for (index, event) in events.iter().enumerate() {
            match *event {
                Event::Rewound { event_count } => {
                    session = session.rewound(&events[..index], event_count)?;
                }
                _ => {
                    session.apply(event)?;
                }
            }
        }
        Ok(session)
    }

    /// The Session as it stood at the Checkpoint after the first `event_count` of
    /// `earlier_events`. It keeps what this one has spent, its settings, and the Work Unit IDs it
    /// has used, so the log never names two Work Units the same.
    pub fn rewound(
        &self,
        earlier_events: &[Event],
        event_count: usize,
    ) -> Result<Self, ApplyError> {
        let invalid = ApplyError::InvalidRewind { event_count };
        let restored_events = earlier_events.get(..event_count).ok_or(invalid.clone())?;
        let mut restored = Self::replay(restored_events)?;
        if !is_rewind_target(&restored.phase) {
            return Err(invalid);
        }
        restored.usage = self.usage.clone();
        restored.budget_cap = self.budget_cap;
        restored.revision_checkpoints = self.revision_checkpoints;
        restored.clarification_mode = self.clarification_mode;
        restored.highest_id = self.highest_id;
        Ok(restored)
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn request(&self) -> Option<&Request> {
        self.request.as_ref()
    }

    pub fn plan(&self) -> Option<&Plan> {
        self.plan.as_ref()
    }

    pub fn work_units(&self) -> &[WorkUnit] {
        &self.work_units
    }

    pub fn commits(&self) -> &[SessionCommit] {
        &self.commits
    }

    /// The commits made since the current Task started, oldest first. The first one's parent is
    /// where the Task diff starts.
    pub fn task_commits(&self) -> &[SessionCommit] {
        &self.commits[self.task_commits_start..]
    }

    /// The current Task's Revisions so far, oldest first.
    pub fn revisions(&self) -> &[RevisionRound] {
        &self.revisions
    }

    /// Each Task gets one Replan; the Tasks a Replan produces get none.
    pub fn may_replan(&self, task: usize) -> bool {
        !self.replanned.get(task).copied().unwrap_or(true)
    }

    /// A Writer may send one Missing Response per Work Unit; a later one costs a retry.
    pub fn may_send_missing_response(&self, id: WorkUnitId) -> bool {
        !self.missing_responses.contains(&id)
    }

    /// The Human Operator's hint for `tier`, from the current Task's last Escalation. A Writer
    /// hint is only for the Work Unit that failed.
    pub fn hint_for(&self, tier: Tier, work_unit: Option<WorkUnitId>) -> Option<&str> {
        self.hint
            .as_ref()
            .filter(|hint| {
                hint.tier == tier && (tier != Tier::Writer || hint.work_unit == work_unit)
            })
            .map(|hint| hint.text.as_str())
    }

    /// Set when the Human Operator chose a stronger model for this Work Unit's Writer.
    pub fn stronger_writer(&self, work_unit: WorkUnitId) -> Option<&StrongerWriter> {
        self.stronger_writer
            .as_ref()
            .filter(|stronger| stronger.work_unit == work_unit)
    }

    pub fn skipped_tasks(&self) -> &[usize] {
        &self.skipped_tasks
    }

    /// The Clarification Mode the Human Operator switched to, or `None` to use the configured one.
    pub fn clarification_mode(&self) -> Option<ClarificationMode> {
        self.clarification_mode
    }

    /// The lowest ID no Work Unit in the Session has used.
    pub fn next_work_unit_id(&self) -> WorkUnitId {
        self.highest_id
            .map_or(WorkUnitId(1), |id| WorkUnitId(id.0 + 1))
    }

    /// Tokens and cost per Tier, from every recorded LLM call.
    pub fn usage(&self) -> &UsageTotals {
        &self.usage
    }

    pub fn budget_cap(&self) -> Option<Usd> {
        self.budget_cap
    }

    /// The Budget Cap is reached, so nothing moves the Session on until the Human Operator raises
    /// the cap or stops: `apply` refuses every event but `BudgetCapSet`, `SessionStopped` and
    /// `UsageRecorded`.
    ///
    /// Ordering rule: the runner records a call's outcome events (e.g. `PlanSubmitted`, or
    /// `WorkUnitPassed` once the Writer's code is spliced and checked) *before* that call's
    /// `UsageRecorded`. So the call that reaches the cap still lands its result, and the
    /// Checkpoint opens right after it.
    pub fn is_at_budget_checkpoint(&self) -> bool {
        is_live(&self.phase)
            && self
                .budget_cap
                .is_some_and(|cap| self.usage.total().cost >= cap)
    }

    /// Finished with every Task done or skipped. A stopped or aborted Session is not done.
    pub fn is_done(&self) -> bool {
        self.phase == Phase::Done
    }

    /// Applies one event and returns the effects that should follow it.
    pub fn apply(&mut self, event: &Event) -> Result<Vec<Effect>, ApplyError> {
        let unexpected = || ApplyError::Unexpected {
            phase: Box::new(self.phase.clone()),
            event: Box::new(event.clone()),
        };
        let resolves_or_records = matches!(
            event,
            Event::BudgetCapSet { .. }
                | Event::SessionStopped
                | Event::UsageRecorded { .. }
                | Event::RunEnded { .. }
        );
        if self.is_at_budget_checkpoint() && !resolves_or_records {
            return Err(ApplyError::AtBudgetCheckpoint {
                event: Box::new(event.clone()),
            });
        }
        let next = match (&self.phase, event) {
            (Phase::NotStarted, Event::SessionStarted { request }) => {
                self.request = Some(request.clone());
                Phase::Planning { feedback: None }
            }
            (Phase::Planning { .. }, Event::PlanSubmitted { plan }) => {
                self.plan = Some(plan.clone());
                Phase::PlanCheckpoint
            }
            (Phase::PlanCheckpoint, Event::PlanApproved { task_order }) => {
                let plan = self.plan.as_mut().ok_or_else(unexpected)?;
                if !is_valid_order(task_order, plan.tasks.len()) {
                    return Err(ApplyError::InvalidTaskOrder {
                        task_order: task_order.clone(),
                    });
                }
                plan.tasks = task_order
                    .iter()
                    .map(|&task_index| plan.tasks[task_index].clone())
                    .collect();
                self.replanned = vec![false; task_order.len()];
                Phase::Scaffolding {
                    task: 0,
                    feedback: None,
                }
            }
            (Phase::PlanCheckpoint, Event::PlanEdited { plan }) => {
                self.plan = Some(plan.clone());
                Phase::PlanCheckpoint
            }
            (Phase::PlanCheckpoint, Event::CheckpointRejected { feedback }) => Phase::Planning {
                feedback: Some(feedback.clone()),
            },
            (&Phase::ScaffoldCheckpoint { task }, Event::CheckpointRejected { feedback }) => {
                Phase::Scaffolding {
                    task,
                    feedback: Some(feedback.clone()),
                }
            }
            (&Phase::Scaffolding { task, .. }, Event::WorkUnitsSubmitted { work_units }) => {
                if let Some(id) = first_duplicate(work_units) {
                    return Err(ApplyError::DuplicateWorkUnit { id });
                }
                self.work_units = work_units.clone();
                self.note_ids(work_units);
                Phase::ScaffoldChecking { task }
            }
            (&Phase::ScaffoldChecking { task }, Event::ScaffoldChecksPassed) => {
                Phase::ScaffoldCheckpoint { task }
            }
            (&Phase::ScaffoldCheckpoint { task }, Event::ScaffoldApproved) => {
                self.write_queue = self.work_units.iter().map(|unit| unit.id).collect();
                Phase::CommittingSkeleton { task }
            }
            (&Phase::CommittingSkeleton { task }, Event::SkeletonCommitted { commit }) => {
                self.record_optional_commit(commit, CommitOrigin::Skeleton);
                self.write_from(task, 0)
            }
            (&Phase::Writing { task, unit }, Event::WorkUnitPassed { id, .. }) => {
                self.expect_work_unit(unit, *id)?;
                Phase::Committing { task, unit }
            }
            (&Phase::Writing { task, unit }, Event::WorkUnitFailed { failure }) => {
                self.expect_work_unit(unit, failure.id)?;
                self.note_missing_response(failure);
                self.revise_or_escalate(task, RevisionCause::WorkUnitFailed(failure.clone()))
            }
            (&Phase::Committing { task, unit }, Event::WorkUnitCommitted { id, commit }) => {
                self.expect_work_unit(unit, *id)?;
                self.record_commit(commit, CommitOrigin::WorkUnit(*id));
                self.write_from(task, unit + 1)
            }
            (&Phase::TaskChecking { task }, Event::TaskChecksPassed) => {
                self.revisions.clear();
                self.clear_escalation_choices();
                self.task_commits_start = self.commits.len();
                self.next_task(task)
            }
            (&Phase::TaskChecking { task }, Event::TaskChecksFailed { output }) => {
                let cause = RevisionCause::TaskChecksFailed {
                    output: output.clone(),
                };
                self.revise_or_escalate(task, cause)
            }
            (Phase::Revising { task, cause }, Event::RevisionSubmitted { revision }) => {
                let (task, cause) = (*task, cause.clone());
                if revision.shared_change.is_some() {
                    self.record_round(&cause, revision, None);
                    Phase::DiscardingRevision { task, cause }
                } else {
                    self.check_revision_ids(&cause, revision)?;
                    if self.revision_checkpoints {
                        Phase::RevisionCheckpoint {
                            task,
                            cause,
                            revision: revision.clone(),
                        }
                    } else {
                        self.apply_revision(&cause, revision);
                        Phase::CommittingRevision { task }
                    }
                }
            }
            (Phase::Revising { task, cause }, Event::RevisionRejected { revision, reason }) => {
                let (task, cause) = (*task, cause.clone());
                self.record_round(&cause, revision, Some(reason));
                Phase::DiscardingRevision { task, cause }
            }
            (
                Phase::RevisionCheckpoint {
                    task,
                    cause,
                    revision,
                },
                Event::RevisionApproved,
            ) => {
                let (task, cause, revision) = (*task, cause.clone(), revision.clone());
                self.apply_revision(&cause, &revision);
                Phase::CommittingRevision { task }
            }
            (
                Phase::RevisionCheckpoint {
                    task,
                    cause,
                    revision,
                },
                Event::CheckpointRejected { feedback },
            ) => {
                let (task, cause, revision) = (*task, cause.clone(), revision.clone());
                self.record_round(&cause, &revision, Some(feedback));
                Phase::DiscardingRevision { task, cause }
            }
            (&Phase::CommittingRevision { task }, Event::RevisionCommitted { commit }) => {
                self.record_optional_commit(commit, CommitOrigin::Revision);
                self.write_from(task, 0)
            }
            (Phase::DiscardingRevision { task, cause }, Event::RevisionDiscarded) => {
                let (task, cause) = (*task, cause.clone());
                if self.last_round_needs_shared_change() {
                    Phase::EscalationCheckpoint { task, cause }
                } else {
                    self.revise_or_escalate(task, cause)
                }
            }
            (Phase::EscalationCheckpoint { task, cause }, Event::OperatorFixed { commit }) => {
                let (task, failed_unit) = (*task, cause.work_unit());
                self.record_optional_commit(commit, CommitOrigin::OperatorFix);
                match failed_unit {
                    Some(fixed) => {
                        self.write_queue = self.queue_after(fixed, &[]);
                        self.write_from(task, 0)
                    }
                    None => Phase::TaskChecking { task },
                }
            }
            (Phase::EscalationCheckpoint { task, cause }, Event::ReplanRequested) => {
                let (task, cause) = (*task, cause.clone());
                let next = self.replanning(task, cause)?;
                self.clear_escalation_choices();
                next
            }
            (Phase::EscalationCheckpoint { task, cause }, Event::RetryWithHint { tier, hint }) => {
                let (task, cause) = (*task, cause.clone());
                self.retry_with_hint(task, cause, *tier, hint)?
            }
            (
                Phase::EscalationCheckpoint { task, cause },
                Event::StrongerWriterRequested { model },
            ) => {
                let task = *task;
                let RevisionCause::WorkUnitFailed(failure) = cause else {
                    return Err(ApplyError::NoFailedWorkUnit);
                };
                let stronger_unit = failure.id;
                let stronger = StrongerWriter {
                    work_unit: stronger_unit,
                    model: model.clone(),
                    failure: failure.clone(),
                };
                self.clear_escalation_choices();
                self.stronger_writer = Some(stronger);
                self.write_again(task, stronger_unit)
            }
            (&Phase::EscalationCheckpoint { task, .. }, Event::TaskSkipped) => {
                Phase::SkippingTask { task }
            }
            (Phase::EscalationCheckpoint { .. }, Event::SessionAborted) => Phase::Aborted,
            (Phase::Replanning { task, cause, .. }, Event::ReplanSubmitted { replan }) => {
                Phase::ReplanCheckpoint {
                    task: *task,
                    cause: cause.clone(),
                    replan: replan.clone(),
                }
            }
            (
                Phase::ReplanCheckpoint { task, cause, .. },
                Event::CheckpointRejected { feedback },
            ) => Phase::Replanning {
                task: *task,
                cause: cause.clone(),
                feedback: Some(feedback.clone()),
            },
            (Phase::ReplanCheckpoint { task, replan, .. }, Event::PlanApproved { task_order }) => {
                let (task, replan) = (*task, replan.clone());
                self.approve_replan(task, &replan, task_order)?;
                Phase::ResettingTask { task }
            }
            (&Phase::ResettingTask { task }, Event::TaskReset) => {
                self.reset_task();
                Phase::Scaffolding {
                    task,
                    feedback: None,
                }
            }
            (&Phase::SkippingTask { task }, Event::TaskReset) => {
                self.reset_task();
                self.skipped_tasks.push(task);
                self.next_task(task)
            }
            (Phase::CheckingMarkers, Event::MarkersChecked { remaining }) => {
                if remaining.is_empty() {
                    Phase::Done
                } else {
                    Phase::MarkerCheckpoint {
                        remaining: remaining.clone(),
                    }
                }
            }
            (Phase::MarkerCheckpoint { .. }, Event::OperatorFixed { commit }) => {
                self.record_optional_commit(commit, CommitOrigin::OperatorFix);
                Phase::CheckingMarkers
            }
            (Phase::MarkerCheckpoint { .. }, Event::MarkersApproved) => Phase::Done,
            (phase, Event::UsageRecorded { tier, usage }) if is_live(phase) => {
                self.usage.record(*tier, *usage);
                phase.clone()
            }
            (phase, &Event::BudgetCapSet { cap }) if is_live(phase) => {
                let spent = self.usage.total().cost;
                if let Some(cap) = cap
                    && cap <= spent
                {
                    return Err(ApplyError::BudgetCapReached { cap, spent });
                }
                self.budget_cap = cap;
                phase.clone()
            }
            (phase, &Event::ClarificationModeSet { mode }) if is_live(phase) => {
                self.clarification_mode = Some(mode);
                phase.clone()
            }
            (phase, &Event::RevisionCheckpointsSet { enabled }) if is_live(phase) => {
                self.revision_checkpoints = enabled;
                phase.clone()
            }
            (_, Event::SessionStopped) if self.is_at_budget_checkpoint() => Phase::Stopped,
            (phase, Event::RunEnded { .. }) => phase.clone(),
            (_, Event::Rewound { .. }) => return Err(ApplyError::RewindNeedsLog),
            _ => return Err(unexpected()),
        };
        self.phase = next;
        Ok(self.pending())
    }

    /// The effects the current state is waiting on. After a resume, these redo the in-flight work.
    pub fn pending(&self) -> Vec<Effect> {
        if self.is_at_budget_checkpoint() {
            return vec![Effect::ShowBudgetCheckpoint];
        }
        let effect = match &self.phase {
            Phase::NotStarted => return vec![],
            Phase::Planning { feedback } => Effect::RunPlanner {
                feedback: feedback.clone(),
            },
            Phase::PlanCheckpoint => Effect::ShowPlanCheckpoint,
            Phase::Scaffolding { task, feedback } => Effect::RunScaffolder {
                task: *task,
                feedback: feedback.clone(),
            },
            &Phase::ScaffoldChecking { task } => Effect::RunScaffoldChecks { task },
            &Phase::ScaffoldCheckpoint { task } => Effect::ShowScaffoldCheckpoint { task },
            &Phase::CommittingSkeleton { task } => Effect::CommitSkeleton { task },
            &Phase::Writing { unit, .. } => Effect::RunWriter {
                id: self.write_queue[unit],
            },
            &Phase::Committing { unit, .. } => Effect::CommitWorkUnit {
                id: self.write_queue[unit],
            },
            &Phase::TaskChecking { task } => Effect::RunTaskChecks { task },
            Phase::Revising { task, cause } => Effect::RunRevision {
                task: *task,
                cause: cause.clone(),
            },
            Phase::RevisionCheckpoint { task, revision, .. } => Effect::ShowRevisionCheckpoint {
                task: *task,
                revision: revision.clone(),
            },
            &Phase::CommittingRevision { task } => Effect::CommitRevision { task },
            &Phase::DiscardingRevision { task, .. } => Effect::DiscardRevision { task },
            Phase::EscalationCheckpoint { task, cause } => Effect::ShowEscalationCheckpoint {
                task: *task,
                cause: cause.clone(),
            },
            Phase::Replanning {
                task,
                cause,
                feedback,
            } => Effect::RunReplan {
                task: *task,
                cause: cause.clone(),
                feedback: feedback.clone(),
            },
            Phase::ReplanCheckpoint { task, replan, .. } => Effect::ShowReplanCheckpoint {
                task: *task,
                replan: replan.clone(),
            },
            &Phase::ResettingTask { task } | &Phase::SkippingTask { task } => {
                Effect::ResetTask { task }
            }
            Phase::CheckingMarkers => Effect::CheckMarkers,
            Phase::MarkerCheckpoint { remaining } => Effect::ShowMarkerCheckpoint {
                remaining: remaining.clone(),
            },
            Phase::Done | Phase::Stopped | Phase::Aborted => Effect::EndSession,
        };
        vec![effect]
    }

    fn task_count(&self) -> usize {
        self.plan.as_ref().map_or(0, |plan| plan.tasks.len())
    }

    fn next_task(&self, task: usize) -> Phase {
        if task + 1 < self.task_count() {
            Phase::Scaffolding {
                task: task + 1,
                feedback: None,
            }
        } else {
            Phase::CheckingMarkers
        }
    }

    fn replanning(&self, task: usize, cause: RevisionCause) -> Result<Phase, ApplyError> {
        if !self.may_replan(task) {
            return Err(ApplyError::ReplanUsed { task });
        }
        Ok(Phase::Replanning {
            task,
            cause,
            feedback: None,
        })
    }

    /// Runs `tier` again from the Escalation Checkpoint, with the Human Operator's hint.
    fn retry_with_hint(
        &mut self,
        task: usize,
        cause: RevisionCause,
        tier: Tier,
        hint: &str,
    ) -> Result<Phase, ApplyError> {
        let failed_unit = cause.work_unit();
        if tier == Tier::Writer && failed_unit.is_none() {
            return Err(ApplyError::NoFailedWorkUnit);
        }
        let next = match (tier, failed_unit) {
            (Tier::Writer, Some(failed)) => self.write_again(task, failed),
            (Tier::Planner, _) => self.replanning(task, cause)?,
            _ => Phase::Revising { task, cause },
        };
        self.clear_escalation_choices();
        self.hint = Some(OperatorHint {
            tier,
            work_unit: failed_unit,
            text: hint.to_owned(),
        });
        Ok(next)
    }

    /// Writes the failed Work Unit again, then the rest of the queue after it.
    fn write_again(&mut self, task: usize, failed: WorkUnitId) -> Phase {
        self.write_queue = self.queue_after(failed, &[failed]);
        self.write_from(task, 0)
    }

    fn clear_escalation_choices(&mut self) {
        self.hint = None;
        self.stronger_writer = None;
    }

    /// Writes the queued Work Units from `unit` on, or checks the Task when none are left.
    fn write_from(&self, task: usize, unit: usize) -> Phase {
        if unit < self.write_queue.len() {
            Phase::Writing { task, unit }
        } else {
            Phase::TaskChecking { task }
        }
    }

    /// Puts the chosen Tasks of the Replan in place of the failed Task.
    fn approve_replan(
        &mut self,
        task: usize,
        replan: &Replan,
        task_order: &[usize],
    ) -> Result<(), ApplyError> {
        if !is_valid_order(task_order, replan.tasks.len()) {
            return Err(ApplyError::InvalidTaskOrder {
                task_order: task_order.to_vec(),
            });
        }
        let plan = self
            .plan
            .as_mut()
            .expect("a Task runs only once a Plan is approved");
        let replacements = task_order
            .iter()
            .map(|&task_index| replan.tasks[task_index].clone());
        plan.tasks.splice(task..=task, replacements);
        self.replanned
            .splice(task..=task, std::iter::repeat_n(true, task_order.len()));
        Ok(())
    }

    /// Forgets the Task's work once its worktree is back where the Task started. Work Unit IDs
    /// stay used, so the event log never names two Work Units the same.
    fn reset_task(&mut self) {
        self.commits.truncate(self.task_commits_start);
        self.work_units.clear();
        self.write_queue.clear();
        self.revisions.clear();
        self.clear_escalation_choices();
    }

    /// The Human Operator's Checkpoint comes before anything goes to the Planner.
    fn revise_or_escalate(&self, task: usize, cause: RevisionCause) -> Phase {
        if revisions_for(&self.revisions, &cause) < MAX_REVISIONS {
            Phase::Revising { task, cause }
        } else {
            Phase::EscalationCheckpoint { task, cause }
        }
    }

    fn last_round_needs_shared_change(&self) -> bool {
        self.revisions
            .last()
            .is_some_and(|round| round.revision.shared_change.is_some())
    }

    fn note_missing_response(&mut self, failure: &WorkUnitFailure) {
        if failure.missing_response.is_some() {
            self.missing_responses.insert(failure.id);
        }
    }

    /// The Work Units before `failed` in the queue have passed, so they are dropped.
    fn queue_after(&self, failed: WorkUnitId, replacements: &[WorkUnitId]) -> Vec<WorkUnitId> {
        let position = self.write_queue.iter().position(|&id| id == failed);
        let rest = position.map_or(&[][..], |position| &self.write_queue[position + 1..]);
        replacements.iter().chain(rest).copied().collect()
    }

    fn record_commit(&mut self, hash: &str, origin: CommitOrigin) {
        self.commits.push(SessionCommit {
            hash: hash.to_owned(),
            origin,
        });
    }

    fn record_optional_commit(&mut self, commit: &Option<String>, origin: CommitOrigin) {
        if let Some(hash) = commit {
            self.record_commit(hash, origin);
        }
    }

    fn note_ids(&mut self, work_units: &[WorkUnit]) {
        let highest = work_units.iter().map(|unit| unit.id).max();
        self.highest_id = self.highest_id.max(highest);
    }

    /// A revised Work Unit keeps an ID from the current Task, only the failed one's when one Work
    /// Unit failed; a new one takes an unused ID.
    fn check_revision_ids(
        &self,
        cause: &RevisionCause,
        revision: &Revision,
    ) -> Result<(), ApplyError> {
        if let Some(id) = first_duplicate(&revision.work_units) {
            return Err(ApplyError::DuplicateWorkUnit { id });
        }
        let in_task = |id| self.work_units.iter().any(|unit| unit.id == id);
        if let Some(failed) = cause.work_unit()
            && let Some(id) = revision
                .work_units
                .iter()
                .map(|unit| unit.id)
                .find(|&id| id != failed && in_task(id))
        {
            return Err(ApplyError::OutsideRevisionScope { id });
        }
        let taken = revision
            .work_units
            .iter()
            .map(|unit| unit.id)
            .find(|&id| !in_task(id) && id < self.next_work_unit_id());
        match taken {
            Some(id) => Err(ApplyError::WorkUnitIdTaken { id }),
            None => Ok(()),
        }
    }

    fn apply_revision(&mut self, cause: &RevisionCause, revision: &Revision) {
        for revised in &revision.work_units {
            match self
                .work_units
                .iter_mut()
                .find(|unit| unit.id == revised.id)
            {
                Some(unit) => *unit = revised.clone(),
                None => self.work_units.push(revised.clone()),
            }
        }
        self.note_ids(&revision.work_units);
        let revised: Vec<WorkUnitId> = revision.work_units.iter().map(|unit| unit.id).collect();
        self.write_queue = match cause.work_unit() {
            Some(failed) => self.queue_after(failed, &revised),
            None => revised,
        };
        self.record_round(cause, revision, None);
    }

    fn record_round(
        &mut self,
        cause: &RevisionCause,
        revision: &Revision,
        rejection: Option<&String>,
    ) {
        self.revisions.push(RevisionRound {
            cause: cause.clone(),
            revision: revision.clone(),
            rejection: rejection.cloned(),
        });
    }

    fn expect_work_unit(&self, unit: usize, got: WorkUnitId) -> Result<(), ApplyError> {
        let expected = self.write_queue[unit];
        if got == expected {
            Ok(())
        } else {
            Err(ApplyError::WrongWorkUnit { expected, got })
        }
    }
}

/// A Checkpoint the Session can be rewound to.
pub(crate) fn is_rewind_target(phase: &Phase) -> bool {
    matches!(
        phase,
        Phase::PlanCheckpoint
            | Phase::ScaffoldCheckpoint { .. }
            | Phase::RevisionCheckpoint { .. }
            | Phase::EscalationCheckpoint { .. }
            | Phase::ReplanCheckpoint { .. }
            | Phase::MarkerCheckpoint { .. }
    )
}

/// Started and not yet finished, so LLM calls can happen.
fn is_live(phase: &Phase) -> bool {
    !matches!(
        phase,
        Phase::NotStarted | Phase::Done | Phase::Stopped | Phase::Aborted
    )
}

fn first_duplicate(work_units: &[WorkUnit]) -> Option<WorkUnitId> {
    let mut seen = std::collections::HashSet::new();
    work_units
        .iter()
        .map(|unit| unit.id)
        .find(|&id| !seen.insert(id))
}

/// A non-empty selection of distinct indices below `len`.
fn is_valid_order(order: &[usize], len: usize) -> bool {
    if order.is_empty() {
        return false;
    }
    let mut seen = vec![false; len];
    for &task_index in order {
        if task_index >= len || seen[task_index] {
            return false;
        }
        seen[task_index] = true;
    }
    true
}
