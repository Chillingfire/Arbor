//! What the TUI shows, folded from the Session's event log (ADR-0004). The Session state machine
//! keeps only the current Task's Work Units, so the tree's history is built here.

use arbor_core::{
    ApplyError, Event, Phase, Plan, Session, Task, Tier, WorkUnit, WorkUnitFailure, WorkUnitId,
};

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
    /// The Writer's replies to this Work Unit, oldest first, across Revisions.
    pub transcript: Vec<WriterReplyRecord>,
    /// The commit of the Work Unit's accepted code.
    pub commit: Option<String>,
    /// The committed code as a unified diff, once the program running the TUI has read it.
    pub diff: Option<String>,
}

/// One Writer reply and what became of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriterReplyRecord {
    pub reply: String,
    pub outcome: ReplyOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyOutcome {
    Passed,
    /// Why the reply was rejected, or the Checks' output.
    Failed(String),
    /// What the Writer said its Context Pack lacked.
    Missing(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskView {
    pub task: Task,
    pub status: Status,
    pub work_units: Vec<WorkUnitView>,
}

/// How an event log line reads: good news, bad news, a model call, or plain progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Progress,
    Success,
    Failure,
    Operator,
    ModelCall,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    pub text: String,
    /// The Tier whose work the event records, if any.
    pub tier: Option<Tier>,
    pub tone: Tone,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionView {
    session: Session,
    tasks: Vec<TaskView>,
    log: Vec<LogEntry>,
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
        let escalated_unit = match self.session.phase() {
            Phase::EscalationCheckpoint { cause, .. } => cause.work_unit(),
            _ => None,
        };
        self.session.apply(event)?;
        self.log.push(LogEntry {
            text: describe(event),
            tier: event_tier(event),
            tone: event_tone(event),
        });
        match event {
            Event::PlanApproved { .. } => self.take_approved_plan(task_before),
            Event::WorkUnitsSubmitted { .. } | Event::RevisionSubmitted { .. } => {
                self.sync_work_units();
            }
            Event::WorkUnitPassed { id } => self.set_work_unit_status(*id, Status::Done),
            Event::WorkUnitCommitted { id, commit } => {
                self.set_work_unit_status(*id, Status::Done);
                if let Some(work_unit_view) = self.work_unit_mut(*id) {
                    work_unit_view.commit = Some(commit.clone());
                }
            }
            Event::WorkUnitFailed { failure } => {
                self.set_work_unit_status(failure.id, Status::Failed);
                self.take_failed_replies(failure);
            }
            Event::OperatorFixed { .. } => {
                if let Some(fixed) = escalated_unit {
                    self.set_work_unit_status(fixed, Status::Done);
                }
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

    /// Adds a line to the event log that no event stands for, e.g. why the run stopped.
    pub fn push_note(&mut self, text: impl Into<String>, tone: Tone) {
        self.log.push(LogEntry {
            text: text.into(),
            tier: None,
            tone,
        });
    }

    /// One line per event, oldest first.
    pub fn log(&self) -> &[LogEntry] {
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
        let mut previous = std::mem::take(&mut task_view.work_units);
        task_view.work_units = self
            .session
            .work_units()
            .iter()
            .map(|work_unit| {
                let earlier = previous
                    .iter()
                    .position(|earlier| earlier.work_unit.id == work_unit.id)
                    .map(|index| previous.swap_remove(index));
                match earlier {
                    Some(earlier) if !rewritten.contains(&work_unit.id) => WorkUnitView {
                        work_unit: work_unit.clone(),
                        ..earlier
                    },
                    // A rewritten unit starts over but keeps the replies that led to its Revision.
                    Some(earlier) => WorkUnitView {
                        work_unit: work_unit.clone(),
                        status: Status::Pending,
                        transcript: earlier.transcript,
                        commit: None,
                        diff: None,
                    },
                    None => WorkUnitView {
                        work_unit: work_unit.clone(),
                        status: Status::Pending,
                        transcript: Vec::new(),
                        commit: None,
                        diff: None,
                    },
                }
            })
            .collect();
    }

    fn set_work_unit_status(&mut self, id: WorkUnitId, status: Status) {
        if let Some(work_unit_view) = self.work_unit_mut(id) {
            work_unit_view.status = status;
        }
    }

    fn work_unit_mut(&mut self, id: WorkUnitId) -> Option<&mut WorkUnitView> {
        self.tasks
            .iter_mut()
            .rev()
            .flat_map(|task_view| &mut task_view.work_units)
            .find(|work_unit_view| work_unit_view.work_unit.id == id)
    }

    /// The program running the TUI records each Writer reply as it comes; replaying a log only has
    /// the failed ones, so they are filled in from the failure when none were recorded.
    fn take_failed_replies(&mut self, failure: &WorkUnitFailure) {
        let Some(work_unit_view) = self.work_unit_mut(failure.id) else {
            return;
        };
        let recorded = work_unit_view.transcript.len();
        let failed_replies = failure.attempts.iter().map(|attempt| WriterReplyRecord {
            reply: attempt.reply.clone(),
            outcome: ReplyOutcome::Failed(attempt.error.clone()),
        });
        let missing = failure
            .missing_response
            .clone()
            .map(|items| WriterReplyRecord {
                reply: String::new(),
                outcome: ReplyOutcome::Missing(items),
            });
        let replies: Vec<WriterReplyRecord> = failed_replies.chain(missing).collect();
        if recorded == 0 {
            work_unit_view.transcript = replies;
        }
    }

    /// Adds a Writer reply to the Work Unit's transcript. A failed one also goes in the event log,
    /// since only the Work Unit's final outcome is an event.
    pub fn record_writer_reply(&mut self, id: WorkUnitId, record: WriterReplyRecord) {
        let Some(work_unit_view) = self.work_unit_mut(id) else {
            return;
        };
        let failure = match &record.outcome {
            ReplyOutcome::Passed => None,
            ReplyOutcome::Failed(reason) => Some(format!(
                "failed: {}",
                first_line(reason).trim_end_matches(':')
            )),
            ReplyOutcome::Missing(_) => Some("sent a Missing Response".to_owned()),
        };
        work_unit_view.transcript.push(record);
        let attempt = work_unit_view.transcript.len();
        if let Some(failure) = failure {
            self.log.push(LogEntry {
                text: format!("{id} attempt {attempt} {failure}"),
                tier: Some(Tier::Writer),
                tone: Tone::Failure,
            });
        }
    }

    /// Sets the diff of a committed Work Unit, read from git by the program running the TUI.
    pub fn set_diff(&mut self, id: WorkUnitId, diff: String) {
        if let Some(work_unit_view) = self.work_unit_mut(id) {
            work_unit_view.diff = Some(diff);
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

pub(crate) fn short(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

pub(crate) fn first_line(text: &str) -> &str {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// The Tier whose work the event records.
pub fn event_tier(event: &Event) -> Option<Tier> {
    match event {
        Event::PlanSubmitted { .. } | Event::ReplanSubmitted { .. } => Some(Tier::Planner),
        Event::WorkUnitsSubmitted { .. }
        | Event::RevisionSubmitted { .. }
        | Event::RevisionRejected { .. } => Some(Tier::Scaffolder),
        Event::WorkUnitPassed { .. } | Event::WorkUnitFailed { .. } => Some(Tier::Writer),
        Event::UsageRecorded { tier, .. } => Some(*tier),
        _ => None,
    }
}

pub fn event_tone(event: &Event) -> Tone {
    match event {
        Event::UsageRecorded { .. } => Tone::ModelCall,
        Event::ScaffoldChecksPassed
        | Event::TaskChecksPassed
        | Event::WorkUnitPassed { .. }
        | Event::WorkUnitCommitted { .. } => Tone::Success,
        Event::WorkUnitFailed { .. }
        | Event::TaskChecksFailed { .. }
        | Event::RevisionRejected { .. }
        | Event::SessionStopped => Tone::Failure,
        Event::MarkersChecked { remaining } if !remaining.is_empty() => Tone::Failure,
        Event::PlanApproved { .. }
        | Event::ScaffoldApproved
        | Event::RevisionApproved
        | Event::CheckpointRejected { .. }
        | Event::ReplanRequested
        | Event::OperatorFixed { .. }
        | Event::MarkersApproved
        | Event::BudgetCapSet { .. }
        | Event::RevisionCheckpointsSet { .. } => Tone::Operator,
        _ => Tone::Progress,
    }
}
