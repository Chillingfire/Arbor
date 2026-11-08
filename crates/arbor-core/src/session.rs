use std::fmt;

use crate::cost::UsageTotals;
use crate::event::Event;
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
    EndSession,
}

/// Where a Session is. `task` indexes the approved Plan's Tasks and `unit` the current Task's
/// Work Units.
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
    Done,
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
    /// The event names a Work Unit other than the one in progress.
    WrongWorkUnit {
        expected: WorkUnitId,
        got: WorkUnitId,
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
            Self::WrongWorkUnit { expected, got } => {
                write!(f, "event for {got} while {expected} is in progress")
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
    /// The current Task's Work Units.
    work_units: Vec<WorkUnit>,
    commits: Vec<(WorkUnitId, String)>,
    usage: UsageTotals,
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

    /// Tokens and cost per Tier, from every recorded LLM call.
    pub fn usage(&self) -> &UsageTotals {
        &self.usage
    }

    pub fn is_done(&self) -> bool {
        self.phase == Phase::Done
    }

    /// Applies one event and returns the effects that should follow it.
    pub fn apply(&mut self, event: &Event) -> Result<Vec<Effect>, ApplyError> {
        let unexpected = || ApplyError::Unexpected {
            phase: self.phase.clone(),
            event: Box::new(event.clone()),
        };
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
                Phase::ScaffoldChecking { task }
            }
            (&Phase::ScaffoldChecking { task }, Event::ScaffoldChecksPassed) => {
                Phase::ScaffoldCheckpoint { task }
            }
            (&Phase::ScaffoldCheckpoint { task }, Event::ScaffoldApproved) => {
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
                if task + 1 < self.task_count() {
                    Phase::Scaffolding {
                        task: task + 1,
                        feedback: None,
                    }
                } else {
                    Phase::Done
                }
            }
            (phase, Event::UsageRecorded { tier, usage }) if is_live(phase) => {
                self.usage.record(*tier, *usage);
                phase.clone()
            }
            _ => return Err(unexpected()),
        };
        self.phase = next;
        Ok(self.pending())
    }

    /// The effects the current state is waiting on. After a resume, these redo the in-flight work.
    pub fn pending(&self) -> Vec<Effect> {
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
                id: self.work_units[unit].id,
            },
            &Phase::Committing { unit, .. } => Effect::CommitWorkUnit {
                id: self.work_units[unit].id,
            },
            &Phase::TaskChecking { task } => Effect::RunTaskChecks { task },
            Phase::Done => Effect::EndSession,
        };
        vec![effect]
    }

    fn task_count(&self) -> usize {
        self.plan.as_ref().map_or(0, |plan| plan.tasks.len())
    }

    /// Writes the Task's Work Units from `unit` on, or checks the Task when none are left.
    fn write_from(&self, task: usize, unit: usize) -> Phase {
        if unit < self.work_units.len() {
            Phase::Writing { task, unit }
        } else {
            Phase::TaskChecking { task }
        }
    }

    fn expect_work_unit(&self, unit: usize, got: WorkUnitId) -> Result<(), ApplyError> {
        let expected = self.work_units[unit].id;
        if got == expected {
            Ok(())
        } else {
            Err(ApplyError::WrongWorkUnit { expected, got })
        }
    }
}

/// Started and not yet finished, so LLM calls can happen.
fn is_live(phase: &Phase) -> bool {
    !matches!(phase, Phase::NotStarted | Phase::Done)
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
