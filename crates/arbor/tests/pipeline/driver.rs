//! Drives a Session end to end: applies each event to the pure Session state machine and carries
//! out the effects it asks for with the real Tiers, real git and the sample repo's real Checks.
//! The Human Operator approves every Checkpoint as it is.

use arbor_config::Config;
use arbor_core::{Effect, Event, Request, RevisionCause, Session, Task, Tier, WorkUnitId};
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

use crate::fixture_client::FixtureClient;
use crate::sample_repo::SampleRepo;

pub struct Pipeline<'a> {
    pub sample: &'a SampleRepo,
    pub client: &'a FixtureClient,
    pub session: Session,
    /// The event log, oldest first (ADR-0004).
    pub events: Vec<Event>,
    pub writer_attempts: Vec<Attempt>,
    config: Config,
    conventions: Conventions,
    prompts: Prompts,
    /// The commit of the Work Unit that passed and waits for `WorkUnitCommitted`.
    passed_commit: Option<String>,
}

impl<'a> Pipeline<'a> {
    pub fn new(sample: &'a SampleRepo, client: &'a FixtureClient) -> Self {
        let config = Config::load(None, sample.path()).unwrap();
        let conventions = Conventions::load(sample.path(), &config.context_files).unwrap();
        let prompts = Prompts::load(sample.path()).unwrap();
        Self {
            sample,
            client,
            session: Session::new(),
            events: Vec::new(),
            writer_attempts: Vec::new(),
            config,
            conventions,
            prompts,
            passed_commit: None,
        }
    }

    /// Starts a Session for `request` and carries out its effects until it ends.
    pub async fn run(&mut self, request: &str) {
        let request = Request {
            text: request.to_owned(),
            issue: None,
        };
        let mut effects = self.apply(Event::SessionStarted { request });
        loop {
            let [effect] = effects.as_slice() else {
                panic!("expected one effect, got {effects:?}");
            };
            let events = match effect.clone() {
                Effect::RunPlanner { feedback: None } => self.plan().await,
                Effect::ShowPlanCheckpoint => {
                    let task_count = self.session.plan().unwrap().tasks.len();
                    vec![Event::PlanApproved {
                        task_order: (0..task_count).collect(),
                    }]
                }
                Effect::RunScaffolder {
                    task,
                    feedback: None,
                } => self.scaffold(task).await,
                Effect::RunScaffoldChecks { .. } => vec![self.scaffold_checks().await],
                Effect::ShowScaffoldCheckpoint { .. } => vec![Event::ScaffoldApproved],
                Effect::CommitSkeleton { task } => {
                    vec![commit_skeleton(&self.sample.worktree, self.task(task)).unwrap()]
                }
                Effect::RunWriter { id } => self.write(id).await,
                Effect::CommitWorkUnit { id } => vec![Event::WorkUnitCommitted {
                    id,
                    commit: self.passed_commit.take().expect("a Work Unit passed"),
                }],
                Effect::RunTaskChecks { .. } => {
                    let task_check = run_task_check(self.sample.path(), &self.config.checks);
                    vec![task_check.await.unwrap()]
                }
                Effect::RunRevision { task, cause } => self.revise(task, cause).await,
                Effect::CommitRevision { task } => {
                    vec![commit_revision(&self.sample.worktree, self.task(task)).unwrap()]
                }
                Effect::DiscardRevision { .. } => {
                    vec![discard_revision(&self.sample.worktree).unwrap()]
                }
                Effect::CheckMarkers => {
                    let markers = check_markers(&self.sample.worktree, &self.sample.mainline);
                    vec![markers.unwrap()]
                }
                Effect::EndSession => return,
                unscripted => panic!("the scenario doesn't script {unscripted:?}"),
            };
            for event in events {
                effects = self.apply(event);
            }
        }
    }

    fn apply(&mut self, event: Event) -> Vec<Effect> {
        let effects = self
            .session
            .apply(&event)
            .unwrap_or_else(|err| panic!("{err}"));
        self.events.push(event);
        effects
    }

    async fn model(&self, id: &str) -> ModelInfo {
        self.client.model(id).await.unwrap()
    }

    fn request(&self) -> Request {
        self.session.request().unwrap().clone()
    }

    fn task(&self, task: usize) -> &Task {
        &self.session.plan().unwrap().tasks[task]
    }

    async fn plan(&mut self) -> Vec<Event> {
        let request = self.request();
        let mode = self.config.clarification_mode;
        let inputs = PlannerInputs {
            request: &request,
            clarification_mode: mode,
            replan: None,
        };
        let prompt = self.prompts.planner(&self.conventions, &inputs).unwrap();
        let role = self.config.roles.planner.clone();
        let agent = PlannerRun {
            prompt: &prompt,
            model: self.model(&role.model).await,
            temperature: role.temperature,
            clarification_mode: mode,
            tools: Tools::new(
                self.sample.path(),
                Toolset::Planner,
                self.config.checks.clone(),
            ),
            rejected: None,
        }
        .start()
        .unwrap();
        let (plan, usage) = self.submitted(Tier::Planner, agent).await;
        vec![Event::PlanSubmitted { plan }, usage]
    }

