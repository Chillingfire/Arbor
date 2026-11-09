//! End-to-end tests: a whole Session against the tiny sample repo in `tests/fixtures/sample-repo`,
//! with the scripted fake LLM, real git and the sample repo's real Checks (`cargo check` and
//! `cargo test`). Each scenario's model replies are in `tests/fixtures/pipeline/<scenario>.json`;
//! `fixture_client.rs` says how to record them again.

mod driver;
mod fixture_client;
mod sample_repo;

use arbor_config::Config;
use arbor_core::{Event, Session, Tier, Usd, WorkUnitId};
use arbor_llm::{ChatRequest, Message};
use arbor_tiers::{AttemptOutcome, FailureReason};
use arbor_tools::{CheckName, run_check};
use driver::Pipeline;
use fixture_client::FixtureClient;
use sample_repo::SampleRepo;

const REQUEST: &str = "Add a `longest_word` function to wordy that returns the longest word in \
                       a text, or `None` when the text has no words. When several words are \
                       equally long, return the first.";

const FEATURE: &str = "Add longest_word";

/// The commit of the Skeleton approved at the scaffold Checkpoint, named after the Task.
const SKELETON_COMMIT: &str = "Add longest_word (structure and tests)";

/// The commit subject the Writer takes from the Work Unit's spec (ADR-0009).
const WRITER_COMMIT: &str = "Return the longest whitespace-separated word in `text`";

const IMPLEMENTATION: &str = "\
pub fn longest_word(text: &str) -> Option<&str> {
    let mut longest: Option<&str> = None;
    for word in text.split_whitespace() {
        match longest {
            Some(current) if current.len() >= word.len() => {}
            _ => longest = Some(word),
        }
    }
    longest
}
";

/// What the Writer says its Context Pack lacked in the Missing Response scenario.
const MISSING_ITEM: &str =
    "the signature of `longest_word` and the lines around its Region in src/lib.rs";

/// The Skeleton's test that a Writer using `max_by_key`, which keeps the last of tied words,
/// fails.
const TIE_TEST: &str = "keeps_the_first_of_tied_words";

/// The event log alone rebuilds the Session (ADR-0004).
fn assert_replays(pipeline: &Pipeline) {
    assert_eq!(Session::replay(&pipeline.events).unwrap(), pipeline.session);
}

fn assert_cost(session: &Session, tier: Tier, dollars: f64) {
    assert_eq!(
        session.usage().tier(tier).cost,
        Usd::from_dollars(dollars),
        "{tier:?}"
    );
}

fn last_user_text(request: &ChatRequest) -> &str {
    request
        .messages
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::User { content } => Some(content[0].text.as_str()),
            _ => None,
        })
        .unwrap()
}

#[tokio::test]
async fn a_fresh_session_worktree_of_the_sample_repo_is_clean_and_passes_its_checks() {
    let sample = SampleRepo::new(FEATURE);

    assert_eq!(sample.worktree.branch(), "arbor/add-longest-word");
    assert_eq!(sample.mainline.name(), "main");
    assert!(sample.session_commit_subjects().is_empty());
    assert_eq!(sample.uncommitted_changes(), "");
    assert!(sample.read("src/lib.rs").contains("pub fn word_count"));
    let checks = Config::load(None, sample.path()).unwrap().checks;
    for check in [CheckName::Scaffold, CheckName::Unit] {
        let check_output = run_check(sample.path(), &checks, check, None)
            .await
            .unwrap();
        assert!(!check_output.is_error, "{}", check_output.text);
    }
    // Build output stays out of the Writers' commits.
    assert_eq!(sample.uncommitted_changes(), "");
}

