mod common;

use arbor_config::{Checks, ClarificationMode};
use arbor_core::{
    Plan, Request, Revision, RevisionCause, RevisionRound, Task, WorkUnit, WorkUnitId,
};
use arbor_llm::Message;
use arbor_prompts::{Conventions, Prompt, Prompts, RevisionInputs, ScaffolderInputs};
use arbor_tiers::{Outcome, RevisionRun, SUBMIT_REVISION, revision_field_problems};
use common::{QueueClient, calls, model, tool_results};
use serde_json::{Value, json};
use tempfile::TempDir;

/// WU-4 passed, so its Markers are gone.
const FILLED: &str = "pub fn add(a: i32, b: i32) -> i32 {\n    a - b\n}\n";

const REMARKED: &str = "pub fn add(a: i32, b: i32) -> i32 {\n    // ARBOR:BEGIN WU-4\n    \
                        todo!()\n    // ARBOR:END WU-4\n}\n\n\n\npub fn double(a: i32) -> i32 {\n    \
                        // ARBOR:BEGIN WU-9\n    todo!()\n    // ARBOR:END WU-9\n}\n";

fn unit(id: u32, spec: &str) -> WorkUnit {
    WorkUnit {
        id: WorkUnitId(id),
        file: "src/lib.rs".into(),
        spec: spec.into(),
        context: vec![],
        acceptance_criteria: vec![],
        test_filter: format!("tests::wu_{id}"),
    }
}

fn revision(diagnosis: &str, ids: &[u32]) -> Revision {
    Revision {
        diagnosis: diagnosis.into(),
        work_units: ids.iter().map(|&id| unit(id, "Add the numbers.")).collect(),
    }
}

fn submit(revision: &Revision) -> (&'static str, Value) {
    (SUBMIT_REVISION, serde_json::to_value(revision).unwrap())
}

fn prompt(dir: &TempDir, earlier: &[RevisionRound]) -> Prompt {
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
    let cause = RevisionCause::TaskChecksFailed {
        output: "test tests::adds ... FAILED".into(),
    };
    let work_units = [unit(4, "Return a + b.")];
    let inputs = ScaffolderInputs {
        request: &request,
        plan: &plan,
        task: &task,
        clarification_mode: ClarificationMode::Assume,
        cap_tokens: 8000,
        revision: Some(RevisionInputs {
            cause: &cause,
            task_diff: "+    a - b",
            work_units: &work_units,
            earlier,
        }),
    };
    let conventions = Conventions::load(dir.path(), &[]).unwrap();
    Prompts::builtin()
        .scaffolder(&conventions, &inputs)
        .unwrap()
}

fn run<'a>(dir: &'a TempDir, prompt: &'a Prompt) -> RevisionRun<'a> {
    RevisionRun {
        prompt,
        model: model(200_000, true),
        temperature: 0.3,
        clarification_mode: ClarificationMode::Assume,
        worktree: dir.path(),
        checks: Checks::default(),
        cap_tokens: 8000,
        revisable: vec![WorkUnitId(4)],
        first_new_id: WorkUnitId(9),
    }
}

fn worktree() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), FILLED).unwrap();
    dir
}

#[tokio::test]
async fn a_revision_marks_the_regions_again_and_submits_its_diagnosis() {
    let dir = worktree();
    let prompt = prompt(&dir, &[]);
    let mut scaffolder = run(&dir, &prompt).start().unwrap();
    let split = revision("add() subtracts; split doubling into WU-9", &[4, 9]);
    let client = QueueClient::new([
        calls(&[submit(&split)]),
        calls(&[
            (
                "write_file",
                json!({ "path": "src/lib.rs", "content": REMARKED }),
            ),
            submit(&split),
        ]),
    ]);

    assert_eq!(scaffolder.turn(&client).await.unwrap().outcome, None);
    let unmarked = tool_results(scaffolder.messages())[0];
    assert!(
        unmarked.contains("src/lib.rs: WU-4 has no ARBOR:BEGIN/ARBOR:END Marker pair"),
        "{unmarked}"
    );
    let turn = scaffolder.turn(&client).await.unwrap();
    assert_eq!(turn.outcome, Some(Outcome::Submitted(split)));

    let request = client.last_request();
    assert_eq!(request.tools.last().unwrap().name, SUBMIT_REVISION);
    let Message::User { content } = &request.messages[1] else {
        panic!("expected the inputs, got {:?}", request.messages[1]);
    };
    assert!(content[0].text.contains("# Revision 1"));
    assert!(content[0].text.contains("test tests::adds ... FAILED"));
    assert!(
        content[1]
            .text
            .contains("Number any new Work Unit from WU-9")
    );
}

#[tokio::test]
async fn a_revision_needs_a_diagnosis_and_ids_from_its_task_or_new_ones() {
    let dir = worktree();
    std::fs::write(dir.path().join("src/lib.rs"), REMARKED).unwrap();
    let prompt = prompt(&dir, &[]);
    let mut scaffolder = run(&dir, &prompt).start().unwrap();
    let client = QueueClient::new([
        calls(&[submit(&revision(" ", &[4, 9]))]),
        calls(&[submit(&revision("Reuse WU-2", &[4, 2]))]),
    ]);

    scaffolder.turn(&client).await.unwrap();
    scaffolder.turn(&client).await.unwrap();
    let errors = tool_results(scaffolder.messages());
    assert_eq!(
        errors[0],
        "Error: the Revision has no diagnosis; say why the work failed"
    );
    assert!(
        errors[1].contains(
            "WU-2 is not one of this Task's Work Units; keep a revised Work Unit's ID and \
             number new ones from WU-9"
        ),
        "{}",
        errors[1]
    );
}

#[test]
fn a_second_revision_sees_what_the_first_tried() {
    let dir = worktree();
    let earlier = [RevisionRound {
        cause: RevisionCause::TaskChecksFailed {
            output: "first failure".into(),
        },
        revision: revision("add() needed wrapping", &[4]),
    }];
    let prompt = prompt(&dir, &earlier);
    assert!(prompt.inputs.contains("# Revision 2"));
    assert!(prompt.inputs.contains("take a different approach"));
    assert!(
        prompt
            .inputs
            .contains("1. add() needed wrapping (revised WU-4)")
    );
}

#[test]
fn revised_ids_may_be_the_tasks_own_or_new() {
    let units = [unit(4, "a"), unit(9, "b"), unit(3, "c")];
    assert_eq!(
        revision_field_problems(&units, WorkUnitId(9), &[WorkUnitId(4)]),
        [
            "WU-3 is not one of this Task's Work Units; keep a revised Work Unit's ID and number \
          new ones from WU-9"
        ]
    );
}
