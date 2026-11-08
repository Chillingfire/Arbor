mod common;

use std::fs;
use std::path::Path;

use arbor_config::{Checks, Config, Detection, PROJECT_TYPES};
use arbor_llm::{BoxFuture, Message};
use arbor_prompts::{Conventions, Prompts};
use arbor_tiers::{
    AutoDetection, CheckRun, ConfirmChecks, Confirmation, InitError, InitOutcome, ProposalError,
    ProposeChecks, ProposedChecks, SUBMIT_CHECKS, ScaffolderProposer, detect_checks, init,
    validate_checks,
};
use arbor_tools::CheckName;
use common::{QueueClient, calls, model};
use serde_json::json;
use tempfile::TempDir;

fn proposal() -> ProposedChecks {
    ProposedChecks {
        scaffold: Some("cargo --version".into()),
        unit: Some("cargo --version".into()),
        unit_filtered: None,
        explanation: "From the README.".into(),
    }
}

/// Answers with a fixed proposal and records what it was asked with.
#[derive(Default)]
struct FixedProposer {
    proposal: Option<ProposedChecks>,
    asked_with: Option<(String, Vec<CheckRun>)>,
}

impl ProposeChecks for FixedProposer {
    fn propose<'a>(
        &'a mut self,
        detection: &'a Detection,
        failed_runs: &'a [CheckRun],
    ) -> BoxFuture<'a, Result<ProposedChecks, ProposalError>> {
        self.asked_with = Some((detection.to_string(), failed_runs.to_vec()));
        let proposal = self
            .proposal
            .clone()
            .ok_or_else(|| ProposalError::Unavailable("no API key".into()));
        Box::pin(async move { proposal })
    }
}

struct Operator {
    confirmation: Confirmation,
    shown: Vec<ProposedChecks>,
}

impl Operator {
    fn new(confirmation: Confirmation) -> Self {
        Self {
            confirmation,
            shown: Vec::new(),
        }
    }
}

impl ConfirmChecks for Operator {
    fn confirm<'a>(&'a mut self, proposal: &'a ProposedChecks) -> BoxFuture<'a, Confirmation> {
        self.shown.push(proposal.clone());
        let confirmation = self.confirmation;
        Box::pin(async move { confirmation })
    }
}

fn write_crate(dir: &Path, manifest: &str) {
    fs::write(dir.join("Cargo.toml"), manifest).unwrap();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src").join("lib.rs"),
        "pub fn two() -> u32 { 2 }\n\n#[test]\nfn two_is_two() { assert_eq!(two(), 2); }\n",
    )
    .unwrap();
}

fn configured_checks(dir: &Path) -> Checks {
    Config::load(None, dir).unwrap().checks
}

#[tokio::test]
async fn detected_checks_that_pass_are_written_without_asking() {
    let dir = TempDir::new().unwrap();
    write_crate(
        dir.path(),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let mut proposer = FixedProposer::default();
    let mut operator = Operator::new(Confirmation::Skip);

    let outcome = init(dir.path(), &mut proposer, &mut operator)
        .await
        .unwrap();

    let rust = &PROJECT_TYPES[0];
    assert_eq!(
        outcome,
        InitOutcome::Detected {
            project_type: rust,
            path: dir.path().join("arbor.toml"),
        }
    );
    assert_eq!(
        outcome.notice(),
        "Wrote arbor.toml with the Checks for a Rust project."
    );
    assert_eq!(configured_checks(dir.path()), rust.checks());
    assert!(proposer.asked_with.is_none() && operator.shown.is_empty());
}

#[tokio::test]
async fn a_failing_detected_check_goes_to_the_proposal_and_an_accepted_one_is_written() {
    let dir = TempDir::new().unwrap();
    write_crate(dir.path(), "this is not a manifest");
    let mut proposer = FixedProposer {
        proposal: Some(proposal()),
        ..FixedProposer::default()
    };
    let mut operator = Operator::new(Confirmation::Accept);

    let outcome = init(dir.path(), &mut proposer, &mut operator)
        .await
        .unwrap();

    let (detection, failed_runs) = proposer.asked_with.unwrap();
    assert_eq!(detection, "Detected a Rust project from Cargo.toml.");
    assert_eq!(failed_runs.len(), 1, "unit isn't run once scaffold fails");
    assert_eq!(failed_runs[0].check, CheckName::Scaffold);
    assert_eq!(failed_runs[0].command, "cargo check --all-targets");
    assert!(failed_runs[0].output.contains("exit code"));
    assert_eq!(operator.shown, [proposal()]);
    assert_eq!(
        outcome,
        InitOutcome::Accepted {
            path: dir.path().join("arbor.toml"),
        }
    );
    assert_eq!(configured_checks(dir.path()), proposal().checks());
}

#[tokio::test]
async fn a_skipped_proposal_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let mut proposer = FixedProposer {
        proposal: Some(proposal()),
        ..FixedProposer::default()
    };
    let mut operator = Operator::new(Confirmation::Skip);

    let outcome = init(dir.path(), &mut proposer, &mut operator)
        .await
        .unwrap();

    assert_eq!(outcome, InitOutcome::Skipped);
    assert!(outcome.notice().contains("skip them and warn"));
    let (detection, failed_runs) = proposer.asked_with.unwrap();
    assert!(detection.starts_with("No manifest"), "{detection}");
    assert!(failed_runs.is_empty());
    assert!(!dir.path().join("arbor.toml").exists());
}

