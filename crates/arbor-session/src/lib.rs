//! Runs a Session: applies each event to the pure Session state machine and carries out the
//! effects it asks for with the real Tiers, real git and the project's real Checks. Checkpoints
//! and a Tier's clarifying questions go to an [`Operator`]; by default [`ApproveEverything`]. The
//! run stops where the Operator says so.

mod cleanup;
mod operator;
mod redo;
mod store;

use std::fmt;
use std::io::Write;

use arbor_config::{Config, RoleConfig, max_temperature};
use arbor_core::{
    ClarificationMode, Effect, Event, EventLog, Phase, Plan, Request, RevisionCause, RunOutcome,
    Session, StrongerWriter, Task, Tier, Usd, WorkUnit, WorkUnitFailure, WorkUnitId,
};
use arbor_git::{CommitId, CommitMessage, Mainline, Worktree};
use arbor_llm::{LlmClient, ModelInfo};
use arbor_prompts::{
    Conventions, PlannerInputs, Prompts, ReplanInputs, RevisionInputs, ScaffolderInputs,
    WriterInputs, estimate_tokens,
};
use arbor_tiers::{
    Agent, Attempt, AttemptError, AttemptOutcome, MARKER_FIX_SUBJECT, MAX_WRITER_ATTEMPTS, Outcome,
    PlannerRun, Rejection, ReplanRun, RevisionRun, ScaffolderRun, TASK_FIX_SUBJECT, Writer,
    WriterError, WriterRun, check_markers, check_operator_fix, commit_operator_fix,
    commit_revision, commit_skeleton, discard_revision, failure_brief, hint_input, reset_task,
    review_revision, run_task_check, strip_work_unit_markers, task_diff, validate_plan,
};

use arbor_tools::{CheckName, Tools, Toolset, run_check};
pub use cleanup::{
    PruneReport, PrunedSession, branch_markers, delete_session, list_sessions, prune_sessions,
};
pub use operator::{
    ApproveEverything, Checkpoint, Decision, Operator, Pending, StrongerWriterOffer,
};
pub use redo::resume_resets_worktree;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
pub use store::{
    RunLock, SessionFolder, SessionStatus, SessionSummary, StoreError, retention, rewind_session,
    session_folders, summarize,
};

/// How an [`Autopilot`] run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEnd {
    /// The Session finished: every Task is done and no Markers are left.
    Finished,
    /// The Session reached its end, but without these Tasks, skipped at the Escalation Checkpoint.
    FinishedWithSkippedTasks { skipped_tasks: Vec<usize> },
    /// The Human Operator stopped the Session at the Budget Checkpoint. The branch is kept.
    StoppedAtBudget,
    /// The Human Operator aborted the Session at the Escalation Checkpoint. The branch is kept.
    Aborted,
    /// The Operator stopped the run at this Checkpoint, where the Session waits.
    NeedsOperator(Effect),
    /// A Tier ended its run without submitting, by escalating or asking questions.
    TierStopped { tier: Tier, reason: String },
    /// The Scaffold Check failed on a Skeleton the Scaffolder had already checked.
    ScaffoldChecksFailed { check_output: String },
}

impl SessionEnd {
    /// What the log records about how the run ended, or `None` when the Session finished or was
    /// aborted, which its own events already record.
    pub fn run_outcome(&self) -> Option<RunOutcome> {
        match self {
            Self::Finished | Self::FinishedWithSkippedTasks { .. } | Self::Aborted => None,
            Self::StoppedAtBudget | Self::NeedsOperator(_) => Some(RunOutcome::Cancelled),
            Self::TierStopped { tier, reason } => Some(RunOutcome::Failed {
                reason: format!("the {tier:?} {reason}"),
            }),
            Self::ScaffoldChecksFailed { check_output } => Some(RunOutcome::Failed {
                reason: format!(
                    "the Scaffold Check failed: {}",
                    check_output.lines().last().unwrap_or_default()
                ),
            }),
        }
    }
}

/// A step the run couldn't carry out, such as a failed model call or git command.
#[derive(Debug, Clone, PartialEq)]
pub struct AutopilotError {
    pub step: &'static str,
    pub message: String,
}

impl fmt::Display for AutopilotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} failed: {}", self.step, self.message)
    }
}

impl std::error::Error for AutopilotError {}

fn failed<E: fmt::Display>(step: &'static str) -> impl FnOnce(E) -> AutopilotError {
    move |err| AutopilotError {
        step,
        message: err.to_string(),
    }
}

/// What a carried-out effect leads to: more events, or the end of the run.
enum Step {
    Events(Vec<Event>),
    End(SessionEnd),
}

