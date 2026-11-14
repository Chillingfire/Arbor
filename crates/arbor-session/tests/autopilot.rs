use std::cell::RefCell;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use arbor_config::Config;
use arbor_core::{Event, Request, Tier};
use arbor_git::{Mainline, Repo, Worktree};
use arbor_llm::{
    BoxFuture, ChatRequest, ChatResponse, FinishReason, LlmClient, LlmError, ModelInfo, Pricing,
    Usage,
};
use arbor_session::{Autopilot, AutopilotUpdate, SessionEnd};
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
