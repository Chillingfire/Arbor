use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::Mutex;

use arbor_config::Config;
use arbor_core::{
    ClarificationMode, Effect, Event, Phase, Plan, Request, RunOutcome, Task, Tier, Usd, WorkUnit,
    WorkUnitId,
};
use arbor_git::{Mainline, Repo, Worktree};
use arbor_llm::{
    BoxFuture, ChatRequest, ChatResponse, FinishReason, LlmClient, LlmError, Message, ModelInfo,
    Pricing, ToolCall, Usage,
};
use arbor_session::{
    Autopilot, AutopilotUpdate, Checkpoint, Decision, Operator, Pending, SessionEnd,
};
use arbor_tiers::commit_subject;
use tempfile::tempdir;

/// A model that answers every call with the same clarifying question and no tool call.
struct AskingModel;

const QUESTION: &str = "Should the flag also print timings?";

impl LlmClient for AskingModel {
    fn chat<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<ChatResponse, LlmError>> {
        let reply = ChatResponse {
            id: "reply-1".to_owned(),
            model: request.model.clone(),
            content: Some(QUESTION.to_owned()),
            tool_calls: Vec::new(),
            finish_reason: FinishReason::Stop,
            usage: Usage {
                input_tokens: 1_000,
                output_tokens: 20,
                cost_usd: 0.0055,
                ..Usage::default()
            },
        };
        Box::pin(async move { Ok(reply) })
    }

    fn model<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ModelInfo, LlmError>> {
        let model_info = ModelInfo {
            id: id.to_owned(),
            context_length: 200_000,
            supported_parameters: vec!["temperature".to_owned(), "tools".to_owned()],
            pricing: Pricing {
                prompt: 0.000005,
                completion: 0.000025,
                cache_read: None,
            },
        };
        Box::pin(async move { Ok(model_info) })
    }
}

fn git(dir: &Path, git_args: &[&str]) {
    let git_output = Command::new("git")
        .args(git_args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        git_output.status.success(),
        "git {git_args:?} failed: {}",
        String::from_utf8_lossy(&git_output.stderr)
    );
}

/// A one-commit project with a Session worktree beside it, both under `dir`.
fn session_worktree(dir: &Path) -> (Worktree, Mainline) {
    let checkout = dir.join("project");
    std::fs::create_dir(&checkout).unwrap();
    std::fs::write(checkout.join("README.md"), "A project.\n").unwrap();
    git(&checkout, &["init", "--quiet", "-b", "main"]);
    git(&checkout, &["config", "user.name", "Test Operator"]);
    git(&checkout, &["config", "user.email", "operator@example.com"]);
    git(&checkout, &["config", "commit.gpgsign", "false"]);
    git(&checkout, &["add", "--all"]);
    git(&checkout, &["commit", "--quiet", "-m", "Start"]);
    let repo = Repo::open(&checkout).unwrap();
    let mainline = repo.mainline(None).unwrap();
    let branch = arbor_git::SessionBranch::for_feature("Add a flag").unwrap();
    let worktree = repo
        .create_worktree(&mainline, &branch, &dir.join("session"))
        .unwrap();
    (worktree, mainline)
}

fn request() -> Request {
    Request {
        text: "Add a --verbose flag".to_owned(),
        issue: None,
    }
}

