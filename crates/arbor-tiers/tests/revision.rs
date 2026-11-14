mod common;

use arbor_config::{Checks, ClarificationMode};
use arbor_core::{
    BrokenRegion, Event, Plan, Priority, Request, Revision, RevisionCause, RevisionRound, Snippet,
    Task, WorkUnit, WorkUnitFailure, WorkUnitId,
};
use arbor_llm::Message;
use arbor_prompts::{Conventions, Prompt, Prompts, RevisionInputs, ScaffolderInputs};
use arbor_tiers::{
    Outcome, REWORDING_ONLY, RevisionRun, SUBMIT_REVISION, changes_outside_regions,
    changes_work_units, review_revision, revision_field_problems,
};
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
        shared_change: None,
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
    let cause = Box::leak(Box::new(RevisionCause::TaskChecksFailed {
        output: "test tests::adds ... FAILED".into(),
    }));
    let task_work_units = Box::leak(Box::new([unit(4, "Return a + b.")]));
    run_for(dir, prompt, cause, task_work_units)
}

fn run_for<'a>(
    dir: &'a TempDir,
    prompt: &'a Prompt,
    cause: &'a RevisionCause,
    task_work_units: &'a [WorkUnit],
) -> RevisionRun<'a> {
    RevisionRun {
        prompt,
        model: model(200_000, true),
        temperature: 0.3,
        clarification_mode: ClarificationMode::Assume,
        worktree: dir.path(),
        checks: Checks::default(),
        cap_tokens: 8000,
        cause,
        task_work_units,
        first_new_id: WorkUnitId(9),
        tool_call_budget: 15,
    }
}

/// WU-4 failed its Writer; WU-5 below it is still waiting for its own.
const TWO_MARKED: &str = "pub fn add(a: i32, b: i32) -> i32 {\n    // ARBOR:BEGIN WU-4\n    \
                          todo!()\n    // ARBOR:END WU-4\n}\n\n\n\npub fn sub(a: i32, b: i32) -> \
                          i32 {\n    // ARBOR:BEGIN WU-5\n    todo!()\n    // ARBOR:END WU-5\n}\n";

fn writer_failed_on_wu_4() -> RevisionCause {
    RevisionCause::WorkUnitFailed(WorkUnitFailure {
        id: WorkUnitId(4),
        attempts: vec![],
        missing_response: Some(vec!["the i32 overflow rules".into()]),
        broken_region: None,
    })
}

#[tokio::test]
async fn a_work_unit_revision_stays_in_its_work_unit_and_file() {
    let dir = worktree();
    std::fs::write(dir.path().join("src/lib.rs"), TWO_MARKED).unwrap();
    let prompt = prompt(&dir, &[]);
    let cause = writer_failed_on_wu_4();
    let task_work_units = [unit(4, "Add."), unit(5, "Subtract.")];
    let mut scaffolder = run_for(&dir, &prompt, &cause, &task_work_units)
        .start()
        .unwrap();
    let mut other_file = revision("Split into a helper", &[4, 9]);
    other_file.work_units[1].file = "src/helper.rs".into();
    let accepted = revision("WU-4 needed the overflow rules", &[4]);
    let client = QueueClient::new([
        calls(&[submit(&revision("Revise both", &[4, 5]))]),
        calls(&[submit(&other_file)]),
        calls(&[submit(&accepted)]),
    ]);

    scaffolder.turn(&client).await.unwrap();
    scaffolder.turn(&client).await.unwrap();
    let turn = scaffolder.turn(&client).await.unwrap();
    let errors = tool_results(scaffolder.messages());
    assert!(
        errors[0].contains("WU-5 is not one of this Task's Work Units"),
        "{}",
        errors[0]
    );
    assert!(
        errors[1].contains("stays in its file, src/lib.rs, but WU-9 is in src/helper.rs"),
        "{}",
        errors[1]
    );
    assert_eq!(
        turn.outcome,
        Some(Outcome::Submitted(accepted)),
        "WU-5 keeps its Region without being submitted"
    );
}

#[tokio::test]
async fn a_shared_signature_change_is_submitted_without_work_units() {
    let dir = worktree();
    let prompt = prompt(&dir, &[]);
    let mut scaffolder = run(&dir, &prompt).start().unwrap();
    let shared = Revision {
        diagnosis: "add() must return i64, and WU-5 calls it".into(),
        work_units: vec![],
        shared_change: Some("Make add() return i64".into()),
    };
    let with_units = Revision {
        work_units: vec![unit(4, "Add.")],
        ..shared.clone()
    };
    let client = QueueClient::new([calls(&[submit(&with_units)]), calls(&[submit(&shared)])]);

    scaffolder.turn(&client).await.unwrap();
    let turn = scaffolder.turn(&client).await.unwrap();
    assert!(
        tool_results(scaffolder.messages())[0].contains("leave `work_units` empty"),
        "{:?}",
        tool_results(scaffolder.messages())
    );
    assert_eq!(turn.outcome, Some(Outcome::Submitted(shared)));
}

