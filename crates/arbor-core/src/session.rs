use std::fmt;

use crate::cost::{UsageTotals, Usd};
use crate::event::Event;
use crate::revision::{MAX_REVISIONS, Revision, RevisionCause, RevisionRound};
use crate::types::{Plan, Request, WorkUnit, WorkUnitId};

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
    /// The Revisions are used up, so the Human Operator decides before anything goes to the
    /// Planner.
    ShowEscalationCheckpoint {
        task: usize,
        cause: RevisionCause,
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
    EscalationCheckpoint {
        task: usize,
        cause: RevisionCause,
    },
    Done,
    /// The Human Operator stopped the Session at a budget Checkpoint.
    Stopped,
}

/// An event that doesn't fit the Session's current state. The state is left unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    Unexpected {
        phase: Phase,
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
    /// The event names a Work Unit other than the one in progress.
    WrongWorkUnit {
        expected: WorkUnitId,
        got: WorkUnitId,
    },
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
            Self::WrongWorkUnit { expected, got } => {
                write!(f, "event for {got} while {expected} is in progress")
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
    commits: Vec<(WorkUnitId, String)>,
    /// Where the current Task's commits start in `commits`.
    task_commits_start: usize,
    /// The current Task's Revisions.
    revisions: Vec<RevisionRound>,
    usage: UsageTotals,
    budget_cap: Option<Usd>,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuilds a Session from its event log. Call `pending` afterwards to redo in-flight work.
    pub fn replay<'a>(events: impl IntoIterator<Item = &'a Event>) -> Result<Self, ApplyError> {
        let mut session = Self::new();
        for event in events {
            session.apply(event)?;
        }
        Ok(session)
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

    pub fn commits(&self) -> &[(WorkUnitId, String)] {
        &self.commits
    }

    /// The commits made since the current Task started, oldest first. The first one's parent is
    /// where the Task diff starts.
    pub fn task_commits(&self) -> &[(WorkUnitId, String)] {
        &self.commits[self.task_commits_start..]
    }

    /// The current Task's Revisions so far, oldest first.
    pub fn revisions(&self) -> &[RevisionRound] {
        &self.revisions
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

    /// Finished with every Task done. A Session stopped at a budget Checkpoint is not done.
    pub fn is_done(&self) -> bool {
        self.phase == Phase::Done
    }

    /// Applies one event and returns the effects that should follow it.
    pub fn apply(&mut self, event: &Event) -> Result<Vec<Effect>, ApplyError> {
        let unexpected = || ApplyError::Unexpected {
            phase: self.phase.clone(),
            event: Box::new(event.clone()),
        };
        let resolves_or_records = matches!(
            event,
            Event::BudgetCapSet { .. } | Event::SessionStopped | Event::UsageRecorded { .. }
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
                plan.tasks = task_order.iter().map(|&i| plan.tasks[i].clone()).collect();
                Phase::Scaffolding {
                    task: 0,
                    feedback: None,
                }
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
                self.write_from(task, 0)
            }
            (&Phase::Writing { task, unit }, Event::WorkUnitPassed { id }) => {
                self.expect_work_unit(unit, *id)?;
                Phase::Committing { task, unit }
            }
            (&Phase::Committing { task, unit }, Event::WorkUnitCommitted { id, commit }) => {
                self.expect_work_unit(unit, *id)?;
                self.commits.push((*id, commit.clone()));
                self.write_from(task, unit + 1)
            }
            (&Phase::TaskChecking { task }, Event::TaskChecksPassed) => {
                self.revisions.clear();
                self.task_commits_start = self.commits.len();
                if task + 1 < self.task_count() {
                    Phase::Scaffolding {
                        task: task + 1,
                        feedback: None,
                    }
                } else {
                    Phase::Done
                }
            }
            (&Phase::TaskChecking { task }, Event::TaskChecksFailed { output }) => {
                let cause = RevisionCause::TaskChecksFailed {
                    output: output.clone(),
                };
                if self.revisions.len() < MAX_REVISIONS {
                    Phase::Revising { task, cause }
                } else {
                    Phase::EscalationCheckpoint { task, cause }
                }
            }
            (Phase::Revising { task, cause }, Event::RevisionSubmitted { revision }) => {
                let (task, cause) = (*task, cause.clone());
                self.check_revision_ids(revision)?;
                self.apply_revision(cause, revision);
                self.write_from(task, 0)
            }
            (&Phase::EscalationCheckpoint { task, .. }, Event::OperatorFixed) => {
                Phase::TaskChecking { task }
            }
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
            (_, Event::SessionStopped) if self.is_at_budget_checkpoint() => Phase::Stopped,
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
            Phase::EscalationCheckpoint { task, cause } => Effect::ShowEscalationCheckpoint {
                task: *task,
                cause: cause.clone(),
            },
            Phase::Done | Phase::Stopped => Effect::EndSession,
        };
        vec![effect]
    }

    fn task_count(&self) -> usize {
        self.plan.as_ref().map_or(0, |plan| plan.tasks.len())
    }

    /// Writes the queued Work Units from `unit` on, or checks the Task when none are left.
    fn write_from(&self, task: usize, unit: usize) -> Phase {
        if unit < self.write_queue.len() {
            Phase::Writing { task, unit }
        } else {
            Phase::TaskChecking { task }
        }
    }

    fn note_ids(&mut self, work_units: &[WorkUnit]) {
        let highest = work_units.iter().map(|unit| unit.id).max();
        self.highest_id = self.highest_id.max(highest);
    }

    /// A revised Work Unit keeps an ID from the current Task; a new one takes an unused ID.
    fn check_revision_ids(&self, revision: &Revision) -> Result<(), ApplyError> {
        if let Some(id) = first_duplicate(&revision.work_units) {
            return Err(ApplyError::DuplicateWorkUnit { id });
        }
        let in_task = |id| self.work_units.iter().any(|unit| unit.id == id);
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

    fn apply_revision(&mut self, cause: RevisionCause, revision: &Revision) {
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
        self.write_queue = revision.work_units.iter().map(|unit| unit.id).collect();
        self.revisions.push(RevisionRound {
            cause,
            revision: revision.clone(),
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

/// Started and not yet finished, so LLM calls can happen.
fn is_live(phase: &Phase) -> bool {
    !matches!(phase, Phase::NotStarted | Phase::Done | Phase::Stopped)
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
    let mut seen = vec![false; len];
    !order.is_empty()
        && order
            .iter()
            .all(|&i| i < len && !std::mem::replace(&mut seen[i], true))
}