#[tokio::test]
async fn happy_path_plans_scaffolds_writes_and_commits_the_request() {
    let sample = SampleRepo::new(FEATURE);
    let client = FixtureClient::new("happy_path");
    let mut pipeline = Pipeline::new(&sample, &client);

    pipeline.run(REQUEST).await;

    client.finish();
    assert!(pipeline.session.is_done());
    assert_replays(&pipeline);
    let tier_models: Vec<String> = client.sent().into_iter().map(|sent| sent.model).collect();
    assert_eq!(
        tier_models,
        [
            "anthropic/claude-opus-5.5",
            "anthropic/claude-sonnet-5.5",
            "anthropic/claude-haiku-5.5"
        ]
    );
    let library = sample.read("src/lib.rs");
    assert!(library.contains(IMPLEMENTATION), "{library}");
    assert!(!library.contains("ARBOR:"), "{library}");
    assert!(library.contains("mod longest_word_tests"), "{library}");
    assert_eq!(
        sample.session_commit_subjects(),
        [SKELETON_COMMIT, WRITER_COMMIT]
    );
    assert_eq!(sample.uncommitted_changes(), "");
    assert_eq!(pipeline.writer_attempts.len(), 1);
    assert_cost(&pipeline.session, Tier::Planner, 0.025);
    assert_cost(&pipeline.session, Tier::Scaffolder, 0.04);
    assert_cost(&pipeline.session, Tier::Writer, 0.0012);
}

#[tokio::test]
async fn writer_retry_after_failing_checks_passes_on_the_second_attempt() {
    let sample = SampleRepo::new(FEATURE);
    let client = FixtureClient::new("writer_retry");
    let mut pipeline = Pipeline::new(&sample, &client);

    pipeline.run(REQUEST).await;

    client.finish();
    assert!(pipeline.session.is_done());
    assert_replays(&pipeline);
    let [first_attempt, second_attempt] = pipeline.writer_attempts.as_slice() else {
        panic!(
            "expected two Writer attempts: {:?}",
            pipeline.writer_attempts
        );
    };
    let AttemptOutcome::Failed(failure) = &first_attempt.outcome else {
        panic!("the first attempt should fail: {first_attempt:?}");
    };
    let FailureReason::ChecksFailed { output } = &failure.reason else {
        panic!("the Checks should fail the first attempt: {failure:?}");
    };
    assert!(output.contains(TIE_TEST), "{output}");
    assert!(matches!(
        second_attempt.outcome,
        AttemptOutcome::Passed { .. }
    ));
    let retry_request = client.sent().pop().unwrap();
    let retry_message = last_user_text(&retry_request);
    assert!(
        retry_message.contains("the Checks failed"),
        "{retry_message}"
    );
    assert!(retry_message.contains(TIE_TEST), "{retry_message}");
    let library = sample.read("src/lib.rs");
    assert!(library.contains(IMPLEMENTATION), "{library}");
    assert_eq!(
        sample.session_commit_subjects(),
        [SKELETON_COMMIT, WRITER_COMMIT]
    );
    assert_cost(&pipeline.session, Tier::Writer, 2.0 * 0.0012);
}

#[tokio::test]
async fn missing_response_goes_to_a_revision_that_gives_the_writer_what_it_lacked() {
    let sample = SampleRepo::new(FEATURE);
    let client = FixtureClient::new("missing_response");
    let mut pipeline = Pipeline::new(&sample, &client);

    pipeline.run(REQUEST).await;

    client.finish();
    assert!(pipeline.session.is_done());
    assert_replays(&pipeline);
    let failure = pipeline
        .events
        .iter()
        .find_map(|event| match event {
            Event::WorkUnitFailed { failure } => Some(failure),
            _ => None,
        })
        .expect("the Writer's Missing Response fails its Work Unit");
    assert_eq!(failure.id, WorkUnitId(1));
    assert_eq!(failure.missing_response.clone().unwrap(), [MISSING_ITEM]);
    assert!(
        failure.attempts.is_empty(),
        "a Missing Response uses no retry"
    );
    let revision_request = &client.sent()[3];
    let revision_inputs = last_user_text(revision_request);
    assert!(revision_inputs.contains(MISSING_ITEM), "{revision_inputs}");
    let revised = &pipeline.session.work_units()[0];
    assert_eq!(revised.context[0].path, "src/lib.rs");
    let library = sample.read("src/lib.rs");
    assert!(library.contains(IMPLEMENTATION), "{library}");
    assert!(!library.contains("ARBOR:"), "{library}");
    // The Revision changed only the Work Unit, so it made no commit of its own.
    assert_eq!(
        sample.session_commit_subjects(),
        [SKELETON_COMMIT, WRITER_COMMIT]
    );
    assert_cost(&pipeline.session, Tier::Scaffolder, 0.04 + 0.04);
    assert_cost(&pipeline.session, Tier::Writer, 2.0 * 0.0012);
}
