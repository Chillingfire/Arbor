use std::path::Path;
use std::process::Command;

use arbor_config::Config;
use arbor_core::{Event, Request, Tier};
use arbor_git::Repo;
use arbor_llm::{
    BoxFuture, ChatRequest, ChatResponse, FinishReason, LlmClient, LlmError, ModelInfo, Pricing,
    Usage,
};
use arbor_session::{Autopilot, SessionEnd};
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

#[tokio::test]
async fn a_tier_that_asks_questions_ends_the_run_for_the_operator() {
    let dir = tempdir().unwrap();
    let checkout = dir.path().join("project");
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
        .create_worktree(&mainline, &branch, &dir.path().join("session"))
        .unwrap();
    let config = Config::load(None, worktree.path()).unwrap();
    let mut autopilot = Autopilot::new(&worktree, &mainline, &AskingModel, config).unwrap();

    let end = autopilot
        .run(Request {
            text: "Add a --verbose flag".to_owned(),
            issue: None,
        })
        .await
        .unwrap();

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
    let planner_cost = autopilot.session().usage().tier(Tier::Planner).cost;
    assert_eq!(planner_cost.dollars(), 0.0055);
}
