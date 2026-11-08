//! The Checks are `git grep` commands, so they work wherever the tests do, Windows included.

mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

use arbor_config::Checks;
use arbor_core::{Priority, Snippet, WorkUnit, WorkUnitId};
use arbor_llm::{ChatResponse, FinishReason, Message};
use arbor_prompts::{Prompts, WriterInputs};
use arbor_tiers::reply::ReplyError;
use arbor_tiers::{
    AttemptOutcome, FailureReason, MAX_WRITER_ATTEMPTS, WRITER_SEED, Writer, WriterError,
    WriterRun, commit_subject,
};
use common::{QueueClient, last_user_text, text};
use tempfile::{TempDir, tempdir};

const SKELETON: &str = "\
\"\"\"A tiny calculator.\"\"\"

def add(a, b):
    # ARBOR:BEGIN WU-1
    raise NotImplementedError
    # ARBOR:END WU-1

def sub(a, b):
    return a - b
";

const FILLED: &str = "\
\"\"\"A tiny calculator.\"\"\"

def add(a, b):
    return a + b

def sub(a, b):
    return a - b
";

const GOOD: &str = "```python\n\ndef add(a, b):\n    return a + b\n\ndef sub(a, b):\n```";
const WRONG: &str = "```python\n\ndef add(a, b):\n    return a * b\n\ndef sub(a, b):\n```";

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn repo() -> TempDir {
    let dir = tempdir().unwrap();
    let path = dir.path();
    git(path, &["init", "--quiet", "-b", "main"]);
    git(path, &["config", "user.name", "Test Operator"]);
    git(path, &["config", "user.email", "operator@example.com"]);
    git(path, &["config", "commit.gpgsign", "false"]);
    git(path, &["config", "core.autocrlf", "false"]);
    fs::write(path.join("calc.py"), SKELETON).unwrap();
    git(path, &["add", "--all"]);
    git(
        path,
        &["commit", "--quiet", "-m", "Add the calculator skeleton"],
    );
    dir
}

fn work_unit() -> WorkUnit {
    WorkUnit {
        id: WorkUnitId(1),
        file: "calc.py".into(),
        spec: "add two numbers. Return their sum, e.g. add(1, 2) is 3.".into(),
        context: vec![Snippet {
            path: "calc.py".into(),
            text: SKELETON.into(),
            priority: Priority::Required,
        }],
        acceptance_criteria: vec!["add(1, 2) == 3".into()],
        test_filter: "return a + b".into(),
    }
}

/// The filtered Unit Check passes only once the file holds the test filter, i.e. the right code.
fn checks() -> Checks {
    Checks {
        scaffold: Some("git grep --quiet -e def -- calc.py".into()),
        unit: None,
        unit_filtered: Some("git grep --quiet -F {filter} -- calc.py".into()),
    }
}

fn writer(dir: &TempDir, checks: Checks) -> Writer {
    let unit = work_unit();
    let prompt = Prompts::builtin()
        .writer(&WriterInputs { work_unit: &unit })
        .unwrap();
    WriterRun {
        prompt: &prompt,
        work_unit: &unit,
        model: "anthropic/claude-haiku-5.5".into(),
        temperature: 0.0,
        worktree: dir.path(),
        checks,
    }
    .start()
}

fn file(dir: &TempDir) -> String {
    fs::read_to_string(dir.path().join("calc.py")).unwrap()
}

fn head(dir: &TempDir) -> String {
    git(dir.path(), &["rev-parse", "HEAD"])
}

#[tokio::test]
async fn a_passing_reply_is_spliced_stripped_and_committed() {
    let dir = repo();
    let before = head(&dir);
    let client = QueueClient::new([text(GOOD)]);
    let mut writer = writer(&dir, checks());

    let attempt = writer.attempt(&client).await.unwrap();

    let AttemptOutcome::Passed {
        commit: Some(commit),
    } = attempt.outcome
    else {
        panic!("expected a commit, got {:?}", attempt.outcome);
    };
    assert_eq!(attempt.usage.input_tokens, 100);
    assert_eq!(file(&dir), FILLED);
    assert_eq!(commit.as_str(), head(&dir));
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD~1"]), before);
    assert_eq!(
        git(dir.path(), &["log", "-1", "--format=%B"]),
        "Add two numbers"
    );
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "");
    assert!(writer.is_finished());
    assert!(matches!(
        writer.attempt(&client).await,
        Err(WriterError::Finished)
    ));
}