/// What the Autopilot tells its observer while it runs.
#[derive(Debug, Clone, PartialEq)]
pub enum AutopilotUpdate {
    /// An event, just applied to the Session.
    Event(Event),
    /// Work that can take a while has started: a model call or a Check.
    Working(Activity),
    /// A Writer attempt finished, before its Work Unit's outcome event.
    WriterAttempt {
        work_unit: WorkUnitId,
        attempt: Attempt,
    },
    /// A Work Unit was committed with this diff.
    Committed { work_unit: WorkUnitId, diff: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    /// The Tier making a model call, or `None` for a Check.
    pub tier: Option<Tier>,
    pub model: Option<String>,
    /// What is running, e.g. `Writing WU-3, attempt 2`.
    pub label: String,
}

/// The attempts of one Writer run on a Work Unit.
#[derive(Debug, Clone, PartialEq)]
pub struct WriterRunAttempts {
    pub work_unit: WorkUnitId,
    pub attempts: Vec<Attempt>,
}

pub struct Autopilot<'a> {
    worktree: &'a Worktree,
    mainline: &'a Mainline,
    client: &'a dyn LlmClient,
    session: Session,
    /// The event log, oldest first (ADR-0004).
    events: Vec<Event>,
    writer_runs: Vec<WriterRunAttempts>,
    config: Config,
    conventions: Conventions,
    prompts: Prompts,
    /// The commit of the Work Unit that passed and waits for `WorkUnitCommitted`.
    passed_commit: Option<CommitId>,
    /// Set when a Task fixed by hand has just passed `checks.unit`, which `RunTaskChecks` would
    /// run again on the same commit.
    task_fix_checked: bool,
    /// Where each event is written before anyone is told of it (ADR-0004).
    event_log: Option<EventLog<Box<dyn Write + 'a>>>,
    observer: Option<Box<dyn FnMut(AutopilotUpdate) + 'a>>,
    operator: Box<dyn Operator + 'a>,
}

impl<'a> Autopilot<'a> {
    /// Loads the Conventions and prompt templates from the Session worktree.
    pub fn new(
        worktree: &'a Worktree,
        mainline: &'a Mainline,
        client: &'a dyn LlmClient,
        config: Config,
    ) -> Result<Self, AutopilotError> {
        let conventions = Conventions::load(worktree.path(), &config.context_files)
            .map_err(failed("loading the Conventions"))?;
        let prompts = Prompts::load(worktree.path()).map_err(failed("loading the prompts"))?;
        Ok(Self {
            worktree,
            mainline,
            client,
            session: Session::new(),
            events: Vec::new(),
            writer_runs: Vec::new(),
            config,
            conventions,
            prompts,
            passed_commit: None,
            task_fix_checked: false,
            event_log: None,
            observer: None,
            operator: Box::new(ApproveEverything),
        })
    }

    /// Sends each Checkpoint and each Tier's questions to `operator` instead of approving them.
    pub fn with_operator(mut self, operator: impl Operator + 'a) -> Self {
        self.operator = Box::new(operator);
        self
    }

    /// Writes each event to `event_file` as it is applied. A failed write ends the run, since a log
    /// with a gap can't be replayed.
    pub fn log_events_to(mut self, event_file: impl Write + 'a) -> Self {
        self.event_log = Some(EventLog::new(Box::new(event_file)));
        self
    }

    /// Calls `observer` with each update as the run goes, e.g. to show it live.
    pub fn on_update(mut self, observer: impl FnMut(AutopilotUpdate) + 'a) -> Self {
        self.observer = Some(Box::new(observer));
        self
    }

    fn notify(&mut self, update: AutopilotUpdate) {
        if let Some(observer) = &mut self.observer {
            observer(update);
        }
    }