#[tokio::test]
async fn a_failed_proposal_is_an_error_and_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let mut operator = Operator::new(Confirmation::Accept);
    let err = init(dir.path(), &mut FixedProposer::default(), &mut operator)
        .await
        .unwrap_err();
    assert!(matches!(err, InitError::Proposal(_)), "{err:?}");
    assert_eq!(err.to_string(), "no API key");
    assert!(operator.shown.is_empty());
    assert!(!dir.path().join("arbor.toml").exists());
}

#[tokio::test]
async fn several_manifests_go_straight_to_the_proposal_without_running_anything() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("go.mod"), "module x\n").unwrap();
    fs::write(dir.path().join("package.json"), "{}\n").unwrap();
    let AutoDetection::Failed {
        detection,
        check_runs,
    } = detect_checks(dir.path()).await
    else {
        panic!("several project types can't be validated");
    };
    assert!(matches!(detection, Detection::Several(_)));
    assert!(check_runs.is_empty());
}

#[tokio::test]
async fn validation_runs_scaffold_then_unit_and_stops_at_the_first_failure() {
    let dir = TempDir::new().unwrap();
    let passing = Checks {
        scaffold: Some("cargo --version".into()),
        unit: Some("cargo --version".into()),
        unit_filtered: Some("no-such-program-for-arbor {filter}".into()),
    };
    let check_runs = validate_checks(dir.path(), &passing).await;
    let checks: Vec<CheckName> = check_runs.iter().map(|run| run.check).collect();
    assert_eq!(checks, [CheckName::Scaffold, CheckName::Unit]);
    assert!(check_runs.iter().all(|run| run.passed));

    let failing = Checks {
        scaffold: Some("no-such-program-for-arbor".into()),
        ..passing
    };
    let check_runs = validate_checks(dir.path(), &failing).await;
    assert_eq!(check_runs.len(), 1);
    assert!(!check_runs[0].passed);
    assert!(check_runs[0].output.contains("not found"), "{check_runs:?}");
}

#[tokio::test]
async fn the_scaffolder_proposes_checks_from_the_failed_detection() {
    let dir = TempDir::new().unwrap();
    write_crate(dir.path(), "this is not a manifest");
    let client = QueueClient::new([calls(&[(
        SUBMIT_CHECKS,
        serde_json::to_value(proposal()).unwrap(),
    )])]);
    let prompts = Prompts::builtin();
    let conventions = Conventions::load(dir.path(), &[]).unwrap();
    let mut proposer = ScaffolderProposer {
        client: &client,
        prompts: &prompts,
        conventions: &conventions,
        model: model(200_000, true),
        temperature: 0.3,
        worktree: dir.path(),
        usage: Default::default(),
    };
    let mut operator = Operator::new(Confirmation::Accept);

    let outcome = init(dir.path(), &mut proposer, &mut operator)
        .await
        .unwrap();

    assert!(matches!(outcome, InitOutcome::Accepted { .. }));
    assert_eq!(configured_checks(dir.path()), proposal().checks());
    assert_eq!(proposer.usage.output_tokens, 10);
    let Message::User { content } = &client.last_request().messages[1] else {
        panic!("the prompt's inputs come second");
    };
    let inputs = &content[0].text;
    assert!(inputs.contains("Detected a Rust project from Cargo.toml."));
    assert!(inputs.contains("## scaffold: `cargo check --all-targets`"));
}

#[tokio::test]
async fn a_scaffolder_that_never_submits_valid_checks_escalates() {
    let dir = TempDir::new().unwrap();
    let invalid = json!({ "explanation": "nothing found" });
    let client = QueueClient::new([
        calls(&[(SUBMIT_CHECKS, invalid.clone())]),
        calls(&[(SUBMIT_CHECKS, invalid.clone())]),
        calls(&[(SUBMIT_CHECKS, invalid)]),
    ]);
    let prompts = Prompts::builtin();
    let conventions = Conventions::default();
    let mut proposer = ScaffolderProposer {
        client: &client,
        prompts: &prompts,
        conventions: &conventions,
        model: model(200_000, true),
        temperature: 0.3,
        worktree: dir.path(),
        usage: Default::default(),
    };
    let err = init(
        dir.path(),
        &mut proposer,
        &mut Operator::new(Confirmation::Accept),
    )
    .await
    .unwrap_err();
    let InitError::Proposal(ProposalError::Escalated { failures }) = err else {
        panic!("expected an escalation, got {err:?}");
    };
    assert_eq!(failures.len(), 3);
    assert_eq!(proposer.usage.output_tokens, 30);
}