#[tokio::test]
async fn the_call_is_deterministic_and_sees_only_the_prompt() {
    let dir = repo();
    let client = QueueClient::new([text(GOOD)]);
    let mut writer = writer(&dir, checks());
    writer.attempt(&client).await.unwrap();

    let request = client.last_request();
    assert!(request.tools.is_empty());
    assert_eq!(request.tool_choice, None);
    assert_eq!(request.temperature, Some(0.0));
    assert_eq!(request.seed, Some(WRITER_SEED));
    assert!(request.require_parameters);
    assert_eq!(request.model, "anthropic/claude-haiku-5.5");
    let [Message::System { .. }, Message::User { content }] = request.messages.as_slice() else {
        panic!("expected the prompt alone, got {:?}", request.messages);
    };
    assert!(content[0].text.contains("# Work Unit WU-1"));
    assert!(content[0].text.contains("raise NotImplementedError"));
}

#[tokio::test]
async fn failed_checks_restore_the_placeholder_and_the_retry_sees_why() {
    let dir = repo();
    let before = head(&dir);
    let client = QueueClient::new([text(WRONG), text(GOOD)]);
    let mut writer = writer(&dir, checks());

    let first = writer.attempt(&client).await.unwrap();
    let AttemptOutcome::Failed(failure) = first.outcome else {
        panic!("expected a failure, got {:?}", first.outcome);
    };
    assert_eq!(failure.reply, WRONG);
    let FailureReason::ChecksFailed { output } = &failure.reason else {
        panic!("expected failed Checks, got {:?}", failure.reason);
    };
    assert!(
        output.contains("git grep --quiet -F 'return a + b'"),
        "{output}"
    );
    assert_eq!(file(&dir), SKELETON);
    assert_eq!(head(&dir), before);
    assert_eq!(writer.attempts_left(), MAX_WRITER_ATTEMPTS - 1);

    let second = writer.attempt(&client).await.unwrap();
    assert!(matches!(second.outcome, AttemptOutcome::Passed { .. }));
    assert_eq!(file(&dir), FILLED);

    let messages = client.last_request().messages;
    assert_eq!(messages.len(), 4);
    assert_eq!(
        messages[2],
        Message::Assistant {
            content: Some(WRONG.into()),
            tool_calls: vec![],
        }
    );
    let retry = last_user_text(&messages);
    assert!(retry.contains("the Checks failed"), "{retry}");
    assert!(retry.contains("git grep"), "{retry}");
}

#[tokio::test]
async fn rejected_replies_use_up_the_attempts() {
    let dir = repo();
    let before = head(&dir);
    let wrong_context = "```python\ndef add(a, b):\n    return a + b\n\ndef sub(a, b):\n```";
    let client = QueueClient::new([
        text(&format!("Here you go:\n{GOOD}")),
        text(wrong_context),
        text(&format!("{GOOD}\n{GOOD}")),
    ]);
    let mut writer = writer(&dir, checks());

    let mut reasons = Vec::new();
    for _ in 0..MAX_WRITER_ATTEMPTS {
        match writer.attempt(&client).await.unwrap().outcome {
            AttemptOutcome::Failed(failure) => reasons.push(failure.reason),
            other => panic!("expected a rejection, got {other:?}"),
        }
        assert_eq!(file(&dir), SKELETON);
    }
    assert!(matches!(
        reasons.as_slice(),
        [
            FailureReason::Rejected(ReplyError::TextOutside),
            FailureReason::Rejected(ReplyError::ContextMismatch { .. }),
            FailureReason::Rejected(ReplyError::SeveralBlocks(2)),
        ]
    ));
    assert_eq!(writer.failures().len(), 3);
    assert_eq!(writer.attempts_left(), 0);
    assert!(matches!(
        writer.attempt(&client).await,
        Err(WriterError::NoAttemptsLeft)
    ));
    assert_eq!(client.requests().len(), 3);
    assert_eq!(head(&dir), before);

    let retry = last_user_text(&client.last_request().messages).to_owned();
    assert!(
        retry.starts_with("Your reply was rejected: the first 2 lines"),
        "{retry}"
    );
}

