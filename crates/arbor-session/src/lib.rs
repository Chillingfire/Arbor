//! Runs a Session without a Human Operator: applies each event to the pure Session state machine
//! and carries out the effects it asks for with the real Tiers, real git and the project's real
//! Checks. Every Checkpoint is approved as it is, and the run stops where only the Human Operator
//! could decide, such as an escalation Checkpoint or a Tier's clarifying questions.

use std::fmt;
use std::io::Write;

use arbor_config::Config;
use arbor_core::{Effect, Event, EventLog, Request, RevisionCause, Session, Task, Tier, WorkUnitId};
use arbor_git::{CommitId, Mainline, Worktree};
use arbor_llm::{LlmClient, ModelInfo};
use arbor_prompts::{
    Conventions, PlannerInputs, Prompts, RevisionInputs, ScaffolderInputs, WriterInputs,
};
use arbor_tiers::{
    Agent, Attempt, AttemptOutcome, Outcome, PlannerRun, RevisionRun, ScaffolderRun, WriterRun,
    check_markers, commit_revision, commit_skeleton, discard_revision, review_revision,
    run_task_check, task_diff,
};
use arbor_tools::{CheckName, Tools, Toolset, run_check};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;

/// How an [`Autopilot`] run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEnd {
    /// The Session finished: every Task is done and no Markers are left.
    Finished,
    /// The Session asked for something only the Human Operator can do.
    NeedsOperator(Effect),
    /// A Tier ended its run without submitting, by escalating or asking questions.
    TierStopped { tier: Tier, reason: String },
    /// The Scaffold Check failed on a Skeleton the Scaffolder had already checked.
    ScaffoldChecksFailed { check_output: String },
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
    /// Where each event is written before anyone is told of it (ADR-0004).
    event_log: Option<EventLog<Box<dyn Write + 'a>>>,
    observer: Option<Box<dyn FnMut(AutopilotUpdate) + 'a>>,
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
            event_log: None,
            observer: None,
        })
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
            Effect::RunPlanner { feedback: None } => return self.plan().await,
            Effect::ShowPlanCheckpoint => {
                let task_count = self.plan_tasks()?.len();
                vec![Event::PlanApproved {
                    task_order: (0..task_count).collect(),
                }]
            }
            Effect::RunScaffolder {
                task,
                feedback: None,
            } => return self.scaffold(task).await,
            Effect::RunScaffoldChecks { .. } => return self.scaffold_checks().await,
            Effect::ShowScaffoldCheckpoint { .. } => vec![Event::ScaffoldApproved],
            Effect::CommitSkeleton { task } => {
                let skeleton = commit_skeleton(self.worktree, self.task(task)?);
                vec![skeleton.map_err(failed("committing the Skeleton"))?]
            }
            Effect::RunWriter { id } => self.write(id).await?,
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
            Effect::EndSession => return Ok(Step::End(SessionEnd::Finished)),
            needs_operator => return Ok(Step::End(SessionEnd::NeedsOperator(needs_operator))),
        };
        Ok(Step::Events(events))
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

    fn plan_tasks(&self) -> Result<&[Task], AutopilotError> {
        match self.session.plan() {
            Some(plan) => Ok(&plan.tasks),
            None => Err(AutopilotError {
                step: "reading the Plan",
                message: "the Session has no Plan".to_owned(),
            }),
        }
    }

    fn task(&self, task: usize) -> Result<&Task, AutopilotError> {
        self.plan_tasks()?.get(task).ok_or(AutopilotError {
            step: "reading the Plan",
            message: format!("the Plan has no Task {task}"),
        })
    }

    fn tools(&self, toolset: Toolset) -> Tools {
        Tools::new(self.worktree.path(), toolset, self.config.checks.clone())
    }

    async fn plan(&mut self) -> Result<Step, AutopilotError> {
        let request = self.request()?;
        let mode = self.config.clarification_mode;
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
            rejected: None,
        }
        .start()
        .map_err(failed("starting the Planner"))?;
        Ok(
            match self
                .submitted(Tier::Planner, &role.model, "Planning", agent)
                .await?
            {
                Submitted::Accepted(plan, usage) => {
                    Step::Events(vec![Event::PlanSubmitted { plan }, usage])
                }
                Submitted::Stopped(end) => Step::End(end),
            },
        )
    }

    async fn scaffold(&mut self, task: usize) -> Result<Step, AutopilotError> {
        let request = self.request()?;
        let plan = self.session.plan().cloned().ok_or(AutopilotError {
            step: "reading the Plan",
            message: "the Session has no Plan".to_owned(),
        })?;
        let mode = self.config.clarification_mode;
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
            feedback: None,
        }
        .start()
        .map_err(failed("starting the Scaffolder"))?;
        Ok(
            match self
                .submitted(
                    Tier::Scaffolder,
                    &role.model,
                    &format!("Scaffolding Task {}", task + 1),
                    agent,
                )
                .await?
            {
                Submitted::Accepted(submitted, usage) => {
                    let work_units = submitted.work_units;
                    Step::Events(vec![Event::WorkUnitsSubmitted { work_units }, usage])
                }
                Submitted::Stopped(end) => Step::End(end),
            },
        )
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
    async fn write(&mut self, id: WorkUnitId) -> Result<Vec<Event>, AutopilotError> {
        let work_unit = self
            .session
            .work_units()
            .iter()
            .find(|unit| unit.id == id)
            .cloned()
            .ok_or(AutopilotError {
                step: "reading the Work Unit",
                message: format!("the Session has no {id:?}"),
            })?;
        let prompt = self
            .prompts
            .writer(&WriterInputs {
                work_unit: &work_unit,
            })
            .map_err(failed("rendering the Writer prompt"))?;
        let role = self.config.roles.writer.clone();
        let supported_parameters = self.model(&role.model).await?.supported_parameters;
        let mut writer = WriterRun {
            prompt: &prompt,
            work_unit: &work_unit,
            model: role.model.clone(),
            temperature: role.temperature,
            supported_parameters,
            worktree: self.worktree.path(),
            checks: self.config.checks.clone(),
            cap_tokens: self.config.writer_cap_tokens,
            missing_response_allowed: self.session.may_send_missing_response(id),
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
                Some(&role.model),
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
            let attempt = writer
                .attempt_reporting_checks(client, &mut checks_started)
                .await
                .map_err(failed("running the Writer"))?;
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
                return Ok(vec![Event::WorkUnitPassed { id }, usage]);
            }
            if let Some(failure) = writer.work_unit_failure() {
                return Ok(vec![Event::WorkUnitFailed { failure }, usage]);
            }
            self.apply(usage)?;
        }
    }

    async fn revise(&mut self, task: usize, cause: RevisionCause) -> Result<Step, AutopilotError> {
        let request = self.request()?;
        let plan = self.session.plan().cloned().ok_or(AutopilotError {
            step: "reading the Plan",
            message: "the Session has no Plan".to_owned(),
        })?;
        let work_units = self.session.work_units().to_vec();
        let earlier = self.session.revisions().to_vec();
        let diff = task_diff(self.worktree, self.session.task_commits())
            .map_err(failed("reading the Task diff"))?;
        let mode = self.config.clarification_mode;
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
        }
        .start()
        .map_err(failed("starting the Revision"))?;
        Ok(
            match self
                .submitted(
                    Tier::Scaffolder,
                    &role.model,
                    &format!("Revising Task {}", task + 1),
                    agent,
                )
                .await?
            {
                Submitted::Accepted(revision, usage) => {
                    let reviewed = review_revision(self.worktree.path(), &work_units, revision)
                        .map_err(failed("reviewing the Revision"))?;
                    Step::Events(vec![reviewed, usage])
                }
                Submitted::Stopped(end) => Step::End(end),
            },
        )
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
                    continue;
                }
                Some(Outcome::Submitted(submission)) => {
                    return Ok(Submitted::Accepted(submission, usage));
                }
                Some(Outcome::Questions(questions)) => format!("asked questions: {questions}"),
                Some(Outcome::Escalated { failures }) => {
                    format!("escalated after: {}", failures.join("; "))
                }
            };
            self.apply(usage)?;
            return Ok(Submitted::Stopped(SessionEnd::TierStopped { tier, reason }));
        }
    }
}

enum Submitted<T> {
    Accepted(T, Event),
    Stopped(SessionEnd),
}