    fn working(&mut self, tier: Option<Tier>, model: Option<&str>, label: String) {
        self.notify(AutopilotUpdate::Working(Activity {
            tier,
            model: model.map(str::to_owned),
            label,
        }));
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// The event log, oldest first.
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// Every Writer attempt, oldest first.
    pub fn writer_attempts(&self) -> Vec<&Attempt> {
        self.writer_runs
            .iter()
            .flat_map(|run| &run.attempts)
            .collect()
    }

    /// Each Writer run, oldest first. A Work Unit written again after a Revision has a run of
    /// its own.
    pub fn writer_runs(&self) -> &[WriterRunAttempts] {
        &self.writer_runs
    }

    /// Starts a Session for `request` and carries out its effects until it ends.
    pub async fn run(&mut self, request: Request) -> Result<SessionEnd, AutopilotError> {
        let mut effects = self.apply(Event::SessionStarted { request })?;
        for setting in self.configured_settings() {
            effects = self.apply(setting)?;
        }
        self.carry_out_from(effects).await
    }

    /// Carries on the Session whose log holds `events`, which aren't written again. Work that
    /// was in flight, such as a model call, is redone from the last commit, and its cost counted
    /// again. The settings from the config are left as the log has them.
    pub async fn resume(&mut self, events: Vec<Event>) -> Result<SessionEnd, AutopilotError> {
        self.session = Session::replay(&events).map_err(failed("replaying the event log"))?;
        self.events = events;
        self.prepare_to_redo()?;
        let effects = self.session.pending();
        self.carry_out_from(effects).await
    }

    /// Records in the log why the run ended, unless the Session has finished, was aborted, or never
    /// started.
    pub fn record_run_end(&mut self, outcome: RunOutcome) -> Result<(), AutopilotError> {
        if matches!(
            self.session.phase(),
            Phase::NotStarted | Phase::Done | Phase::Aborted
        ) {
            return Ok(());
        }
        self.apply(Event::RunEnded { outcome }).map(drop)
    }

    async fn carry_out_from(
        &mut self,
        mut effects: Vec<Effect>,
    ) -> Result<SessionEnd, AutopilotError> {
        loop {
            let [effect] = effects.as_slice() else {
                return Err(AutopilotError {
                    step: "reading the Session's next effect",
                    message: format!("expected one effect, got {effects:?}"),
                });
            };
            let events = match self.carry_out(effect.clone()).await? {
                Step::Events(events) => events,
                Step::End(end) => return Ok(end),
            };
            for event in events {
                effects = self.apply(event)?;
            }
        }
    }

    async fn carry_out(&mut self, effect: Effect) -> Result<Step, AutopilotError> {
        let events = match effect {
            Effect::RunPlanner { feedback } => return self.plan(feedback).await,
            Effect::ShowPlanCheckpoint => {
                let plan = self.session_plan()?.clone();
                return self.checkpoint(Checkpoint::Plan { plan }).await;
            }
            Effect::RunScaffolder { task, feedback } => return self.scaffold(task, feedback).await,
            Effect::RunScaffoldChecks { .. } => return self.scaffold_checks().await,
            Effect::ShowScaffoldCheckpoint { task } => {
                let work_units = self.session.work_units().to_vec();
                return self
                    .checkpoint(Checkpoint::Scaffold { task, work_units })
                    .await;
            }
            Effect::ShowRevisionCheckpoint { task, revision } => {
                return self
                    .checkpoint(Checkpoint::Revision { task, revision })
                    .await;
            }
            Effect::ShowEscalationCheckpoint { task, cause } => {
                return self.escalation(task, cause).await;
            }
            Effect::RunReplan {
                task,
                cause,
                feedback,
            } => return self.replan(task, cause, feedback).await,
            Effect::ShowReplanCheckpoint { task, replan } => {
                return self.checkpoint(Checkpoint::Replan { task, replan }).await;
            }
            Effect::ResetTask { .. } => {
                let reset = reset_task(self.worktree, self.session.task_commits());
                vec![reset.map_err(failed("resetting the Task"))?]
            }
            Effect::CommitSkeleton { task } => {
                let skeleton = commit_skeleton(self.worktree, self.task(task)?);
                vec![skeleton.map_err(failed("committing the Skeleton"))?]
            }
            Effect::RunWriter { id } => return self.write(id).await,
            Effect::CommitWorkUnit { id } => {
                let commit = self.passed_commit.take().ok_or_else(|| AutopilotError {
                    step: "committing the Work Unit",
                    message: format!("{id:?} has no passed commit"),
                })?;
                self.notify_committed(id, &commit);
                vec![Event::WorkUnitCommitted {
                    id,
                    commit: commit.as_str().to_owned(),
                }]
            }
            Effect::RunTaskChecks { .. } if std::mem::take(&mut self.task_fix_checked) => {
                vec![Event::TaskChecksPassed]
            }
            Effect::RunTaskChecks { .. } => {
                self.working(None, None, "Running the Task's Unit Check".to_owned());
                let task_check = run_task_check(self.worktree.path(), &self.config.checks).await;
                vec![task_check.map_err(failed("running the Task's Checks"))?]
            }
            Effect::RunRevision { task, cause } => return self.revise(task, cause).await,
            Effect::CommitRevision { task } => {
                let revision = commit_revision(self.worktree, self.task(task)?);
                vec![revision.map_err(failed("committing the Revision"))?]
            }
            Effect::DiscardRevision { .. } => {
                let discarded = discard_revision(self.worktree);
                vec![discarded.map_err(failed("discarding the Revision"))?]
            }
            Effect::CheckMarkers => {
                self.working(
                    None,
                    None,
                    "Checking the branch for leftover Markers".to_owned(),
                );
                let markers = check_markers(self.worktree, self.mainline);
                vec![markers.map_err(failed("checking for Markers"))?]
            }
            Effect::ShowMarkerCheckpoint { remaining } => {
                return self.checkpoint(Checkpoint::Markers { remaining }).await;
            }
            Effect::ShowBudgetCheckpoint => vec![self.budget_checkpoint().await?],
            Effect::EndSession if *self.session.phase() == Phase::Stopped => {
                return Ok(Step::End(SessionEnd::StoppedAtBudget));
            }
            Effect::EndSession if *self.session.phase() == Phase::Aborted => {
                return Ok(Step::End(SessionEnd::Aborted));
            }
            Effect::EndSession if !self.session.skipped_tasks().is_empty() => {
                return Ok(Step::End(SessionEnd::FinishedWithSkippedTasks {
                    skipped_tasks: self.session.skipped_tasks().to_vec(),
                }));
            }
            Effect::EndSession => return Ok(Step::End(SessionEnd::Finished)),
        };
        Ok(Step::Events(events))
    }

    /// Waits for the Operator's Decision at `checkpoint` and turns it into the events that record it.
    async fn checkpoint(&mut self, checkpoint: Checkpoint) -> Result<Step, AutopilotError> {
        let decision = self.operator.decide(&checkpoint).await;
        let decided = match (&checkpoint, decision) {
            (_, Decision::Stop) => return Ok(self.stopped_here()),
            (_, Decision::Reject(feedback)) => Event::CheckpointRejected { feedback },
            (Checkpoint::Plan { plan }, Decision::Approve) => Event::PlanApproved {
                task_order: (0..plan.tasks.len()).collect(),
            },
            (Checkpoint::Replan { replan, .. }, Decision::Approve) => Event::PlanApproved {
                task_order: (0..replan.tasks.len()).collect(),
            },
            (
                Checkpoint::Plan { .. } | Checkpoint::Replan { .. },
                Decision::ApproveTasks(task_order),
            ) => Event::PlanApproved { task_order },
            (Checkpoint::Plan { .. }, Decision::EditPlan(plan)) => {
                validate_plan(&plan).map_err(|problems| AutopilotError {
                    step: "reading the edited Plan",
                    message: problems,
                })?;
                Event::PlanEdited { plan }
            }
            (Checkpoint::Scaffold { .. }, Decision::Approve) => Event::ScaffoldApproved,
            (Checkpoint::Revision { .. }, Decision::Approve) => Event::RevisionApproved,
            (Checkpoint::Markers { .. }, Decision::Approve) => Event::MarkersApproved,
            (Checkpoint::Markers { .. }, Decision::Fixed) => {
                let message = CommitMessage::new(MARKER_FIX_SUBJECT)
                    .map_err(failed("committing the Marker fix"))?;
                commit_operator_fix(self.worktree, &message)
                    .map_err(failed("committing the Marker fix"))?
            }
            (_, decision) => return Err(misplaced(&checkpoint, &decision)),
        };
        Ok(Step::Events(vec![decided]))
    }

    /// Asks the Operator at the Escalation Checkpoint until they choose how the Session goes on, or
    /// fix it by hand in a way that passes the Checks.
    async fn escalation(
        &mut self,
        task: usize,
        cause: RevisionCause,
    ) -> Result<Step, AutopilotError> {
        let later_tasks: Vec<String> = self
            .plan_tasks()?
            .iter()
            .skip(task + 1)
            .map(|later| later.title.clone())
            .collect();
        let stronger_writer = match &cause {
            RevisionCause::WorkUnitFailed(failure) => {
                Some(self.stronger_writer_offer(failure).await?)
            }
            RevisionCause::TaskChecksFailed { .. } => None,
        };
        let mut failed_fix = None;
        loop {
            let checkpoint = Checkpoint::Escalation {
                task,
                cause: cause.clone(),
                may_replan: self.session.may_replan(task),
                failed_fix: failed_fix.take(),
                later_tasks: later_tasks.clone(),
                stronger_writer: stronger_writer.clone(),
            };
            let chosen = match self.operator.decide(&checkpoint).await {
                Decision::Stop => return Ok(self.stopped_here()),
                Decision::Replan if self.session.may_replan(task) => Event::ReplanRequested,
                Decision::RetryWithHint { tier, hint } => Event::RetryWithHint { tier, hint },
                Decision::RetryWithStrongerWriter if stronger_writer.is_some() => {
                    let model = stronger_writer.clone().map(|offer| offer.model);
                    Event::StrongerWriterRequested {
                        model: model.unwrap_or_default(),
                    }
                }
                Decision::Skip => Event::TaskSkipped,
                Decision::Abort => Event::SessionAborted,
                Decision::Fixed => match self.check_fix(&cause).await? {
                    Some(check_output) => {
                        failed_fix = Some(check_output);
                        continue;
                    }
                    None => {
                        self.task_fix_checked = cause.work_unit().is_none();
                        self.commit_fix(&cause)?
                    }
                },
                decision => return Err(misplaced(&checkpoint, &decision)),
            };
            return Ok(Step::Events(vec![chosen]));
        }
    }

    /// The Scaffolder's model in the Writer role (grilling log, Round 3 decision 2), priced for the
    /// failed Work Unit's prompt with its Failure Brief.
    async fn stronger_writer_offer(
        &self,
        failure: &WorkUnitFailure,
    ) -> Result<StrongerWriterOffer, AutopilotError> {
        let model = self.config.roles.scaffolder.model.clone();
        let pricing = self.model(&model).await?.pricing;
        let work_unit = self.work_unit(failure.id)?;
        let prompt = self
            .prompts
            .writer(&WriterInputs {
                work_unit: &work_unit,
            })
            .map_err(failed("rendering the Writer prompt"))?;
        let brief = failure_brief(
            failure,
            self.session.revisions(),
            self.config.failure_brief_tokens,
        );
        let input_tokens = estimate_tokens(&prompt.text()) + estimate_tokens(&brief);
        let dollars = f64::from(input_tokens) * pricing.prompt
            + f64::from(ESTIMATED_WRITER_REPLY_TOKENS) * pricing.completion;
        Ok(StrongerWriterOffer {
            model,
            cost_per_attempt: Usd::from_dollars(dollars),
            max_attempts: MAX_WRITER_ATTEMPTS,
        })
    }

    /// The hint and Failure Brief a Writer gets when its Work Unit is retried from the Escalation
    /// Checkpoint.
    fn escalation_notes(
        &self,
        id: WorkUnitId,
        stronger: Option<&StrongerWriter>,
    ) -> Option<String> {
        let hint = self
            .session
            .hint_for(Tier::Writer, Some(id))
            .map(hint_input);
        let brief = stronger.map(|stronger| {
            failure_brief(
                &stronger.failure,
                self.session.revisions(),
                self.config.failure_brief_tokens,
            )
        });
        let notes: Vec<String> = hint.into_iter().chain(brief).collect();
        (!notes.is_empty()).then(|| notes.join("\n\n"))
    }

    /// The output of the first Check the Human Operator's fix fails, or `None` when it passes.
    async fn check_fix(&mut self, cause: &RevisionCause) -> Result<Option<String>, AutopilotError> {
        let test_filter = match cause.work_unit() {
            Some(id) => Some(self.work_unit(id)?.test_filter),
            None => None,
        };
        self.working(None, None, "Checking your fix".to_owned());
        check_operator_fix(
            self.worktree.path(),
            &self.config.checks,
            test_filter.as_deref(),
        )
        .await
        .map_err(failed("checking the fix"))
    }

    /// Commits the fix by hand. The Work Unit it fixes loses its Markers, as when a Writer's code
    /// passes, so the Operator needn't remove them to get past the Marker Checkpoint.
    fn commit_fix(&self, cause: &RevisionCause) -> Result<Event, AutopilotError> {
        if let Some(id) = cause.work_unit() {
            let work_unit = self.work_unit(id)?;
            strip_work_unit_markers(self.worktree.path(), &work_unit.file, id)
                .map_err(failed("removing the fixed Work Unit's Markers"))?;
        }
        let message = CommitMessage::new(TASK_FIX_SUBJECT).map_err(failed("committing the fix"))?;
        commit_operator_fix(self.worktree, &message).map_err(failed("committing the fix"))
    }

    /// The configured Clarification Mode, or the one the Human Operator last switched to. A switch
    /// since the last Tier run is recorded first.
    fn clarification_mode(&mut self) -> Result<ClarificationMode, AutopilotError> {
        let current = self
            .session
            .clarification_mode()
            .unwrap_or(self.config.clarification_mode);
        match self.operator.clarification_mode_change() {
            Some(mode) if mode != current => {
                self.apply(Event::ClarificationModeSet { mode })?;
                Ok(mode)
            }
            _ => Ok(current),
        }
    }

    /// The settings from the config that the event log records, applied as the Session starts.
    fn configured_settings(&self) -> Vec<Event> {
        let mut settings = Vec::new();
        if self.config.revision_checkpoints {
            settings.push(Event::RevisionCheckpointsSet { enabled: true });
        }
        if let Some(budget_cap_usd) = self.config.budget_cap_usd {
            settings.push(Event::BudgetCapSet {
                cap: Some(Usd::from_dollars(budget_cap_usd)),
            });
        }
        settings
    }

    /// Asks the Operator at the Budget Checkpoint until they set a cap above what the Session has
    /// spent, or stop. Returns `BudgetCapSet` or `SessionStopped`.
    async fn budget_checkpoint(&mut self) -> Result<Event, AutopilotError> {
        let spent = self.session.usage().total().cost;
        let cap = self.session.budget_cap().unwrap_or(spent);
        loop {
            let checkpoint = Checkpoint::Budget { cap, spent };
            match self.operator.decide(&checkpoint).await {
                Decision::SetBudgetCap(new_cap)
                    if new_cap.is_none_or(|new_cap| new_cap > spent) =>
                {
                    return Ok(Event::BudgetCapSet { cap: new_cap });
                }
                Decision::SetBudgetCap(_) => {}
                Decision::Stop => return Ok(Event::SessionStopped),
                decision => return Err(misplaced(&checkpoint, &decision)),
            }
        }
    }

    /// Between two calls of one run, waits at the Budget Checkpoint if the last call reached the
    /// cap. Once the cap is raised the run goes on where it was; `Some` means the Operator
    /// stopped.
    async fn pause_at_budget(&mut self) -> Result<Option<SessionEnd>, AutopilotError> {
        if !self.session.is_at_budget_checkpoint() {
            return Ok(None);
        }
        let decided = self.budget_checkpoint().await?;
        let stopped = decided == Event::SessionStopped;
        self.apply(decided)?;
        Ok(stopped.then_some(SessionEnd::StoppedAtBudget))
    }

    /// Ends the run where the Session waits, so a later resume shows the same Checkpoint again.
    fn stopped_here(&self) -> Step {
        let waiting = self.session.pending().into_iter().next();
        Step::End(SessionEnd::NeedsOperator(
            waiting.unwrap_or(Effect::EndSession),
        ))
    }

    /// Tells the observer the diff of the Work Unit's commit, which is `HEAD` just after it passed.
    fn notify_committed(&mut self, work_unit: WorkUnitId, commit: &CommitId) {
        if self.observer.is_none() {
            return;
        }
        let diff = self
            .worktree
            .parent_of(commit)
            .and_then(|parent| self.worktree.committed_diff_from(&parent));
        if let Ok(diff) = diff {
            self.notify(AutopilotUpdate::Committed { work_unit, diff });
        }
    }

    fn apply(&mut self, event: Event) -> Result<Vec<Effect>, AutopilotError> {
        let effects = self
            .session
            .apply(&event)
            .map_err(failed("applying an event"))?;
        if let Some(event_log) = &mut self.event_log {
            event_log
                .append(&event)
                .map_err(failed("writing the event log"))?;
        }
        self.events.push(event.clone());
        self.notify(AutopilotUpdate::Event(event));
        Ok(effects)
    }

    async fn model(&self, id: &str) -> Result<ModelInfo, AutopilotError> {
        self.client
            .model(id)
            .await
            .map_err(failed("looking up the model"))
    }

    fn request(&self) -> Result<Request, AutopilotError> {
        self.session.request().cloned().ok_or(AutopilotError {
            step: "reading the Request",
            message: "the Session has no Request".to_owned(),
        })
    }

    /// Not called `plan`: that runs the Planner.
    fn session_plan(&self) -> Result<&Plan, AutopilotError> {
        self.session.plan().ok_or(AutopilotError {
            step: "reading the Plan",
            message: "the Session has no Plan".to_owned(),
        })
    }

    fn plan_tasks(&self) -> Result<&[Task], AutopilotError> {
        Ok(&self.session_plan()?.tasks)
    }

    fn task(&self, task: usize) -> Result<&Task, AutopilotError> {
        self.plan_tasks()?.get(task).ok_or(AutopilotError {
            step: "reading the Plan",
            message: format!("the Plan has no Task {task}"),
        })
    }

    fn work_unit(&self, id: WorkUnitId) -> Result<WorkUnit, AutopilotError> {
        self.session
            .work_units()
            .iter()
            .find(|unit| unit.id == id)
            .cloned()
            .ok_or(AutopilotError {
                step: "reading the Work Unit",
                message: format!("the Session has no {id:?}"),
            })
    }

    fn tools(&self, toolset: Toolset) -> Tools {
        Tools::new(self.worktree.path(), toolset, self.config.checks.clone())
    }

    /// With `feedback`, the Planner sees the Plan the Human Operator rejected and why.
    async fn plan(&mut self, feedback: Option<String>) -> Result<Step, AutopilotError> {
        let request = self.request()?;
        let rejected_plan = self.session.plan().cloned();
        let mode = self.clarification_mode()?;
        let inputs = PlannerInputs {
            request: &request,
            clarification_mode: mode,
            replan: None,
        };
        let prompt = self
            .prompts
            .planner(&self.conventions, &inputs)
            .map_err(failed("rendering the Planner prompt"))?;
        let role = self.config.roles.planner.clone();
        let agent = PlannerRun {
            prompt: &prompt,
            model: self.model(&role.model).await?,
            temperature: role.temperature,
            clarification_mode: mode,
            tools: self.tools(Toolset::Planner),
            rejected: rejected_plan
                .as_ref()
                .zip(feedback.as_deref())
                .map(|(plan, feedback)| Rejection { plan, feedback }),
        }
        .start()
        .map_err(failed("starting the Planner"))?;
        let label = if feedback.is_some() {
            "Planning again with feedback"
        } else {
            "Planning"
        };
        let submitted = self
            .submitted(Tier::Planner, &role.model, label, agent)
            .await?;
        let (plan, usage) = match submitted {
            Submitted::Accepted(plan, usage) => (plan, usage),
            Submitted::Stopped(end) => return Ok(Step::End(end)),
        };
        Ok(Step::Events(vec![Event::PlanSubmitted { plan }, usage]))
    }

    /// With `feedback`, the Scaffolder revises the Skeleton the Human Operator rejected, in place.
    async fn scaffold(
        &mut self,
        task: usize,
        feedback: Option<String>,
    ) -> Result<Step, AutopilotError> {
        let request = self.request()?;
        let plan = self.session_plan()?.clone();
        let mode = self.clarification_mode()?;
        let cap_tokens = self.config.writer_cap_tokens;
        let inputs = ScaffolderInputs {
            request: &request,
            plan: &plan,
            task: self.task(task)?,
            clarification_mode: mode,
            cap_tokens,
            revision: None,
        };
        let prompt = self
            .prompts
            .scaffolder(&self.conventions, &inputs)
            .map_err(failed("rendering the Scaffolder prompt"))?;
        let role = self.config.roles.scaffolder.clone();
        let agent = ScaffolderRun {
            prompt: &prompt,
            model: self.model(&role.model).await?,
            temperature: role.temperature,
            clarification_mode: mode,
            worktree: self.worktree.path(),
            checks: self.config.checks.clone(),
            cap_tokens,
            first_id: self.session.next_work_unit_id(),
            feedback: feedback.as_deref(),
        }
        .start()
        .map_err(failed("starting the Scaffolder"))?;
        let again = if feedback.is_some() {
            " again with feedback"
        } else {
            ""
        };
        let submitted = self
            .submitted(
                Tier::Scaffolder,
                &role.model,
                &format!("Scaffolding Task {}{again}", task + 1),
                agent,
            )
            .await?;
        let (submitted_units, usage) = match submitted {
            Submitted::Accepted(submitted_units, usage) => (submitted_units, usage),
            Submitted::Stopped(end) => return Ok(Step::End(end)),
        };
        let work_units = submitted_units.work_units;
        Ok(Step::Events(vec![
            Event::WorkUnitsSubmitted { work_units },
            usage,
        ]))
    }

    /// The submission already ran the Scaffold Check; this runs it again for the effect.
    async fn scaffold_checks(&mut self) -> Result<Step, AutopilotError> {
        self.working(None, None, "Running the Scaffold Check".to_owned());
        let check_output = run_check(
            self.worktree.path(),
            &self.config.checks,
            CheckName::Scaffold,
            None,
        )
        .await
        .map_err(failed("running the Scaffold Check"))?;
        Ok(if check_output.is_error {
            Step::End(SessionEnd::ScaffoldChecksFailed {
                check_output: check_output.text,
            })
        } else {
            Step::Events(vec![Event::ScaffoldChecksPassed])
        })
    }

    /// Runs the Writer until its Work Unit passes or the Writer gives up.
    async fn write(&mut self, id: WorkUnitId) -> Result<Step, AutopilotError> {
        let work_unit = self.work_unit(id)?;
        let prompt = self
            .prompts
            .writer(&WriterInputs {
                work_unit: &work_unit,
            })
            .map_err(failed("rendering the Writer prompt"))?;
        let stronger = self.session.stronger_writer(id).cloned();
        let escalation_notes = self.escalation_notes(id, stronger.as_ref());
        let role = match &stronger {
            Some(stronger) => stronger_writer_role(&self.config.roles.scaffolder, &stronger.model),
            None => self.config.roles.writer.clone(),
        };
        let model_id = role.model;
        let supported_parameters = self.model(&model_id).await?.supported_parameters;
        let mut writer = WriterRun {
            prompt: &prompt,
            work_unit: &work_unit,
            model: model_id.clone(),
            temperature: role.temperature,
            supported_parameters,
            worktree: self.worktree.path(),
            checks: self.config.checks.clone(),
            cap_tokens: self.config.writer_cap_tokens,
            missing_response_allowed: self.session.may_send_missing_response(id),
            escalation_notes,
        }
        .start();
        self.writer_runs.push(WriterRunAttempts {
            work_unit: id,
            attempts: Vec::new(),
        });
        let mut attempt_number = 0;
        loop {
            attempt_number += 1;
            self.working(
                Some(Tier::Writer),
                Some(&model_id),
                format!("Writing {id}, attempt {attempt_number}"),
            );
            let client = self.client;
            let mut checks_started = || {
                self.working(
                    None,
                    None,
                    format!("Checking {id}, attempt {attempt_number}"),
                );
            };
            let attempt = match writer
                .attempt_reporting_checks(client, &mut checks_started)
                .await
            {
                Ok(attempt) => attempt,
                Err(err) => return self.region_failure(&writer, err).map(Step::Events),
            };
            if let Some(run) = self.writer_runs.last_mut() {
                run.attempts.push(attempt.clone());
            }
            self.notify(AutopilotUpdate::WriterAttempt {
                work_unit: id,
                attempt: attempt.clone(),
            });
            let usage = Event::UsageRecorded {
                tier: Tier::Writer,
                usage: attempt.usage,
            };
            if let AttemptOutcome::Passed { commit, .. } = attempt.outcome {
                // The Writer committed already, so `CommitWorkUnit` only records it.
                self.passed_commit = Some(commit);
                let passed = Event::WorkUnitPassed {
                    id,
                    model: Some(attempt.model),
                };
                return Ok(Step::Events(vec![passed, usage]));
            }
            if let Some(failure) = writer.work_unit_failure() {
                return Ok(Step::Events(vec![Event::WorkUnitFailed { failure }, usage]));
            }
            self.apply(usage)?;
            if let Some(end) = self.pause_at_budget().await? {
                return Ok(Step::End(end));
            }
        }
    }

    /// A broken Region ends the Writer with a `WorkUnitFailed` that goes to a Revision; any other
    /// Writer error ends the run.
    fn region_failure(
        &mut self,
        writer: &Writer,
        err: AttemptError,
    ) -> Result<Vec<Event>, AutopilotError> {
        let (WriterError::RegionBroken { .. }, Some(failure)) =
            (&err.error, writer.work_unit_failure())
        else {
            return Err(failed("running the Writer")(err));
        };
        let mut events = vec![Event::WorkUnitFailed { failure }];
        events.extend(err.usage.map(|usage| Event::UsageRecorded {
            tier: Tier::Writer,
            usage,
        }));
        Ok(events)
    }

    async fn revise(&mut self, task: usize, cause: RevisionCause) -> Result<Step, AutopilotError> {
        let request = self.request()?;
        let plan = self.session_plan()?.clone();
        let work_units = self.session.work_units().to_vec();
        let earlier = self.session.revisions().to_vec();
        let hint = self
            .session
            .hint_for(Tier::Scaffolder, None)
            .map(str::to_owned);
        let diff = task_diff(self.worktree, self.session.task_commits())
            .map_err(failed("reading the Task diff"))?;
        let mode = self.clarification_mode()?;
        let cap_tokens = self.config.writer_cap_tokens;
        let inputs = ScaffolderInputs {
            request: &request,
            plan: &plan,
            task: self.task(task)?,
            clarification_mode: mode,
            cap_tokens,
            revision: Some(RevisionInputs {
                cause: &cause,
                task_diff: &diff,
                work_units: &work_units,
                earlier: &earlier,
            }),
        };
        let prompt = self
            .prompts
            .scaffolder(&self.conventions, &inputs)
            .map_err(failed("rendering the Revision prompt"))?;
        let role = self.config.roles.scaffolder.clone();
        let agent = RevisionRun {
            prompt: &prompt,
            model: self.model(&role.model).await?,
            temperature: role.temperature,
            clarification_mode: mode,
            worktree: self.worktree.path(),
            checks: self.config.checks.clone(),
            cap_tokens,
            cause: &cause,
            task_work_units: &work_units,
            first_new_id: self.session.next_work_unit_id(),
            tool_call_budget: self.config.revision_tool_call_budget,
            hint: hint.as_deref(),
        }
        .start()
        .map_err(failed("starting the Revision"))?;
        let submitted = self
            .submitted(
                Tier::Scaffolder,
                &role.model,
                &format!("Revising Task {}", task + 1),
                agent,
            )
            .await?;
        let (revision, usage) = match submitted {
            Submitted::Accepted(revision, usage) => (revision, usage),
            Submitted::Stopped(end) => return Ok(Step::End(end)),
        };
        let reviewed = review_revision(self.worktree.path(), &work_units, &cause, revision)
            .map_err(failed("reviewing the Revision"))?;
        Ok(Step::Events(vec![reviewed, usage]))
    }

    /// The Planner reworks the failed Task from its failure chain. With `feedback`, the Human
    /// Operator rejected its last Replan.
    async fn replan(
        &mut self,
        task: usize,
        cause: RevisionCause,
        feedback: Option<String>,
    ) -> Result<Step, AutopilotError> {
        let request = self.request()?;
        let plan = self.session_plan()?.clone();
        let revisions = self.session.revisions().to_vec();
        let hint = self
            .session
            .hint_for(Tier::Planner, None)
            .map(str::to_owned);
        let diff = task_diff(self.worktree, self.session.task_commits())
            .map_err(failed("reading the Task diff"))?;
        let mode = self.clarification_mode()?;
        let inputs = PlannerInputs {
            request: &request,
            clarification_mode: mode,
            replan: Some(ReplanInputs {
                plan: &plan,
                task,
                cause: &cause,
                revisions: &revisions,
                task_diff: &diff,
            }),
        };
        let prompt = self
            .prompts
            .planner(&self.conventions, &inputs)
            .map_err(failed("rendering the Replan prompt"))?;
        let role = self.config.roles.planner.clone();
        let agent = ReplanRun {
            prompt: &prompt,
            model: self.model(&role.model).await?,
            temperature: role.temperature,
            clarification_mode: mode,
            tools: self.tools(Toolset::Planner),
            feedback: feedback.as_deref(),
            hint: hint.as_deref(),
        }
        .start()
        .map_err(failed("starting the Replan"))?;
        let again = if feedback.is_some() {
            " again with feedback"
        } else {
            ""
        };
        let label = format!("Replanning Task {}{again}", task + 1);
        let submitted = self
            .submitted(Tier::Planner, &role.model, &label, agent)
            .await?;
        let (replan, usage) = match submitted {
            Submitted::Accepted(replan, usage) => (replan, usage),
            Submitted::Stopped(end) => return Ok(Step::End(end)),
        };
        Ok(Step::Events(vec![Event::ReplanSubmitted { replan }, usage]))
    }

    /// Takes turns until the agent submits or stops. A submission comes with the
    /// `UsageRecorded` of the call that made it, which goes in the log after the submission's own
    /// event.
    async fn submitted<T>(
        &mut self,
        tier: Tier,
        model: &str,
        label: &str,
        mut agent: Agent<T>,
    ) -> Result<Submitted<T>, AutopilotError>
    where
        T: DeserializeOwned + JsonSchema + fmt::Debug,
    {
        let mut turn_number = 0;
        loop {
            turn_number += 1;
            self.working(
                Some(tier),
                Some(model),
                format!("{label}, turn {turn_number}"),
            );
            let turn = agent
                .turn(self.client)
                .await
                .map_err(failed("calling the model"))?;
            let usage = Event::UsageRecorded {
                tier,
                usage: turn.usage,
            };
            let reason = match turn.outcome {
                None => {
                    self.apply(usage)?;
                    if let Some(end) = self.pause_at_budget().await? {
                        return Ok(Submitted::Stopped(end));
                    }
                    continue;
                }
                Some(Outcome::Submitted(submission)) => {
                    return Ok(Submitted::Accepted(submission, usage));
                }
                Some(Outcome::Questions(questions)) => {
                    let answers = self.operator.answer(tier, &questions).await;
                    match answers {
                        Some(answers) => {
                            self.apply(usage)?;
                            agent
                                .answer(&answers)
                                .map_err(failed("passing on the Operator's answers"))?;
                            if let Some(end) = self.pause_at_budget().await? {
                                return Ok(Submitted::Stopped(end));
                            }
                            continue;
                        }
                        None => format!("asked questions: {questions}"),
                    }
                }
                Some(Outcome::Escalated { failures }) => {
                    format!("escalated after: {}", failures.join("; "))
                }
            };
            self.apply(usage)?;
            return Ok(Submitted::Stopped(SessionEnd::TierStopped { tier, reason }));
        }
    }
}

/// The reply length a stronger Writer's estimated cost assumes: a Region with its context lines.
const ESTIMATED_WRITER_REPLY_TOKENS: u32 = 1_000;

/// The stronger Writer runs at the Scaffolder's temperature, the pair the model probe checked, not
/// the Writer's. A model logged before the Scaffolder's was changed keeps that temperature within
/// its own range.
fn stronger_writer_role(scaffolder: &RoleConfig, model: &str) -> RoleConfig {
    RoleConfig {
        model: model.to_owned(),
        temperature: scaffolder.temperature.clamp(0.0, max_temperature(model)),
    }
}

fn misplaced(checkpoint: &Checkpoint, decision: &Decision) -> AutopilotError {
    AutopilotError {
        step: "reading the Operator's decision",
        message: format!("{decision:?} doesn't fit {checkpoint:?}"),
    }
}

enum Submitted<T> {
    Accepted(T, Event),
    Stopped(SessionEnd),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scaffolder(temperature: f64) -> RoleConfig {
        RoleConfig {
            model: "openai/gpt-6".to_owned(),
            temperature,
        }
    }

    #[test]
    fn a_stronger_writer_takes_the_scaffolders_temperature_with_its_model() {
        let role = stronger_writer_role(&scaffolder(1.5), "openai/gpt-6");
        assert_eq!(role.model, "openai/gpt-6");
        assert_eq!(role.temperature, 1.5);
    }

    #[test]
    fn a_stronger_writer_temperature_stays_in_its_models_range() {
        let role = stronger_writer_role(&scaffolder(1.5), "anthropic/claude-sonnet-5.5");
        assert_eq!(role.temperature, 1.0);
    }
}