    async fn scaffold(&mut self, task: usize) -> Vec<Event> {
        let request = self.request();
        let plan = self.session.plan().unwrap().clone();
        let mode = self.config.clarification_mode;
        let cap_tokens = self.config.writer_cap_tokens;
        let inputs = ScaffolderInputs {
            request: &request,
            plan: &plan,
            task: &plan.tasks[task],
            clarification_mode: mode,
            cap_tokens,
            revision: None,
        };
        let prompt = self.prompts.scaffolder(&self.conventions, &inputs).unwrap();
        let role = self.config.roles.scaffolder.clone();
        let agent = ScaffolderRun {
            prompt: &prompt,
            model: self.model(&role.model).await,
            temperature: role.temperature,
            clarification_mode: mode,
            worktree: self.sample.path(),
            checks: self.config.checks.clone(),
            cap_tokens,
            first_id: self.session.next_work_unit_id(),
            feedback: None,
        }
        .start()
        .unwrap();
        let (submitted, usage) = self.submitted(Tier::Scaffolder, agent).await;
        let work_units = submitted.work_units;
        vec![Event::WorkUnitsSubmitted { work_units }, usage]
    }

    /// The submission already ran the Scaffold Check; this runs it again for the effect.
    async fn scaffold_checks(&mut self) -> Event {
        let check_output = run_check(
            self.sample.path(),
            &self.config.checks,
            CheckName::Scaffold,
            None,
        )
        .await
        .unwrap();
        assert!(!check_output.is_error, "{}", check_output.text);
        Event::ScaffoldChecksPassed
    }

    /// Runs the Writer until its Work Unit passes or the Writer gives up.
    async fn write(&mut self, id: WorkUnitId) -> Vec<Event> {
        let work_unit = self
            .session
            .work_units()
            .iter()
            .find(|unit| unit.id == id)
            .unwrap()
            .clone();
        let prompt = self
            .prompts
            .writer(&WriterInputs {
                work_unit: &work_unit,
            })
            .unwrap();
        let role = self.config.roles.writer.clone();
        let mut writer = WriterRun {
            prompt: &prompt,
            work_unit: &work_unit,
            model: role.model,
            temperature: role.temperature,
            worktree: self.sample.path(),
            checks: self.config.checks.clone(),
            cap_tokens: self.config.writer_cap_tokens,
            missing_response_allowed: self.session.may_send_missing_response(id),
        }
        .start();
        loop {
            let attempt = writer.attempt(self.client).await.unwrap();
            self.writer_attempts.push(attempt.clone());
            let usage = Event::UsageRecorded {
                tier: Tier::Writer,
                usage: attempt.usage,
            };
            if let AttemptOutcome::Passed { commit } = attempt.outcome {
                // The Writer committed already, so `CommitWorkUnit` only records it.
                self.passed_commit = Some(commit.as_str().to_owned());
                return vec![Event::WorkUnitPassed { id }, usage];
            }
            if let Some(failure) = writer.work_unit_failure() {
                return vec![Event::WorkUnitFailed { failure }, usage];
            }
            self.apply(usage);
        }
    }

    async fn revise(&mut self, task: usize, cause: RevisionCause) -> Vec<Event> {
        let request = self.request();
        let plan = self.session.plan().unwrap().clone();
        let work_units = self.session.work_units().to_vec();
        let earlier = self.session.revisions().to_vec();
        let diff = task_diff(&self.sample.worktree, self.session.task_commits()).unwrap();
        let mode = self.config.clarification_mode;
        let cap_tokens = self.config.writer_cap_tokens;
        let inputs = ScaffolderInputs {
            request: &request,
            plan: &plan,
            task: &plan.tasks[task],
            clarification_mode: mode,
            cap_tokens,
            revision: Some(RevisionInputs {
                cause: &cause,
                task_diff: &diff,
                work_units: &work_units,
                earlier: &earlier,
            }),
        };
        let prompt = self.prompts.scaffolder(&self.conventions, &inputs).unwrap();
        let role = self.config.roles.scaffolder.clone();
        let agent = RevisionRun {
            prompt: &prompt,
            model: self.model(&role.model).await,
            temperature: role.temperature,
            clarification_mode: mode,
            worktree: self.sample.path(),
            checks: self.config.checks.clone(),
            cap_tokens,
            cause: &cause,
            task_work_units: &work_units,
            first_new_id: self.session.next_work_unit_id(),
            tool_call_budget: self.config.revision_tool_call_budget,
        }
        .start()
        .unwrap();
        let (revision, usage) = self.submitted(Tier::Scaffolder, agent).await;
        let reviewed = review_revision(self.sample.path(), &work_units, revision).unwrap();
        vec![reviewed, usage]
    }

    /// Takes turns until the agent submits. Returns the submission with the `UsageRecorded` of
    /// the call that made it, which goes in the log after the submission's own event.
    async fn submitted<T>(&mut self, tier: Tier, mut agent: Agent<T>) -> (T, Event)
    where
        T: DeserializeOwned + JsonSchema + std::fmt::Debug,
    {
        loop {
            let turn = agent.turn(self.client).await.unwrap();
            let usage = Event::UsageRecorded {
                tier,
                usage: turn.usage,
            };
            match turn.outcome {
                None => {
                    self.apply(usage);
                }
                Some(Outcome::Submitted(submitted)) => return (submitted, usage),
                Some(other) => panic!("the {tier:?} didn't submit: {other:?}"),
            }
        }
    }
}
