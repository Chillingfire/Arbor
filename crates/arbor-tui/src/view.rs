//! What the TUI shows, folded from the Session's event log (ADR-0004). The Session state machine
//! keeps only the current Task's Work Units, so the tree's history is built here.

use arbor_core::{ApplyError, Event, Phase, Plan, Session, Task, WorkUnit, WorkUnitId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Active,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkUnitView {
    pub work_unit: WorkUnit,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskView {
    pub task: Task,
    pub status: Status,
    pub work_units: Vec<WorkUnitView>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionView {
    session: Session,
    tasks: Vec<TaskView>,
    log: Vec<String>,
}

impl SessionView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_events<'a>(
        events: impl IntoIterator<Item = &'a Event>,
    ) -> Result<Self, ApplyError> {
        let mut view = Self::new();
        for event in events {
            view.apply(event)?;
        }
        Ok(view)
    }

    /// Applies the event to the Session and updates the tree and the event log to match.
    pub fn apply(&mut self, event: &Event) -> Result<(), ApplyError> {
        let task_before = phase_task(self.session.phase());
        self.session.apply(event)?;
        self.log.push(describe(event));
        match event {
            Event::PlanApproved { .. } => self.take_approved_plan(task_before),
            Event::WorkUnitsSubmitted { .. } | Event::RevisionSubmitted { .. } => {
                self.sync_work_units();
            }
            Event::WorkUnitPassed { id } | Event::WorkUnitCommitted { id, .. } => {
                self.set_work_unit_status(*id, Status::Done);
            }
            Event::WorkUnitFailed { failure } => {
                self.set_work_unit_status(failure.id, Status::Failed);
            }
            _ => {}
        }
        self.update_task_statuses();
        Ok(())
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn plan(&self) -> Option<&Plan> {
        self.session.plan()
    }

    pub fn tasks(&self) -> &[TaskView] {
        &self.tasks
    }

    /// One line per event, oldest first.
    pub fn log(&self) -> &[String] {
        &self.log
    }

    /// Tasks finished before a Replan keep their Work Units; the rest come from the new Plan.
    fn take_approved_plan(&mut self, replanned_task: Option<usize>) {
        let Some(plan) = self.session.plan() else {
            return;
        };
        let kept = replanned_task.unwrap_or(0).min(self.tasks.len());
        self.tasks.truncate(kept);
        self.tasks
            .extend(plan.tasks.iter().skip(kept).map(|task| TaskView {
                task: task.clone(),
                status: Status::Pending,
                work_units: Vec::new(),
            }));
    }

    /// Takes the current Task's Work Units from the Session. Units a Revision rewrote are pending
    /// again; the others keep their status.
    fn sync_work_units(&mut self) {
        let Some(task_index) = phase_task(self.session.phase()) else {
            return;
        };
        let rewritten: Vec<WorkUnitId> = match self.session.revisions().last() {
            Some(round) => round
                .revision
                .work_units
                .iter()
                .map(|unit| unit.id)
                .collect(),
            None => Vec::new(),
        };
        let Some(task_view) = self.tasks.get_mut(task_index) else {
            return;
        };
        let previous = std::mem::take(&mut task_view.work_units);
        task_view.work_units = self
            .session
            .work_units()
            .iter()
            .map(|work_unit| {
                let earlier_status = previous
                    .iter()
                    .find(|earlier| earlier.work_unit.id == work_unit.id)
                    .map(|earlier| earlier.status);
                let status = match earlier_status {
                    Some(status) if !rewritten.contains(&work_unit.id) => status,
                    _ => Status::Pending,
                };
                WorkUnitView {
                    work_unit: work_unit.clone(),
                    status,
                }
            })
            .collect();
    }

    fn set_work_unit_status(&mut self, id: WorkUnitId, status: Status) {
        let work_unit_view = self
            .tasks
            .iter_mut()
            .flat_map(|task_view| &mut task_view.work_units)
            .find(|work_unit_view| work_unit_view.work_unit.id == id);
        if let Some(work_unit_view) = work_unit_view {
            work_unit_view.status = status;
        }
    }

    fn update_task_statuses(&mut self) {
        let phase = self.session.phase();
        let done = matches!(
            phase,
            Phase::Done | Phase::CheckingMarkers | Phase::MarkerCheckpoint { .. }
        );
        let current = phase_task(phase);
        let writing = matches!(phase, Phase::Writing { .. });
        for (task_index, task_view) in self.tasks.iter_mut().enumerate() {
            task_view.status = match current {
                _ if done => Status::Done,
                Some(current) if task_index < current => Status::Done,
                Some(current) if task_index == current => Status::Active,
                _ => Status::Pending,
            };
            for work_unit_view in &mut task_view.work_units {
                if work_unit_view.status == Status::Active {
                    work_unit_view.status = Status::Pending;
                }
            }
            if writing && current == Some(task_index) {
                let next_unwritten = task_view
                    .work_units
                    .iter_mut()
                    .find(|work_unit_view| work_unit_view.status == Status::Pending);
                if let Some(work_unit_view) = next_unwritten {
                    work_unit_view.status = Status::Active;
                }
            }
        }
    }
}

/// The approved Plan's Task the phase works on.
pub fn phase_task(phase: &Phase) -> Option<usize> {
    match phase {
        Phase::Scaffolding { task, .. }
        | Phase::ScaffoldChecking { task }
        | Phase::ScaffoldCheckpoint { task }
        | Phase::CommittingSkeleton { task }
        | Phase::Writing { task, .. }
        | Phase::Committing { task, .. }
        | Phase::TaskChecking { task }
        | Phase::Revising { task, .. }
        | Phase::RevisionCheckpoint { task, .. }
        | Phase::CommittingRevision { task }
        | Phase::DiscardingRevision { task, .. }
        | Phase::EscalationCheckpoint { task, .. }
        | Phase::Replanning { task, .. }
        | Phase::ReplanCheckpoint { task, .. }
        | Phase::ResettingTask { task } => Some(*task),
        _ => None,
    }
}

/// A one-line description of the event for the event log.
pub fn describe(event: &Event) -> String {
    match event {
        Event::SessionStarted { request } => {
            format!("Session started: {}", first_line(&request.text))
        }
        Event::PlanSubmitted { plan } => format!(
            "Planner submitted a Plan with {} Task{}",
            plan.tasks.len(),
            plural(plan.tasks.len())
        ),
        Event::PlanApproved { task_order } => format!(
            "Plan approved: {} Task{}",
            task_order.len(),
            plural(task_order.len())
        ),
        Event::CheckpointRejected { feedback } => {
            format!("Checkpoint rejected: {}", first_line(feedback))
        }
        Event::WorkUnitsSubmitted { work_units } => format!(
            "Scaffolder submitted {} Work Unit{}",
            work_units.len(),
            plural(work_units.len())
        ),
        Event::ScaffoldChecksPassed => "Scaffold Check passed".to_owned(),
        Event::ScaffoldApproved => "Skeleton approved".to_owned(),
        Event::SkeletonCommitted { commit } => committed("Skeleton", commit.as_deref()),
        Event::WorkUnitPassed { id } => format!("{id} passed its Checks"),
        Event::WorkUnitFailed { failure } => match &failure.missing_response {
            Some(_) => format!("{} sent a Missing Response", failure.id),
            None => format!(
                "{} failed after {} attempt{}",
                failure.id,
                failure.attempts.len(),
                plural(failure.attempts.len())
            ),
        },
        Event::WorkUnitCommitted { id, commit } => format!("{id} committed {}", short(commit)),
        Event::TaskChecksPassed => "Task's Unit Check passed".to_owned(),
        Event::TaskChecksFailed { .. } => "Task's Unit Check failed".to_owned(),
        Event::RevisionSubmitted { revision } => format!(
            "Scaffolder submitted a Revision of {} Work Unit{}",
            revision.work_units.len(),
            plural(revision.work_units.len())
        ),
        Event::RevisionRejected { reason, .. } => {
            format!("Revision rejected: {}", first_line(reason))
        }
        Event::RevisionCheckpointsSet { enabled } => format!(
            "Revision Checkpoints turned {}",
            if *enabled { "on" } else { "off" }
        ),
        Event::RevisionApproved => "Revision approved".to_owned(),
        Event::RevisionCommitted { commit } => committed("Revision", commit.as_deref()),
        Event::RevisionDiscarded => "Revision discarded".to_owned(),
        Event::ReplanRequested => "Task sent to the Planner for a Replan".to_owned(),
        Event::ReplanSubmitted { replan } => format!(
            "Planner submitted a Replan with {} Task{}",
            replan.tasks.len(),
            plural(replan.tasks.len())
        ),
        Event::TaskReset => "Task reset to where it started".to_owned(),
        Event::OperatorFixed { commit } => committed("Operator's fix", commit.as_deref()),
        Event::MarkersChecked { remaining } if remaining.is_empty() => {
            "No Markers left in the branch".to_owned()
        }
        Event::MarkersChecked { remaining } => format!(
            "{} Marker{} left in the branch",
            remaining.len(),
            plural(remaining.len())
        ),
        Event::MarkersApproved => "Remaining Markers approved".to_owned(),
        Event::UsageRecorded { tier, usage } => format!(
            "{tier:?} call: {} in / {} out tokens, ${:.4}",
            tokens(usage.input_tokens),
            tokens(usage.output_tokens),
            usage.cost.dollars()
        ),
        Event::BudgetCapSet { cap: Some(cap) } => {
            format!("Budget Cap set to ${:.2}", cap.dollars())
        }
        Event::BudgetCapSet { cap: None } => "Budget Cap removed".to_owned(),
        Event::SessionStopped => "Session stopped at the Budget Cap".to_owned(),
    }
}

/// Token counts as `950`, `12.3k` or `1.2M`.
pub fn tokens(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{:.1}k", count as f64 / 1_000.0),
        _ => format!("{:.1}M", count as f64 / 1_000_000.0),
    }
}

fn committed(what: &str, commit: Option<&str>) -> String {
    match commit {
        Some(hash) => format!("{what} committed {}", short(hash)),
        None => format!("{what} changed nothing, so nothing was committed"),
    }
}

fn short(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

fn first_line(text: &str) -> &str {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}
