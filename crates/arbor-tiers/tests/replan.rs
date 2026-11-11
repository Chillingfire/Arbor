mod common;

use arbor_config::{Checks, ClarificationMode};
use arbor_core::{Plan, Replan, Request, RevisionCause, Task};
use arbor_llm::Message;
use arbor_prompts::{Conventions, PlannerInputs, Prompt, Prompts, ReplanInputs};
use arbor_tiers::{Outcome, ReplanRun, SUBMIT_REPLAN};
use arbor_tools::{Tools, Toolset};
use common::{QueueClient, calls, model, tool_results};
use serde_json::{Value, json};
use tempfile::TempDir;

fn task(title: &str) -> Task {
    Task {
        title: title.into(),
        description: format!("{title} in src/cli.rs."),
    }
}

fn replan_prompt(dir: &TempDir) -> Prompt {
    let request = Request {
        text: "Add a --verbose flag".into(),
        issue: None,
    };
    let plan = Plan {
        goal: "Log each tool call".into(),
        constraints: vec![],
        tasks: vec![task("Parse the flag"), task("Log the calls")],
    };
    let cause = RevisionCause::TaskChecksFailed {
        output: "test verbose_logs ... FAILED".into(),
    };
    let inputs = PlannerInputs {
        request: &request,
        clarification_mode: ClarificationMode::Assume,
        replan: Some(ReplanInputs {
            plan: &plan,
            task: 1,
            cause: &cause,
            revisions: &[],
            task_diff: "+fn log() {}",
        }),
    };
    let conventions = Conventions::load(dir.path(), &[]).unwrap();
    Prompts::builtin().planner(&conventions, &inputs).unwrap()
}

fn submit(replan: &Replan) -> (&'static str, Value) {
    (SUBMIT_REPLAN, serde_json::to_value(replan).unwrap())
}

#[tokio::test]
async fn the_planner_replaces_the_failed_task_with_revised_ones() {
    let dir = TempDir::new().unwrap();
    let prompt = replan_prompt(&dir);
    let mut planner = ReplanRun {
        prompt: &prompt,
        model: model(200_000, true),
        temperature: 0.5,
        clarification_mode: ClarificationMode::Assume,
        tools: Tools::new(dir.path(), Toolset::Planner, Checks::default()),
        feedback: Some("Keep the logger separate"),
    }
    .start()
    .unwrap();
    let empty = Replan {
        diagnosis: " ".into(),
        tasks: vec![],
    };
    let revised = Replan {
        diagnosis: "Logging needed a logger first".into(),
        tasks: vec![task("Add a logger"), task("Log the calls")],
    };
    let client = QueueClient::new([calls(&[submit(&empty)]), calls(&[submit(&revised)])]);

    planner.turn(&client).await.unwrap();
    let turn = planner.turn(&client).await.unwrap();

    assert_eq!(
        tool_results(planner.messages())[0],
        "Error: the Replan is invalid: the diagnosis is empty; say why the Task failed; there \
         are no Tasks; a Replan replaces the failed Task with at least one"
    );
    assert_eq!(turn.outcome, Some(Outcome::Submitted(revised)));
    let request = client.last_request();
    assert_eq!(request.tools.last().unwrap().name, SUBMIT_REPLAN);
    let Message::User { content } = &request.messages[1] else {
        panic!("expected the inputs, got {:?}", request.messages[1]);
    };
    assert!(
        content[0]
            .text
            .contains("# Replan of Task 2: Log the calls")
    );
    assert!(content[0].text.contains("test verbose_logs ... FAILED"));
    assert!(content[1].text.contains("Keep the logger separate"));
    let names: Vec<_> = request
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert!(
        !names.contains(&"write_file"),
        "the Planner stays read-only"
    );
}

#[test]
fn the_replan_schema_asks_for_a_diagnosis_and_tasks() {
    let schema = serde_json::to_value(schemars::schema_for!(Replan)).unwrap();
    assert_eq!(schema["required"], json!(["diagnosis", "tasks"]));
}