#[tokio::test]
async fn a_tier_that_asks_questions_ends_the_run_for_the_operator() {
    let dir = tempdir().unwrap();
    let (worktree, mainline) = session_worktree(dir.path());
    let config = Config::load(None, worktree.path()).unwrap();
    let updates = RefCell::new(Vec::new());
    let mut autopilot = Autopilot::new(&worktree, &mainline, &AskingModel, config)
        .unwrap()
        .on_update(|update| updates.borrow_mut().push(update));

    let end = autopilot.run(request()).await.unwrap();

    let SessionEnd::TierStopped { tier, reason } = end else {
        panic!("expected the Planner to stop, got {end:?}");
    };
    assert_eq!(tier, Tier::Planner);
    assert!(reason.contains(QUESTION), "{reason}");
    assert!(matches!(
        autopilot.events(),
        [
            Event::SessionStarted { .. },
            Event::UsageRecorded {
                tier: Tier::Planner,
                ..
            }
        ]
    ));
    let working: Vec<String> = updates
        .borrow()
        .iter()
        .filter_map(|update| match update {
            AutopilotUpdate::Working(activity) => Some(activity.label.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(working, ["Planning, turn 1"]);
    let streamed_events = updates
        .borrow()
        .iter()
        .filter(|update| matches!(update, AutopilotUpdate::Event(_)))
        .count();
    assert_eq!(streamed_events, autopilot.events().len());
    let planner_cost = autopilot.session().usage().tier(Tier::Planner).cost;
    assert_eq!(planner_cost.dollars(), 0.0055);
}

/// An event log whose disk is full.
struct FullDisk;

impl Write for FullDisk {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("no space left on device"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn a_failed_event_log_write_ends_the_run_before_the_event_is_shown() {
    let dir = tempdir().unwrap();
    let (worktree, mainline) = session_worktree(dir.path());
    let config = Config::load(None, worktree.path()).unwrap();
    let updates = RefCell::new(Vec::new());
    let mut autopilot = Autopilot::new(&worktree, &mainline, &AskingModel, config)
        .unwrap()
        .log_events_to(FullDisk)
        .on_update(|update| updates.borrow_mut().push(update));

    let err = autopilot.run(request()).await.unwrap_err();

    assert_eq!(err.step, "writing the event log");
    assert!(err.message.contains("no space left"), "{err}");
    assert!(updates.borrow().is_empty());
}

/// A model that gives the scripted replies in order, keeping each request it was sent.
struct ScriptedModel {
    replies: Mutex<VecDeque<ChatResponse>>,
    requests: Mutex<Vec<ChatRequest>>,
}

impl ScriptedModel {
    fn new(replies: impl IntoIterator<Item = ChatResponse>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().collect()),
            requests: Mutex::default(),
        }
    }

    /// The text of the `index`th request's system message.
    fn system_text(&self, index: usize) -> String {
        self.requests.lock().unwrap()[index]
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::System { content } => Some(content),
                _ => None,
            })
            .flatten()
            .map(|part| part.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The text of every user message in the `index`th request, joined.
    fn user_text(&self, index: usize) -> String {
        self.requests.lock().unwrap()[index]
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::User { content } => Some(content),
                _ => None,
            })
            .flatten()
            .map(|part| part.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl LlmClient for ScriptedModel {
    fn chat<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<ChatResponse, LlmError>> {
        self.requests.lock().unwrap().push(request.clone());
        let scripted_reply = self.replies.lock().unwrap().pop_front();
        Box::pin(async move {
            scripted_reply.ok_or(LlmError::Fixture {
                message: "no scripted reply left".to_owned(),
            })
        })
    }

    fn model<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ModelInfo, LlmError>> {
        AskingModel.model(id)
    }
}

fn model_reply(content: Option<&str>, tool_calls: Vec<ToolCall>) -> ChatResponse {
    ChatResponse {
        id: "reply".to_owned(),
        model: "scripted".to_owned(),
        content: content.map(str::to_owned),
        tool_calls,
        finish_reason: FinishReason::Stop,
        usage: Usage {
            input_tokens: 100,
            output_tokens: 10,
            cost_usd: 0.001,
            ..Usage::default()
        },
    }
}

fn question(question_text: &str) -> ChatResponse {
    model_reply(Some(question_text), Vec::new())
}

fn two_task_plan() -> Plan {
    let task = |title: &str| Task {
        title: title.to_owned(),
        description: format!("{title} in src/main.rs."),
    };
    Plan {
        goal: "Add a --verbose flag".to_owned(),
        constraints: Vec::new(),
        tasks: vec![task("Parse the flag"), task("Print details")],
    }
}

fn submitted_plan(plan: &Plan) -> ChatResponse {
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "submit_plan".to_owned(),
        arguments: serde_json::to_string(plan).unwrap(),
    };
    model_reply(None, vec![call])
}

/// Decides and answers as scripted, noting every Checkpoint and question it was shown.
#[derive(Default)]
struct ScriptedOperator {
    decisions: VecDeque<Decision>,
    answers: VecDeque<Option<String>>,
    shown: Rc<RefCell<Vec<String>>>,
    /// Switched to before the next Tier run asks.
    mode_switch: Option<ClarificationMode>,
}

impl Operator for ScriptedOperator {
    fn decide<'a>(&'a mut self, checkpoint: &'a Checkpoint) -> Pending<'a, Decision> {
        self.shown.borrow_mut().push(format!("{checkpoint:?}"));
        let decision = self.decisions.pop_front().expect("a scripted decision");
        Box::pin(async move { decision })
    }

    fn answer<'a>(&'a mut self, tier: Tier, questions: &'a str) -> Pending<'a, Option<String>> {
        self.shown
            .borrow_mut()
            .push(format!("{tier:?} asked: {questions}"));
        let answers = self.answers.pop_front().expect("scripted answers");
        Box::pin(async move { answers })
    }

    fn clarification_mode_change(&mut self) -> Option<ClarificationMode> {
        self.mode_switch.take()
    }
}

#[tokio::test]
async fn the_operators_answers_and_rejection_feedback_reach_the_planner() {
    let dir = tempdir().unwrap();
    let (worktree, mainline) = session_worktree(dir.path());
    let config = Config::load(None, worktree.path()).unwrap();
    let plan = two_task_plan();
    let model = ScriptedModel::new([
        question(QUESTION),
        submitted_plan(&plan),
        question("Which file holds the flags?"),
    ]);
    let operator = ScriptedOperator {
        decisions: [Decision::Reject("Print details first".to_owned())].into(),
        answers: [Some("No timings".to_owned()), None].into(),
        ..ScriptedOperator::default()
    };
    let shown = Rc::clone(&operator.shown);
    let mut autopilot = Autopilot::new(&worktree, &mainline, &model, config)
        .unwrap()
        .with_operator(operator);

    let end = autopilot.run(request()).await.unwrap();

    assert!(
        matches!(&end, SessionEnd::TierStopped { tier: Tier::Planner, reason }
            if reason.contains("Which file holds the flags?")),
        "{end:?}"
    );
    assert_eq!(
        *shown.borrow(),
        [
            format!("Planner asked: {QUESTION}"),
            format!("{:?}", Checkpoint::Plan { plan }),
            "Planner asked: Which file holds the flags?".to_owned(),
        ]
    );
    let answered = model.user_text(1);
    assert!(
        answered.contains("The Human Operator's answers:\n\nNo timings"),
        "{answered}"
    );
    let rerun = model.user_text(2);
    assert!(rerun.contains("# Rejected Plan"), "{rerun}");
    assert!(
        rerun.contains("Their feedback:\n\nPrint details first"),
        "{rerun}"
    );
    assert!(autopilot.events().contains(&Event::CheckpointRejected {
        feedback: "Print details first".to_owned()
    }));
}

#[tokio::test]
async fn a_clarification_mode_switched_mid_session_is_logged_and_reaches_the_next_tier_run() {
    let dir = tempdir().unwrap();
    let (worktree, mainline) = session_worktree(dir.path());
    let config = Config::load(None, worktree.path()).unwrap();
    assert_eq!(config.clarification_mode, ClarificationMode::Ask);
    let model = ScriptedModel::new([submitted_plan(&two_task_plan())]);
    let operator = ScriptedOperator {
        decisions: [Decision::Stop].into(),
        mode_switch: Some(ClarificationMode::Assume),
        ..ScriptedOperator::default()
    };
    let mut autopilot = Autopilot::new(&worktree, &mainline, &model, config)
        .unwrap()
        .with_operator(operator);

    autopilot.run(request()).await.unwrap();

    assert_eq!(
        autopilot.events()[1],
        Event::ClarificationModeSet {
            mode: ClarificationMode::Assume
        }
    );
    let instructions = model.system_text(0);
    assert!(
        instructions.contains("Clarification Mode is Assume"),
        "{instructions}"
    );
    assert_eq!(
        autopilot.session().clarification_mode(),
        Some(ClarificationMode::Assume)
    );
}

#[tokio::test]
async fn a_plan_edited_at_its_checkpoint_is_shown_again_and_approved_as_edited() {
    let dir = tempdir().unwrap();
    let (worktree, mainline) = session_worktree(dir.path());
    let config = Config::load(None, worktree.path()).unwrap();
    let mut edited = two_task_plan();
    edited.tasks.truncate(1);
    edited.goal = "Add a --verbose flag, parsed only".to_owned();
    let model = ScriptedModel::new([submitted_plan(&two_task_plan()), question(QUESTION)]);
    let operator = ScriptedOperator {
        decisions: [Decision::EditPlan(edited.clone()), Decision::Approve].into(),
        answers: [None].into(),
        ..ScriptedOperator::default()
    };
    let shown = Rc::clone(&operator.shown);
    let mut autopilot = Autopilot::new(&worktree, &mainline, &model, config)
        .unwrap()
        .with_operator(operator);

    autopilot.run(request()).await.unwrap();

    assert_eq!(
        shown.borrow()[1],
        format!(
            "{:?}",
            Checkpoint::Plan {
                plan: edited.clone()
            }
        )
    );
    assert!(autopilot.events().contains(&Event::PlanEdited {
        plan: edited.clone()
    }));
    assert_eq!(autopilot.session().plan(), Some(&edited));
}

#[tokio::test]
async fn an_edited_plan_without_tasks_ends_the_run_without_recording_it() {
    let dir = tempdir().unwrap();
    let (worktree, mainline) = session_worktree(dir.path());
    let config = Config::load(None, worktree.path()).unwrap();
    let mut emptied = two_task_plan();
    emptied.tasks.clear();
    let model = ScriptedModel::new([submitted_plan(&two_task_plan())]);
    let operator = ScriptedOperator {
        decisions: [Decision::EditPlan(emptied)].into(),
        ..ScriptedOperator::default()
    };
    let mut autopilot = Autopilot::new(&worktree, &mainline, &model, config)
        .unwrap()
        .with_operator(operator);

    let err = autopilot.run(request()).await.unwrap_err();

    assert_eq!(err.step, "reading the edited Plan");
    assert!(
        !autopilot
            .events()
            .iter()
            .any(|event| matches!(event, Event::PlanEdited { .. }))
    );
}

#[tokio::test]
async fn approving_some_tasks_drops_the_rest_before_scaffolding() {
    let dir = tempdir().unwrap();
    let (worktree, mainline) = session_worktree(dir.path());
    let config = Config::load(None, worktree.path()).unwrap();
    let plan = two_task_plan();
    let model = ScriptedModel::new([submitted_plan(&plan), question(QUESTION)]);
    let operator = ScriptedOperator {
        decisions: [Decision::ApproveTasks(vec![1])].into(),
        answers: [None].into(),
        ..ScriptedOperator::default()
    };
    let mut autopilot = Autopilot::new(&worktree, &mainline, &model, config)
        .unwrap()
        .with_operator(operator);

    let end = autopilot.run(request()).await.unwrap();

    assert!(
        matches!(
            end,
            SessionEnd::TierStopped {
                tier: Tier::Scaffolder,
                ..
            }
        ),
        "{end:?}"
    );
    assert!(autopilot.events().contains(&Event::PlanApproved {
        task_order: vec![1]
    }));
    let approved = autopilot.session().plan().unwrap();
    assert_eq!(approved.tasks, [plan.tasks[1].clone()]);
}

#[tokio::test]
async fn an_invalid_task_order_ends_the_run_without_approving() {
    let dir = tempdir().unwrap();
    let (worktree, mainline) = session_worktree(dir.path());
    let config = Config::load(None, worktree.path()).unwrap();
    let model = ScriptedModel::new([submitted_plan(&two_task_plan())]);
    let operator = ScriptedOperator {
        decisions: [Decision::ApproveTasks(vec![0, 0])].into(),
        ..ScriptedOperator::default()
    };
    let mut autopilot = Autopilot::new(&worktree, &mainline, &model, config)
        .unwrap()
        .with_operator(operator);

    let err = autopilot.run(request()).await.unwrap_err();

    assert_eq!(err.step, "applying an event");
    assert!(err.message.contains("invalid task order"), "{err}");
    assert!(
        !autopilot
            .events()
            .iter()
            .any(|event| matches!(event, Event::PlanApproved { .. }))
    );
}

fn read_readme() -> ChatResponse {
    let call = ToolCall {
        id: "call-read".to_owned(),
        name: "read_file".to_owned(),
        arguments: r#"{"path": "README.md"}"#.to_owned(),
    };
    model_reply(None, vec![call])
}

/// A Session whose Budget Cap is the cost of one scripted call, so the first call reaches it.
fn capped_config(worktree: &Worktree) -> Config {
    let mut config = Config::load(None, worktree.path()).unwrap();
    config.budget_cap_usd = Some(0.001);
    config
}

#[tokio::test]
async fn raising_the_budget_cap_between_two_calls_lets_the_run_go_on() {
    let dir = tempdir().unwrap();
    let (worktree, mainline) = session_worktree(dir.path());
    let model = ScriptedModel::new([read_readme(), submitted_plan(&two_task_plan())]);
    let raised = Usd::from_dollars(0.01);
    let operator = ScriptedOperator {
        decisions: [Decision::SetBudgetCap(Some(raised)), Decision::Stop].into(),
        ..ScriptedOperator::default()
    };
    let shown = operator.shown.clone();
    let mut autopilot = Autopilot::new(&worktree, &mainline, &model, capped_config(&worktree))
        .unwrap()
        .with_operator(operator);

    let end = autopilot.run(request()).await.unwrap();

    assert_eq!(end, SessionEnd::NeedsOperator(Effect::ShowPlanCheckpoint));
    let spent = Usd::from_dollars(0.001);
    assert_eq!(
        shown.borrow()[0],
        format!("{:?}", Checkpoint::Budget { cap: spent, spent })
    );
    assert!(
        shown.borrow()[1].starts_with("Plan"),
        "{:?}",
        shown.borrow()
    );
    assert_eq!(autopilot.session().budget_cap(), Some(raised));
    let cap_settings: Vec<&Event> = autopilot
        .events()
        .iter()
        .filter(|event| matches!(event, Event::BudgetCapSet { .. }))
        .collect();
    assert_eq!(cap_settings.len(), 2, "the config's cap, then the raise");
}

#[tokio::test]
async fn stopping_at_the_budget_checkpoint_ends_the_session_before_the_next_call() {
    let dir = tempdir().unwrap();
    let (worktree, mainline) = session_worktree(dir.path());
    let model = ScriptedModel::new([read_readme(), submitted_plan(&two_task_plan())]);
    let operator = ScriptedOperator {
        decisions: [Decision::Stop].into(),
        ..ScriptedOperator::default()
    };
    let mut autopilot = Autopilot::new(&worktree, &mainline, &model, capped_config(&worktree))
        .unwrap()
        .with_operator(operator);

    let end = autopilot.run(request()).await.unwrap();

    assert_eq!(end, SessionEnd::StoppedAtBudget);
    assert_eq!(*autopilot.session().phase(), Phase::Stopped);
    assert_eq!(autopilot.events().last(), Some(&Event::SessionStopped));
    assert_eq!(model.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_decision_that_does_not_fit_the_budget_checkpoint_is_an_error_not_a_second_question() {
    let dir = tempdir().unwrap();
    let (worktree, mainline) = session_worktree(dir.path());
    let model = ScriptedModel::new([read_readme(), submitted_plan(&two_task_plan())]);
    let operator = ScriptedOperator {
        decisions: [Decision::Approve].into(),
        ..ScriptedOperator::default()
    };
    let shown = operator.shown.clone();
    let mut autopilot = Autopilot::new(&worktree, &mainline, &model, capped_config(&worktree))
        .unwrap()
        .with_operator(operator);

    let err = autopilot.run(request()).await.unwrap_err();

    assert!(err.message.contains("Approve"), "{err}");
    assert_eq!(shown.borrow().len(), 1);
}

#[tokio::test]
async fn a_resumed_session_shows_its_checkpoint_again_without_rewriting_the_log() {
    let dir = tempdir().unwrap();
    let (worktree, mainline) = session_worktree(dir.path());
    let config = Config::load(None, worktree.path()).unwrap();
    let plan = two_task_plan();
    let first_model = ScriptedModel::new([submitted_plan(&plan)]);
    let stopping = ScriptedOperator {
        decisions: [Decision::Stop].into(),
        ..ScriptedOperator::default()
    };
    let mut first_run = Autopilot::new(&worktree, &mainline, &first_model, config.clone())
        .unwrap()
        .with_operator(stopping);
    let end = first_run.run(request()).await.unwrap();
    assert_eq!(end.run_outcome(), Some(RunOutcome::Cancelled));
    first_run.record_run_end(RunOutcome::Cancelled).unwrap();
    let logged = first_run.events().to_vec();

    let second_model = ScriptedModel::new([question("Which file?")]);
    let rejecting = ScriptedOperator {
        decisions: [Decision::Reject("Smaller Tasks".to_owned())].into(),
        answers: [None].into(),
        ..ScriptedOperator::default()
    };
    let shown = Rc::clone(&rejecting.shown);
    let mut new_log = Vec::new();
    let resumed_events = {
        let mut resumed = Autopilot::new(&worktree, &mainline, &second_model, config)
            .unwrap()
            .with_operator(rejecting)
            .log_events_to(&mut new_log);
        let end = resumed.resume(logged.clone()).await.unwrap();
        assert!(matches!(end, SessionEnd::TierStopped { .. }), "{end:?}");
        resumed.events().to_vec()
    };

    assert_eq!(
        shown.borrow()[0],
        format!("{:?}", Checkpoint::Plan { plan })
    );
    assert_eq!(resumed_events[..logged.len()], logged[..]);
    let rewritten = arbor_core::read_events(&new_log[..]).unwrap();
    assert_eq!(rewritten, resumed_events[logged.len()..]);
    assert_eq!(
        rewritten[0],
        Event::CheckpointRejected {
            feedback: "Smaller Tasks".to_owned()
        }
    );
}

#[tokio::test]
async fn resuming_redoes_the_model_call_that_was_in_flight() {
    let dir = tempdir().unwrap();
    let (worktree, mainline) = session_worktree(dir.path());
    let config = Config::load(None, worktree.path()).unwrap();
    let plan = two_task_plan();
    let model = ScriptedModel::new([submitted_plan(&plan)]);
    let operator = ScriptedOperator {
        decisions: [Decision::Stop].into(),
        ..ScriptedOperator::default()
    };
    let mut autopilot = Autopilot::new(&worktree, &mainline, &model, config)
        .unwrap()
        .with_operator(operator);

    let crashed_log = vec![Event::SessionStarted { request: request() }];
    let end = autopilot.resume(crashed_log).await.unwrap();

    assert_eq!(end, SessionEnd::NeedsOperator(Effect::ShowPlanCheckpoint));
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    assert!(autopilot.session().usage().tier(Tier::Planner).cost > Usd::ZERO);
}

/// A model that is reachable but fails every call, so a run stops at its first model call.
struct UnreachableChat;

impl LlmClient for UnreachableChat {
    fn chat<'a>(
        &'a self,
        _request: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<ChatResponse, LlmError>> {
        Box::pin(async {
            Err(LlmError::Server {
                status: 503,
                message: "down".to_owned(),
            })
        })
    }

    fn model<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ModelInfo, LlmError>> {
        AskingModel.model(id)
    }
}

fn work_unit(id: u32, file: &str, spec: &str) -> WorkUnit {
    WorkUnit {
        id: WorkUnitId(id),
        file: file.to_owned(),
        spec: spec.to_owned(),
        context: Vec::new(),
        acceptance_criteria: Vec::new(),
        test_filter: file.to_owned(),
    }
}

fn region(id: u32) -> String {
    format!("// ARBOR:BEGIN WU-{id}\ntodo!()\n// ARBOR:END WU-{id}\n")
}

/// The log up to committing the first Task's Skeleton, whose two Work Units fill `a.rs` and
/// `b.rs`.
fn log_until_the_skeleton_commit() -> Vec<Event> {
    vec![
        Event::SessionStarted { request: request() },
        Event::PlanSubmitted {
            plan: two_task_plan(),
        },
        Event::PlanApproved {
            task_order: vec![0, 1],
        },
        Event::WorkUnitsSubmitted {
            work_units: vec![
                work_unit(1, "a.rs", "Greet the user."),
                work_unit(2, "b.rs", "Say goodbye."),
            ],
        },
        Event::ScaffoldChecksPassed,
        Event::ScaffoldApproved,
    ]
}

fn commit_all(worktree: &Worktree, subject: &str) -> String {
    let message = arbor_git::CommitMessage::new(subject).unwrap();
    worktree
        .commit(&message)
        .unwrap()
        .unwrap()
        .as_str()
        .to_owned()
}

fn write_skeleton(worktree: &Worktree) {
    std::fs::write(worktree.path().join("a.rs"), region(1)).unwrap();
    std::fs::write(worktree.path().join("b.rs"), region(2)).unwrap();
}

#[tokio::test]
async fn a_skeleton_committed_by_a_killed_run_is_logged_with_its_hash_not_committed_again() {
    let dir = tempdir().unwrap();
    let (worktree, mainline) = session_worktree(dir.path());
    let config = Config::load(None, worktree.path()).unwrap();
    write_skeleton(&worktree);
    let skeleton = commit_all(&worktree, "Parse the flag (structure and tests)");
    let mut autopilot = Autopilot::new(&worktree, &mainline, &UnreachableChat, config).unwrap();

    let resumed = autopilot.resume(log_until_the_skeleton_commit()).await;

    assert!(resumed.is_err(), "the Writer's model call should fail");
    assert!(
        autopilot.events().contains(&Event::SkeletonCommitted {
            commit: Some(skeleton.clone())
        }),
        "{:?}",
        autopilot.events()
    );
    assert_eq!(worktree.head().unwrap().as_str(), skeleton);
}

#[tokio::test]
async fn a_work_unit_committed_by_a_killed_writer_is_logged_as_passed_not_written_again() {
    let dir = tempdir().unwrap();
    let (worktree, mainline) = session_worktree(dir.path());
    let config = Config::load(None, worktree.path()).unwrap();
    write_skeleton(&worktree);
    let skeleton = commit_all(&worktree, "Parse the flag (structure and tests)");
    std::fs::write(worktree.path().join("a.rs"), "println!(\"hello\");\n").unwrap();
    let written = commit_all(&worktree, &commit_subject("Greet the user.", "a.rs"));
    let mut killed_log = log_until_the_skeleton_commit();
    killed_log.push(Event::SkeletonCommitted {
        commit: Some(skeleton),
    });
    let mut autopilot = Autopilot::new(&worktree, &mainline, &UnreachableChat, config).unwrap();

    let resumed = autopilot.resume(killed_log.clone()).await;

    assert!(resumed.is_err(), "the next Writer's model call should fail");
    assert_eq!(
        autopilot.events()[killed_log.len()..killed_log.len() + 2],
        [
            Event::WorkUnitPassed {
                id: WorkUnitId(1),
                model: None
            },
            Event::WorkUnitCommitted {
                id: WorkUnitId(1),
                commit: written.clone()
            },
        ]
    );
    assert_eq!(worktree.head().unwrap().as_str(), written);
}

#[tokio::test]
async fn a_writer_killed_before_committing_is_redone_from_the_last_commit() {
    let dir = tempdir().unwrap();
    let (worktree, mainline) = session_worktree(dir.path());
    let config = Config::load(None, worktree.path()).unwrap();
    write_skeleton(&worktree);
    let skeleton = commit_all(&worktree, "Parse the flag (structure and tests)");
    std::fs::write(worktree.path().join("a.rs"), "half written\n").unwrap();
    let mut killed_log = log_until_the_skeleton_commit();
    killed_log.push(Event::SkeletonCommitted {
        commit: Some(skeleton.clone()),
    });
    let mut autopilot = Autopilot::new(&worktree, &mainline, &UnreachableChat, config).unwrap();

    let resumed = autopilot.resume(killed_log.clone()).await;

    assert!(resumed.is_err(), "the Writer's model call should fail");
    assert_eq!(autopilot.events().len(), killed_log.len());
    let file_text = std::fs::read_to_string(worktree.path().join("a.rs")).unwrap();
    assert!(file_text.contains("ARBOR:BEGIN WU-1"), "{file_text}");
    assert!(!file_text.contains("half written"), "{file_text}");
}
