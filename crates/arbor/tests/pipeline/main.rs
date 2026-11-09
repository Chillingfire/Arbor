//! End-to-end tests: a whole Session against the tiny sample repo in `tests/fixtures/sample-repo`,
//! with the scripted fake LLM, real git and the sample repo's real Checks (`cargo check` and
//! `cargo test`).

mod sample_repo;

use arbor_config::Config;
use arbor_tools::{CheckName, run_check};
use sample_repo::SampleRepo;

const FEATURE: &str = "Add longest_word";

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