#[tokio::test]
async fn a_reply_cut_off_at_the_token_limit_is_rejected() {
    let dir = repo();
    let cut_off = ChatResponse {
        finish_reason: FinishReason::Length,
        ..text(GOOD)
    };
    let client = QueueClient::new([cut_off]);
    let mut writer = writer(&dir, checks());
    let attempt = writer.attempt(&client).await.unwrap();
    assert!(matches!(
        attempt.outcome,
        AttemptOutcome::Failed(failure) if failure.reason == FailureReason::Rejected(ReplyError::CutOff)
    ));
    assert_eq!(file(&dir), SKELETON);
}

#[tokio::test]
async fn a_missing_response_finishes_the_writer_without_touching_the_file() {
    let dir = repo();
    let client = QueueClient::new([text("```missing\nthe Number type\n- what sub returns\n```")]);
    let mut writer = writer(&dir, checks());

    let attempt = writer.attempt(&client).await.unwrap();
    assert_eq!(
        attempt.outcome,
        AttemptOutcome::Missing {
            items: vec!["the Number type".into(), "what sub returns".into()],
        }
    );
    assert_eq!(file(&dir), SKELETON);
    assert!(writer.is_finished());
    assert!(writer.failures().is_empty());
}

#[tokio::test]
async fn without_a_unit_filtered_template_only_the_build_is_checked() {
    // The scaffold Check alone decides: it fails here, whatever the code.
    let dir = repo();
    let failing = Checks {
        scaffold: Some("git grep --quiet -e NOT_IN_THE_FILE -- calc.py".into()),
        ..Checks::default()
    };
    let client = QueueClient::new([text(GOOD)]);
    let attempt = writer(&dir, failing).attempt(&client).await.unwrap();
    let AttemptOutcome::Failed(failure) = attempt.outcome else {
        panic!("expected a failure, got {:?}", attempt.outcome);
    };
    assert!(failure.reason.to_string().contains("NOT_IN_THE_FILE"));
    assert_eq!(file(&dir), SKELETON);

    // Wrong code passes, since no Unit Check runs.
    let dir = repo();
    let scaffold_only = Checks {
        unit_filtered: None,
        ..checks()
    };
    let client = QueueClient::new([text(WRONG)]);
    let attempt = writer(&dir, scaffold_only).attempt(&client).await.unwrap();
    assert!(matches!(attempt.outcome, AttemptOutcome::Passed { .. }));
    assert!(file(&dir).contains("return a * b"));
}

#[tokio::test]
async fn a_file_without_the_region_is_an_error_not_an_attempt() {
    let dir = repo();
    fs::write(dir.path().join("calc.py"), FILLED).unwrap();
    let client = QueueClient::new([text(GOOD)]);
    let mut writer = writer(&dir, checks());
    let err = writer.attempt(&client).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "calc.py doesn't hold exactly one Region for WU-1"
    );
    assert_eq!(writer.attempts_left(), MAX_WRITER_ATTEMPTS);
}

#[test]
fn commit_subjects_are_plain_sentences_from_the_spec() {
    let long = "word ".repeat(30);
    let cases = [
        ("add two numbers. Return their sum.", "Add two numbers"),
        (
            "Parse the header, e.g. a Content-Type line. Then stop.",
            "Parse the header, e.g. a Content-Type line",
        ),
        (
            "\n\n- Validate the token:\n  check expiry",
            "Validate the token",
        ),
        ("Implement WU-3 for the parser", "Update src/lib.rs"),
        ("Let Arbor fill this in.", "Update src/lib.rs"),
        ("Normalise harbor names", "Normalise harbor names"),
        ("   ", "Update src/lib.rs"),
    ];
    for (spec, expected) in cases {
        assert_eq!(commit_subject(spec, "src/lib.rs"), expected, "{spec:?}");
    }
    let subject = commit_subject(&long, "src/lib.rs");
    assert!(
        subject.len() <= 72 && subject.starts_with("Word word"),
        "{subject}"
    );
}