#[test]
fn only_a_real_change_to_a_work_unit_counts() {
    let before = [unit(4, "Add the numbers.")];
    let reworded = revision("Say it more clearly", &[4]);
    assert!(!changes_work_units(&before, &reworded));

    let mut more_context = reworded.clone();
    more_context.work_units[0].context.push(Snippet {
        path: "src/num.rs".into(),
        text: "pub type Num = i64;".into(),
        priority: Priority::Required,
    });
    assert!(changes_work_units(&before, &more_context));
    assert!(changes_work_units(&before, &revision("Split", &[4, 9])));
}

const DIFF_HEADER: &str =
    "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n";

#[test]
fn marking_a_region_again_is_not_a_change_but_the_skeleton_is() {
    let remark = format!(
        "{DIFF_HEADER}@@ -1,3 +1,5 @@\n pub fn add(a: i32, b: i32) -> i32 {{\n-    a - b\n\
         +    // ARBOR:BEGIN WU-4\n+    todo!()\n+    // ARBOR:END WU-4\n }}\n"
    );
    let current = |_: &str| Some(REMARKED.to_owned());
    assert!(!changes_outside_regions(&remark, current));

    let signature = format!(
        "{DIFF_HEADER}@@ -1,3 +1,3 @@\n-pub fn add(a: i32, b: i32) -> i32 {{\n\
         +pub fn add(a: i64, b: i64) -> i64 {{\n     a - b\n }}\n"
    );
    assert!(changes_outside_regions(&signature, |_: &str| Some(
        FILLED.to_owned()
    )));
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
        rejection: None,
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

fn git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// A repo whose last commit holds WU-4's failed Region, as a Writer leaves it.
fn repo_with_failed_region() -> TempDir {
    let dir = worktree();
    let path = dir.path();
    std::fs::write(path.join("src/lib.rs"), TWO_MARKED).unwrap();
    git(path, &["init", "--quiet", "-b", "main"]);
    git(path, &["config", "user.name", "Test Operator"]);
    git(path, &["config", "user.email", "operator@example.com"]);
    git(path, &["config", "commit.gpgsign", "false"]);
    git(path, &["config", "core.autocrlf", "false"]);
    git(path, &["add", "--all"]);
    git(path, &["commit", "--quiet", "-m", "Add the skeleton"]);
    dir
}

#[test]
fn a_revision_that_only_rewords_is_rejected_and_a_new_test_file_is_a_change() {
    let dir = repo_with_failed_region();
    let task_work_units = [unit(4, "Add the numbers."), unit(5, "Subtract.")];
    let reworded = revision("Say it more plainly", &[4]);

    let reviewed = review_revision(
        dir.path(),
        &task_work_units,
        &writer_failed_on_wu_4(),
        reworded.clone(),
    )
    .unwrap();
    assert_eq!(
        reviewed,
        Event::RevisionRejected {
            revision: reworded.clone(),
            reason: REWORDING_ONLY.into(),
        }
    );

    std::fs::create_dir(dir.path().join("tests")).unwrap();
    std::fs::write(dir.path().join("tests/add.rs"), "#[test]\nfn adds() {}\n").unwrap();
    let reviewed = review_revision(
        dir.path(),
        &task_work_units,
        &writer_failed_on_wu_4(),
        reworded.clone(),
    )
    .unwrap();
    assert_eq!(reviewed, Event::RevisionSubmitted { revision: reworded });
}

#[test]
fn a_revision_of_a_broken_region_counts_once_the_region_is_marked_again() {
    let dir = repo_with_failed_region();
    let task_work_units = [unit(4, "Add the numbers."), unit(5, "Subtract.")];
    let broken_file = TWO_MARKED.replace("    // ARBOR:BEGIN WU-4\n", "");
    let broken = RevisionCause::WorkUnitFailed(WorkUnitFailure {
        id: WorkUnitId(4),
        attempts: vec![],
        missing_response: None,
        broken_region: Some(BrokenRegion {
            problem: "WU-4: ARBOR:END on line 2 has no ARBOR:BEGIN".into(),
            file_text: broken_file.clone(),
        }),
    });
    let marked_again = revision("Put WU-4's BEGIN Marker back", &[4]);

    let reviewed =
        review_revision(dir.path(), &task_work_units, &broken, marked_again.clone()).unwrap();
    assert_eq!(
        reviewed,
        Event::RevisionSubmitted {
            revision: marked_again.clone()
        }
    );

    std::fs::write(dir.path().join("src/lib.rs"), &broken_file).unwrap();
    git(
        dir.path(),
        &["commit", "--quiet", "--all", "-m", "Lose WU-4's BEGIN"],
    );
    let reviewed =
        review_revision(dir.path(), &task_work_units, &broken, marked_again.clone()).unwrap();
    assert!(
        matches!(reviewed, Event::RevisionRejected { .. }),
        "{reviewed:?}"
    );
}
