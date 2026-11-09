//! End-to-end tests: a whole Session against the tiny sample repo in `tests/fixtures/sample-repo`,
//! with the scripted fake LLM, real git and the sample repo's real Checks (`cargo check` and
//! `cargo test`). Each scenario's model replies are in `tests/fixtures/pipeline/<scenario>.json`;
//! `fixture_client.rs` says how to record them again.

mod driver;
mod fixture_client;
mod sample_repo;

use arbor_config::Config;
use arbor_core::{Session, Tier, Usd};
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
