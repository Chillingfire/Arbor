mod common;

use arbor_config::{Checks, ClarificationMode};
use arbor_core::{Plan, Request, Task, Tier, WorkUnit, WorkUnitId};
use arbor_llm::{Message, ModelInfo};
use arbor_prompts::{Conventions, Prompt, Prompts, ScaffolderInputs};
use arbor_tiers::{
    NO_SCAFFOLD_CHECK, Outcome, SUBMIT_WORK_UNITS, ScaffolderRun, TurnError, UnsupportedModel,
    WorkUnits,
};
use common::{QueueClient, calls, git_dir, model, text, tool_results};
use serde_json::{Value, json};
use tempfile::TempDir;

const SKELETON: &str = "pub fn add(a: i32, b: i32) -> i32 {\n    // ARBOR:BEGIN WU-4\n    \
                        todo!()\n    // ARBOR:END WU-4\n}\n";

fn work_units(spec: &str) -> WorkUnits {
    WorkUnits {
        work_units: vec![WorkUnit {
            id: WorkUnitId(4),
            file: "src/lib.rs".into(),
            spec: spec.into(),
            context: vec![],
            acceptance_criteria: vec!["add(2, 3) == 5".into()],
            test_filter: "tests::add".into(),
        }],
    }
}

fn submit(units: &WorkUnits) -> (&'static str, Value) {
    (SUBMIT_WORK_UNITS, serde_json::to_value(units).unwrap())
}

fn prompt(dir: &TempDir, mode: ClarificationMode) -> Prompt {
    let request = Request {
        text: "Add an add function".into(),
        issue: None,
    };
    let task = Task {
        title: "Add add".into(),
        description: "Add pub fn add to src/lib.rs.".into(),
    };
    let plan = Plan {
        goal: "Add numbers".into(),
        constraints: vec![],
        tasks: vec![task.clone()],
    };
    let conventions = Conventions::load(dir.path(), &[]).unwrap();
    let inputs = ScaffolderInputs {
        request: &request,
        plan: &plan,
        task: &task,
        clarification_mode: mode,
        cap_tokens: 100,
        revision: None,
    };
    Prompts::builtin()
        .scaffolder(&conventions, &inputs)
        .unwrap()
}

fn run<'a>(
    dir: &'a TempDir,
    prompt: &'a Prompt,
    model: ModelInfo,
    mode: ClarificationMode,
    scaffold: Option<&str>,
) -> ScaffolderRun<'a> {
    ScaffolderRun {
        prompt,
        model,
        temperature: 0.3,
        clarification_mode: mode,
        worktree: dir.path(),
        checks: Checks {
            scaffold: scaffold.map(str::to_owned),
            ..Checks::default()
        },
        cap_tokens: 100,
        first_id: WorkUnitId(4),
        feedback: None,
    }
}

#[tokio::test]
async fn the_scaffolder_writes_the_skeleton_then_submits_work_units() {
    let dir = git_dir();
    let prompt = prompt(&dir, ClarificationMode::Assume);
    let mut scaffolder = run(
        &dir,
        &prompt,
        model(200_000, true),
        ClarificationMode::Assume,
        Some("cargo --version"),
    )
    .start()
    .unwrap();
    let units = work_units("Return a + b.");
    let client = QueueClient::new([
        calls(&[(
            "write_file",
            json!({ "path": "src/lib.rs", "content": SKELETON }),
        )]),
        calls(&[submit(&units)]),
    ]);

    assert_eq!(scaffolder.turn(&client).await.unwrap().outcome, None);
    let turn = scaffolder.turn(&client).await.unwrap();
    assert_eq!(turn.outcome, Some(Outcome::Submitted(units)));

    let request = client.last_request();
    let names: Vec<_> = request
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "read_file",
            "list_dir",
            "glob",
            "grep",
            "write_file",
            "edit_file",
            "run_checks",
            SUBMIT_WORK_UNITS
        ]
    );
    let schema = &request.tools[7].parameters;
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["required"], json!(["work_units"]));
    assert!(schema["$defs"]["WorkUnit"].is_object());
}

#[tokio::test]
async fn the_opening_message_names_the_first_work_unit_id() {
    let dir = git_dir();
    let prompt = prompt(&dir, ClarificationMode::Assume);
    let scaffolder = run(
        &dir,
        &prompt,
        model(200_000, true),
        ClarificationMode::Assume,
        None,
    )
    .start()
    .unwrap();
    let messages = scaffolder.messages();
    assert_eq!(messages[0], prompt.messages()[0]);
    let Message::User { content } = &messages[1] else {
        panic!("expected the inputs, got {:?}", messages[1]);
    };
    assert_eq!(content[0].text, prompt.inputs);
    assert!(
        content[1]
            .text
            .starts_with("Number this Task's Work Units from WU-4")
    );
    assert_eq!(content.len(), 2, "no rejection without feedback");
}

