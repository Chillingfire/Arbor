mod common;

use arbor_config::Checks;
use arbor_prompts::{CheckProposalInputs, Conventions, Prompt, Prompts};
use arbor_tiers::{
    CheckProposalRun, Outcome, ProposedChecks, SUBMIT_CHECKS, validate_proposed_checks,
};
use common::{QueueClient, calls, model, tool_results};
use serde_json::json;
use tempfile::TempDir;

fn proposal() -> ProposedChecks {
    ProposedChecks {
        scaffold: Some("cargo check --all-targets".into()),
        unit: Some("cargo test".into()),
        unit_filtered: Some("cargo test {filter}".into()),
        explanation: "From .github/workflows/ci.yml.".into(),
    }
}

fn prompt(dir: &TempDir) -> Prompt {
    let conventions = Conventions::load(dir.path(), &[]).unwrap();
    let inputs = CheckProposalInputs {
        detection: "No manifest Arbor recognises is at the repository root.",
        tried_checks: &[],
    };
    Prompts::builtin()
        .check_proposal(&conventions, &inputs)
        .unwrap()
}

fn run<'a>(dir: &'a TempDir, prompt: &'a Prompt) -> CheckProposalRun<'a> {
    CheckProposalRun {
        prompt,
        model: model(200_000, true),
        temperature: 0.3,
        worktree: dir.path(),
    }
}

#[tokio::test]
async fn the_proposal_reads_the_ci_config_then_submits_checks() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".github/workflows")).unwrap();
    std::fs::write(
        dir.path().join(".github/workflows/ci.yml"),
        "steps:\n  - run: cargo test\n",
    )
    .unwrap();
    let prompt = prompt(&dir);
    let mut agent = run(&dir, &prompt).start().unwrap();
    let client = QueueClient::new([
        calls(&[("read_file", json!({ "path": ".github/workflows/ci.yml" }))]),
        calls(&[(SUBMIT_CHECKS, serde_json::to_value(proposal()).unwrap())]),
    ]);

    assert_eq!(agent.turn(&client).await.unwrap().outcome, None);
    assert!(tool_results(agent.messages())[0].contains("cargo test"));
    let turn = agent.turn(&client).await.unwrap();
    assert_eq!(turn.outcome, Some(Outcome::Submitted(proposal())));
    assert_eq!(proposal().checks().unit.as_deref(), Some("cargo test"));
}

#[tokio::test]
async fn the_proposal_is_read_only_and_opens_with_its_prompt() {
    let dir = TempDir::new().unwrap();
    let prompt = prompt(&dir);
    let mut agent = run(&dir, &prompt).start().unwrap();
    let client = QueueClient::new([calls(&[(
        SUBMIT_CHECKS,
        serde_json::to_value(proposal()).unwrap(),
    )])]);
    agent.turn(&client).await.unwrap();

    let request = client.last_request();
    assert_eq!(request.messages[..2], prompt.messages()[..]);
    let names: Vec<&str> = request
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["read_file", "list_dir", "glob", "grep", SUBMIT_CHECKS]
    );
    assert_eq!(
        request.tools[4].parameters["required"],
        json!(["explanation"])
    );
}

#[tokio::test]
async fn an_invalid_proposal_goes_back_to_the_scaffolder() {
    let dir = TempDir::new().unwrap();
    let prompt = prompt(&dir);
    let mut agent = run(&dir, &prompt).start().unwrap();
    let invalid = ProposedChecks {
        unit: Some("no-such-program-for-arbor test".into()),
        ..proposal()
    };
    let client = QueueClient::new([
        calls(&[(SUBMIT_CHECKS, serde_json::to_value(invalid).unwrap())]),
        calls(&[(SUBMIT_CHECKS, serde_json::to_value(proposal()).unwrap())]),
    ]);

    assert_eq!(agent.turn(&client).await.unwrap().outcome, None);
    let feedback = tool_results(agent.messages())[0];
    assert!(
        feedback.contains("unit: no-such-program-for-arbor"),
        "{feedback}"
    );
    let turn = agent.turn(&client).await.unwrap();
    assert_eq!(turn.outcome, Some(Outcome::Submitted(proposal())));
}

#[test]
fn proposed_checks_need_a_command_that_runs_without_a_shell_and_a_filter_placeholder() {
    let dir = TempDir::new().unwrap();
    assert_eq!(validate_proposed_checks(dir.path(), &proposal()), Ok(()));
    let unit_only = ProposedChecks {
        scaffold: None,
        unit_filtered: None,
        ..proposal()
    };
    assert_eq!(validate_proposed_checks(dir.path(), &unit_only), Ok(()));

    for (proposed, problem) in [
        (
            ProposedChecks {
                scaffold: None,
                unit: None,
                ..proposal()
            },
            "at least a scaffold or a unit command",
        ),
        (
            ProposedChecks {
                unit_filtered: Some("cargo test".into()),
                ..proposal()
            },
            "unit_filtered must contain {filter}",
        ),
        (
            ProposedChecks {
                scaffold: Some("cargo check 'unclosed".into()),
                ..proposal()
            },
            "scaffold: the configured command",
        ),
    ] {
        let message = validate_proposed_checks(dir.path(), &proposed).unwrap_err();
        assert!(message.contains(problem), "{message}");
    }
    assert_eq!(
        unit_only.checks(),
        Checks {
            unit: Some("cargo test".into()),
            ..Checks::default()
        }
    );
}
