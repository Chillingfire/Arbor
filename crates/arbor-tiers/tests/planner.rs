mod common;

use arbor_config::{Checks, ClarificationMode};
use arbor_core::{Plan, Request, Task, Tier};
use arbor_llm::{Message, ModelInfo};
use arbor_prompts::{Conventions, PlannerInputs, Prompt, Prompts};
use arbor_tiers::{Outcome, PlannerRun, Rejection, SUBMIT_PLAN, UnsupportedModel, validate_plan};
use arbor_tools::{Tools, Toolset};
use common::{QueueClient, calls, model, tool_results};
use serde_json::json;
use tempfile::TempDir;

fn plan() -> Plan {
    Plan {
        goal: "Add a --verbose flag".into(),
        constraints: vec!["Assumption: verbose output goes to stderr".into()],
        tasks: vec![Task {
            title: "Parse the flag".into(),
            description: "Add --verbose to the CLI parser in src/cli.rs.".into(),
        }],
    }
}

fn prompt(dir: &TempDir, mode: ClarificationMode) -> Prompt {
    let request = Request {
        text: "Add a --verbose flag".into(),
        issue: None,
    };
    let conventions = Conventions::load(dir.path(), &[]).unwrap();
    let inputs = PlannerInputs {
        request: &request,
        clarification_mode: mode,
        replan: None,
    };
    Prompts::builtin().planner(&conventions, &inputs).unwrap()
}

fn run<'a>(
    dir: &TempDir,
    prompt: &'a Prompt,
    model: ModelInfo,
    rejected: Option<Rejection<'a>>,
) -> PlannerRun<'a> {
    PlannerRun {
        prompt,
        model,
        temperature: 0.5,
        clarification_mode: ClarificationMode::Assume,
        tools: Tools::new(dir.path(), Toolset::Planner, Checks::default()),
        rejected,
    }
}

#[tokio::test]
async fn the_planner_explores_then_submits_a_plan() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("cli.rs"), "fn parse() {}\n").unwrap();
    let prompt = prompt(&dir, ClarificationMode::Assume);
    let mut planner = run(&dir, &prompt, model(200_000, true), None)
        .start()
        .unwrap();
    let client = QueueClient::new([
        calls(&[("grep", json!({ "pattern": "fn parse" }))]),
        calls(&[(SUBMIT_PLAN, serde_json::to_value(plan()).unwrap())]),
    ]);

    assert_eq!(planner.turn(&client).await.unwrap().outcome, None);
    assert!(tool_results(planner.messages())[0].contains("cli.rs"));
    let turn = planner.turn(&client).await.unwrap();

    let Some(Outcome::Submitted(submitted)) = turn.outcome else {
        panic!("expected a Plan, got {:?}", turn.outcome);
    };
    assert_eq!(submitted, plan());
    assert_eq!(
        submitted.assumptions().collect::<Vec<_>>(),
        ["Assumption: verbose output goes to stderr"]
    );
}

#[tokio::test]
async fn the_planner_opens_with_its_prompt_and_offers_only_read_tools_and_submit_plan() {
    let dir = TempDir::new().unwrap();
    let prompt = prompt(&dir, ClarificationMode::Assume);
    let mut planner = run(&dir, &prompt, model(200_000, true), None)
        .start()
        .unwrap();
    let client = QueueClient::new([calls(&[(
        SUBMIT_PLAN,
        serde_json::to_value(plan()).unwrap(),
    )])]);
    planner.turn(&client).await.unwrap();

    let request = client.last_request();
    assert_eq!(request.messages[..2], prompt.messages()[..]);
    let names: Vec<_> = request
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["read_file", "list_dir", "glob", "grep", SUBMIT_PLAN]
    );
    let schema = &request.tools[4].parameters;
    assert_eq!(schema["required"], json!(["goal", "constraints", "tasks"]));
    assert!(schema["$defs"]["Task"].is_object());
}

#[tokio::test]
async fn a_plan_without_tasks_goes_back_to_the_planner() {
    let dir = TempDir::new().unwrap();
    let prompt = prompt(&dir, ClarificationMode::Assume);
    let mut planner = run(&dir, &prompt, model(200_000, true), None)
        .start()
        .unwrap();
    let empty = Plan {
        tasks: vec![],
        ..plan()
    };
    let client = QueueClient::new([
        calls(&[(SUBMIT_PLAN, serde_json::to_value(empty).unwrap())]),
        calls(&[(SUBMIT_PLAN, serde_json::to_value(plan()).unwrap())]),
    ]);

    assert_eq!(planner.turn(&client).await.unwrap().outcome, None);
    assert_eq!(
        tool_results(planner.messages())[0],
        "Error: the Plan is invalid: there are no Tasks; a Plan needs at least one"
    );
    let turn = planner.turn(&client).await.unwrap();
    assert_eq!(turn.outcome, Some(Outcome::Submitted(plan())));
}

#[test]
fn validation_lists_every_problem() {
    let plan = Plan {
        goal: "  ".into(),
        constraints: vec![],
        tasks: vec![
            Task {
                title: "".into(),
                description: "Do it".into(),
            },
            Task {
                title: "Second".into(),
                description: "\n".into(),
            },
        ],
    };
    assert_eq!(
        validate_plan(&plan),
        Err(
            "the Plan is invalid: the goal is empty; Task 1 has no title; \
             Task 2 has no description"
                .into()
        )
    );
    assert_eq!(validate_plan(&self::plan()), Ok(()));
}

#[tokio::test]
async fn a_rerun_after_rejection_sees_the_rejected_plan_and_the_feedback() {
    let dir = TempDir::new().unwrap();
    let prompt = prompt(&dir, ClarificationMode::Assume);
    let rejected = plan();
    let rejection = Rejection {
        plan: &rejected,
        feedback: "Also document the flag in the README.",
    };
    let mut planner = run(&dir, &prompt, model(200_000, true), Some(rejection))
        .start()
        .unwrap();
    let client = QueueClient::new([calls(&[(
        SUBMIT_PLAN,
        serde_json::to_value(plan()).unwrap(),
    )])]);
    planner.turn(&client).await.unwrap();

    let messages = client.last_request().messages;
    assert_eq!(
        messages[0],
        prompt.messages()[0],
        "the static prompt is unchanged"
    );
    let Message::User { content } = &messages[1] else {
        panic!("expected the inputs, got {:?}", messages[1]);
    };
    assert_eq!(content[0].text, prompt.inputs);
    assert!(
        content[1]
            .text
            .contains("\"goal\": \"Add a --verbose flag\"")
    );
    assert!(
        content[1]
            .text
            .contains("Also document the flag in the README.")
    );
}

#[test]
fn a_planner_model_without_tool_calling_is_rejected() {
    let dir = TempDir::new().unwrap();
    let prompt = prompt(&dir, ClarificationMode::Assume);
    let result = run(&dir, &prompt, model(200_000, false), None).start();
    assert_eq!(
        result.err(),
        Some(UnsupportedModel {
            tier: Tier::Planner,
            model: "anthropic/claude-opus-5.5".into(),
        })
    );
}