#[tokio::test]
async fn a_rerun_after_rejection_sees_the_feedback_and_revises_the_skeleton_in_place() {
    let dir = git_dir();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), SKELETON).unwrap();
    let prompt = prompt(&dir, ClarificationMode::Assume);
    let mut scaffolder = ScaffolderRun {
        feedback: Some("Split the overflow handling into its own Work Unit."),
        ..run(
            &dir,
            &prompt,
            model(200_000, true),
            ClarificationMode::Assume,
            None,
        )
    }
    .start()
    .unwrap();
    let units = work_units("Return a + b.");
    let client = QueueClient::new([calls(&[submit(&units)])]);
    let turn = scaffolder.turn(&client).await.unwrap();
    assert_eq!(turn.outcome, Some(Outcome::Submitted(units)));

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
    let rejection = &content[2].text;
    assert!(rejection.contains("rejected your Skeleton and Work Units"));
    assert!(rejection.contains("still in the worktree"));
    assert!(rejection.contains("Split the overflow handling into its own Work Unit."));
    assert!(rejection.ends_with("then call `submit_work_units` again."));
}

#[tokio::test]
async fn an_over_cap_work_unit_goes_back_with_exact_sizes() {
    let dir = git_dir();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), SKELETON).unwrap();
    let prompt = prompt(&dir, ClarificationMode::Assume);
    let run = run(
        &dir,
        &prompt,
        model(200_000, true),
        ClarificationMode::Assume,
        None,
    );
    assert_eq!(run.warning(), Some(NO_SCAFFOLD_CHECK));
    let mut scaffolder = run.start().unwrap();
    // 250 characters are 100 tokens, and the criterion and margin push it over the cap of 100.
    let over = work_units(&"a".repeat(250));
    let trimmed = work_units("Return a + b.");
    let client = QueueClient::new([calls(&[submit(&over)]), calls(&[submit(&trimmed)])]);

    assert_eq!(scaffolder.turn(&client).await.unwrap().outcome, None);
    assert_eq!(
        tool_results(scaffolder.messages())[0],
        "Error: the Work Units were not accepted.\n\nEstimated Writer input per Work Unit \
         against the Writer Cap, in tokens (10% margin included):\n\
         - WU-4: 117 / 100 — over, trim or split"
    );
    let turn = scaffolder.turn(&client).await.unwrap();
    assert_eq!(turn.outcome, Some(Outcome::Submitted(trimmed)));
}

#[tokio::test]
async fn a_failing_scaffold_check_is_retried_then_escalates_to_the_checkpoint() {
    let dir = git_dir();
    let prompt = prompt(&dir, ClarificationMode::Assume);
    let mut scaffolder = run(
        &dir,
        &prompt,
        model(200_000, true),
        ClarificationMode::Assume,
        Some("cargo no-such-subcommand-for-arbor"),
    )
    .start()
    .unwrap();
    let units = work_units("Return a + b.");
    let client = QueueClient::new([
        calls(&[submit(&units)]),
        calls(&[
            (
                "write_file",
                json!({ "path": "src/lib.rs", "content": SKELETON }),
            ),
            submit(&units),
        ]),
        calls(&[submit(&units)]),
    ]);

    assert_eq!(scaffolder.turn(&client).await.unwrap().outcome, None);
    assert!(
        tool_results(scaffolder.messages())[0]
            .contains("src/lib.rs doesn't exist; write the Skeleton there before submitting")
    );
    assert_eq!(scaffolder.turn(&client).await.unwrap().outcome, None);
    let check_failed = tool_results(scaffolder.messages())[2];
    assert!(
        check_failed.starts_with("Error: the Scaffold Check failed"),
        "{check_failed}"
    );
    let turn = scaffolder.turn(&client).await.unwrap();

    let Some(Outcome::Escalated { failures }) = turn.outcome else {
        panic!("expected an Escalation, got {:?}", turn.outcome);
    };
    assert_eq!(failures.len(), 3);
    assert!(failures[2].contains("no-such-subcommand-for-arbor"));
    assert_eq!(scaffolder.turn(&client).await, Err(TurnError::Finished));
}

#[tokio::test]
async fn in_ask_mode_the_scaffolder_asks_once_then_submits() {
    let dir = git_dir();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), SKELETON).unwrap();
    let prompt = prompt(&dir, ClarificationMode::Ask);
    assert!(prompt.instructions.contains("Clarification Mode is Ask"));
    let mut scaffolder = run(
        &dir,
        &prompt,
        model(200_000, true),
        ClarificationMode::Ask,
        None,
    )
    .start()
    .unwrap();
    let units = work_units("Return a + b, wrapping on overflow.");
    let client = QueueClient::new([
        text("Should add wrap or panic on overflow?"),
        calls(&[submit(&units)]),
    ]);

    let turn = scaffolder.turn(&client).await.unwrap();
    assert_eq!(
        turn.outcome,
        Some(Outcome::Questions(
            "Should add wrap or panic on overflow?".into()
        ))
    );
    scaffolder.answer("Wrap.").unwrap();
    let turn = scaffolder.turn(&client).await.unwrap();
    assert_eq!(turn.outcome, Some(Outcome::Submitted(units)));
}

#[test]
fn a_scaffolder_model_without_tool_calling_is_rejected() {
    let dir = git_dir();
    let prompt = prompt(&dir, ClarificationMode::Assume);
    let started = run(
        &dir,
        &prompt,
        model(200_000, false),
        ClarificationMode::Assume,
        None,
    )
    .start();
    assert_eq!(
        started.err(),
        Some(UnsupportedModel {
            tier: Tier::Scaffolder,
            model: "anthropic/claude-opus-5.5".into(),
        })
    );
}
