//! End-to-end tests: a whole Session against the tiny sample repo in `tests/fixtures/sample-repo`,
//! with the scripted fake LLM, real git and the sample repo's real Checks (`cargo check` and
//! `cargo test`). Each scenario's model replies are in `tests/fixtures/pipeline/<scenario>.json`;
//! `fixture_client.rs` says how to record them again.

mod escalation_options;
mod fixture_client;
mod sample_repo;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use arbor_config::Config;
use arbor_core::{Effect, Event, Request, RevisionCause, Session, Tier, Usd, WorkUnitId};
use arbor_llm::{BoxFuture, ChatRequest, ChatResponse, LlmClient, LlmError, Message, ModelInfo};
use arbor_session::{Autopilot, Checkpoint, Decision, Operator, Pending, SessionEnd};
use arbor_tiers::{AttemptOutcome, FailureReason};
use arbor_tools::{CheckName, run_check};
use fixture_client::FixtureClient;
use sample_repo::SampleRepo;

const REQUEST: &str = "Add a `longest_word` function to wordy that returns the longest word in \
                       a text, or `None` when the text has no words. When several words are \
                       equally long, return the first.";

const FEATURE: &str = "Add longest_word";

/// The Writer's model in the sample repo's config.
const WRITER_MODEL: &str = "anthropic/claude-haiku-5.5";

/// The commit of the Skeleton approved at the scaffold Checkpoint, named after the Task.
const SKELETON_COMMIT: &str = "Add longest_word (structure and tests)";

/// The commit of an accepted Revision that changed files.
const REVISION_COMMIT: &str = "Add longest_word (revised structure and tests)";

/// The commit subject the Writer takes from the Work Unit's spec (ADR-0009).
const WRITER_COMMIT: &str = "Return the longest whitespace-separated word in `text`";

/// The Writer's commit for the Work Unit before its Revision, whose spec left out ties.
const NARROW_WRITER_COMMIT: &str =
    "Find the longest word in `text`, or `None` when it has no words";

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
fn assert_replays(pipeline: &Autopilot) {
    assert_eq!(
        &Session::replay(pipeline.events()).unwrap(),
        pipeline.session()
    );
}

fn assert_cost(session: &Session, tier: Tier, dollars: f64) {
    assert_eq!(
        session.usage().tier(tier).cost,
        Usd::from_dollars(dollars),
        "{tier:?}"
    );
}

/// The Skeleton's Region in the Work Unit the Escalation scenarios fail.
const SKELETON_REGION: &str = "    // ARBOR:BEGIN WU-1\n    todo!()\n    // ARBOR:END WU-1\n";

/// The body of [`IMPLEMENTATION`], as the Human Operator types it in place of the Region.
const HAND_WRITTEN_BODY: &str = "    text.split_whitespace()
        .fold(None, |longest: Option<&str>, word| match longest {
            Some(current) if current.len() >= word.len() => Some(current),
            _ => Some(word),
        })
";

/// A hand-written body that also pastes in the Markers of a Work Unit that isn't being fixed.
const STRAY_MARKERS_AND_BODY: &str = "    // ARBOR:BEGIN WU-9
    // ARBOR:END WU-9
    text.split_whitespace()
        .fold(None, |longest: Option<&str>, word| match longest {
            Some(current) if current.len() >= word.len() => Some(current),
            _ => Some(word),
        })
";

/// Runs the Session for the Request to its end, approving every Checkpoint as it is.
async fn run_session<'a>(sample: &'a SampleRepo, client: &'a FixtureClient) -> Autopilot<'a> {
    let (pipeline, end) = run_with_operator(sample, client, None).await;
    assert_eq!(end, SessionEnd::Finished);
    pipeline
}

/// Runs the Session for the Request until it ends, with `operator` deciding at each Checkpoint,
/// or approving every Checkpoint as it is when there is none.
async fn run_with_operator<'a>(
    sample: &'a SampleRepo,
    client: &'a FixtureClient,
    operator: Option<ScriptedOperator<'a>>,
) -> (Autopilot<'a>, SessionEnd) {
    let config = Config::load(None, sample.path()).unwrap();
    let mut pipeline = Autopilot::new(&sample.worktree, &sample.mainline, client, config).unwrap();
    if let Some(operator) = operator {
        pipeline = pipeline.with_operator(operator);
    }
    let request = Request {
        text: REQUEST.to_owned(),
        issue: None,
    };
    let end = pipeline.run(request).await.unwrap();
    (pipeline, end)
}

/// Decides with `decide`, and keeps every Checkpoint it was shown.
struct ScriptedOperator<'a> {
    decide: Box<dyn FnMut(&Checkpoint) -> Decision + 'a>,
    shown: std::rc::Rc<std::cell::RefCell<Vec<Checkpoint>>>,
}

impl<'a> ScriptedOperator<'a> {
    fn new(decide: impl FnMut(&Checkpoint) -> Decision + 'a) -> Self {
        Self {
            decide: Box::new(decide),
            shown: std::rc::Rc::default(),
        }
    }
}

impl Operator for ScriptedOperator<'_> {
    fn decide<'a>(&'a mut self, checkpoint: &'a Checkpoint) -> Pending<'a, Decision> {
        self.shown.borrow_mut().push(checkpoint.clone());
        let decision = (self.decide)(checkpoint);
        Box::pin(async move { decision })
    }

    fn answer<'a>(&'a mut self, _tier: Tier, _questions: &'a str) -> Pending<'a, Option<String>> {
        Box::pin(async { None })
    }
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
    let pipeline = run_session(&sample, &client).await;

    client.finish();
    assert!(pipeline.session().is_done());
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
    assert_eq!(pipeline.writer_attempts().len(), 1);
    assert_cost(pipeline.session(), Tier::Planner, 0.025);
    assert_cost(pipeline.session(), Tier::Scaffolder, 0.04);
    assert_cost(pipeline.session(), Tier::Writer, 0.0012);
}

#[tokio::test]
async fn writer_retry_after_failing_checks_passes_on_the_second_attempt() {
    let sample = SampleRepo::new(FEATURE);
    let client = FixtureClient::new("writer_retry");
    let pipeline = run_session(&sample, &client).await;

    client.finish();
    assert!(pipeline.session().is_done());
    assert_replays(&pipeline);
    let writer_attempts = pipeline.writer_attempts();
    let [first_attempt, second_attempt] = writer_attempts.as_slice() else {
        panic!("expected two Writer attempts: {:?}", writer_attempts);
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
    assert_cost(pipeline.session(), Tier::Writer, 2.0 * 0.0012);
}

#[tokio::test]
async fn missing_response_goes_to_a_revision_that_gives_the_writer_what_it_lacked() {
    let sample = SampleRepo::new(FEATURE);
    let client = FixtureClient::new("missing_response");
    let pipeline = run_session(&sample, &client).await;

    client.finish();
    assert!(pipeline.session().is_done());
    assert_replays(&pipeline);
    let failure = pipeline
        .events()
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
    let revised = &pipeline.session().work_units()[0];
    assert_eq!(revised.context[0].path, "src/lib.rs");
    let library = sample.read("src/lib.rs");
    assert!(library.contains(IMPLEMENTATION), "{library}");
    assert!(!library.contains("ARBOR:"), "{library}");
    // The Revision changed only the Work Unit, so it made no commit of its own.
    assert_eq!(
        sample.session_commit_subjects(),
        [SKELETON_COMMIT, WRITER_COMMIT]
    );
    assert_cost(pipeline.session(), Tier::Scaffolder, 0.04 + 0.04);
    assert_cost(pipeline.session(), Tier::Writer, 2.0 * 0.0012);
}

#[tokio::test]
async fn revision_rewrites_a_work_unit_after_the_task_checks_fail() {
    let sample = SampleRepo::new(FEATURE);
    let client = FixtureClient::new("revision");
    let pipeline = run_session(&sample, &client).await;

    client.finish();
    assert!(pipeline.session().is_done());
    assert_replays(&pipeline);
    let task_check_output = pipeline
        .events()
        .iter()
        .find_map(|event| match event {
            Event::TaskChecksFailed { output } => Some(output),
            _ => None,
        })
        .expect("the Task's Unit Check fails once");
    assert!(task_check_output.contains(TIE_TEST), "{task_check_output}");
    let revision = pipeline
        .events()
        .iter()
        .find_map(|event| match event {
            Event::RevisionSubmitted { revision } => Some(revision),
            _ => None,
        })
        .expect("the Scaffolder submits a Revision");
    assert_eq!(revision.work_units[0].test_filter, "longest_word_tests");
    let revision_request = &client.sent()[3];
    let revision_inputs = last_user_text(revision_request);
    assert!(
        revision_inputs.contains("# Revision 1"),
        "{revision_inputs}"
    );
    assert!(revision_inputs.contains(TIE_TEST), "{revision_inputs}");
    assert!(revision_inputs.contains("max_by_key"), "{revision_inputs}");
    assert_eq!(pipeline.writer_attempts().len(), 2);
    let library = sample.read("src/lib.rs");
    assert!(library.contains(IMPLEMENTATION), "{library}");
    assert!(!library.contains("ARBOR:"), "{library}");
    assert_eq!(
        sample.session_commit_subjects(),
        [
            SKELETON_COMMIT,
            NARROW_WRITER_COMMIT,
            REVISION_COMMIT,
            WRITER_COMMIT
        ]
    );
    assert_cost(pipeline.session(), Tier::Scaffolder, 0.04 + 0.04);
}

#[tokio::test]
async fn an_unattended_session_stops_at_the_escalation_checkpoint() {
    let sample = SampleRepo::new(FEATURE);
    let client = FixtureClient::new("escalation");
    let (pipeline, end) = run_with_operator(&sample, &client, None).await;

    client.finish();
    let SessionEnd::NeedsOperator(Effect::ShowEscalationCheckpoint { task: 0, cause }) = end else {
        panic!("expected the Escalation Checkpoint, got {end:?}");
    };
    assert_eq!(cause.work_unit(), Some(WorkUnitId(1)));
    assert_replays(&pipeline);
    assert_eq!(sample.session_commit_subjects(), [SKELETON_COMMIT]);
    assert_eq!(sample.uncommitted_changes(), "");
}

#[tokio::test]
async fn a_fix_by_hand_is_checked_then_committed_and_the_session_finishes() {
    let sample = SampleRepo::new(FEATURE);
    let client = FixtureClient::new("escalation");
    let operator = ScriptedOperator::new(|checkpoint| {
        let Checkpoint::Escalation { failed_fix, .. } = checkpoint else {
            return Decision::Approve;
        };
        // The first time, the Human Operator says "fixed" without fixing anything.
        if failed_fix.is_some() {
            let library = sample.read("src/lib.rs");
            sample.write(
                "src/lib.rs",
                &library.replace(SKELETON_REGION, HAND_WRITTEN_BODY),
            );
        }
        Decision::Fixed
    });
    let shown = operator.shown.clone();
    let (pipeline, end) = run_with_operator(&sample, &client, Some(operator)).await;

    client.finish();
    assert_eq!(end, SessionEnd::Finished);
    assert_replays(&pipeline);
    let escalations: Vec<Option<String>> = shown
        .borrow()
        .iter()
        .filter_map(|checkpoint| match checkpoint {
            Checkpoint::Escalation {
                may_replan: true,
                failed_fix,
                ..
            } => Some(failed_fix.clone()),
            _ => None,
        })
        .collect();
    let [None, Some(failed_fix)] = escalations.as_slice() else {
        panic!("expected two Escalation Checkpoints, got {escalations:?}");
    };
    assert!(failed_fix.contains("not yet implemented"), "{failed_fix}");
    let library = sample.read("src/lib.rs");
    assert!(library.contains(HAND_WRITTEN_BODY), "{library}");
    assert!(!library.contains("ARBOR:"), "{library}");
    assert_eq!(
        sample.session_commit_subjects(),
        [SKELETON_COMMIT, arbor_tiers::TASK_FIX_SUBJECT]
    );
    assert_eq!(sample.uncommitted_changes(), "");
}

#[tokio::test]
async fn a_replan_resets_the_task_and_scaffolds_its_replacement() {
    let sample = SampleRepo::new(FEATURE);
    let client = FixtureClient::new("escalation_replan");
    let operator = ScriptedOperator::new(|checkpoint| match checkpoint {
        Checkpoint::Escalation { .. } => Decision::Replan,
        _ => Decision::Approve,
    });
    let shown = operator.shown.clone();
    let (pipeline, end) = run_with_operator(&sample, &client, Some(operator)).await;

    client.finish();
    assert_eq!(end, SessionEnd::Finished);
    assert_replays(&pipeline);
    let replan = shown
        .borrow()
        .iter()
        .find_map(|checkpoint| match checkpoint {
            Checkpoint::Replan { task: 0, replan } => Some(replan.clone()),
            _ => None,
        })
        .expect("the Replan goes to its Checkpoint");
    assert_eq!(pipeline.session().plan().unwrap().tasks, replan.tasks);
    let sent = client.sent();
    let replan_inputs = last_user_text(&sent[4]);
    assert!(
        replan_inputs.contains("take and return owned"),
        "the Replan sees the shared change: {replan_inputs}"
    );
    let failed_cause = pipeline.events().iter().find_map(|event| match event {
        Event::WorkUnitFailed { failure } => Some(RevisionCause::WorkUnitFailed(failure.clone())),
        _ => None,
    });
    assert_eq!(failed_cause.unwrap().work_unit(), Some(WorkUnitId(1)));
    assert_eq!(pipeline.session().work_units()[0].id, WorkUnitId(2));
    let library = sample.read("src/lib.rs");
    assert!(library.contains(IMPLEMENTATION), "{library}");
    assert!(!library.contains("ARBOR:"), "{library}");
    // The reset dropped the first Skeleton's commit.
    assert_eq!(
        sample.session_commit_subjects(),
        [SKELETON_COMMIT, WRITER_COMMIT]
    );
    assert_cost(pipeline.session(), Tier::Planner, 0.025 + 0.025);
}

#[tokio::test]
async fn a_fix_by_hand_that_keeps_the_regions_markers_has_them_removed() {
    let sample = SampleRepo::new(FEATURE);
    let client = FixtureClient::new("escalation");
    let operator = ScriptedOperator::new(|checkpoint| {
        if let Checkpoint::Escalation { .. } = checkpoint {
            let library = sample.read("src/lib.rs");
            let fixed = library.replace("    todo!()\n", HAND_WRITTEN_BODY);
            sample.write("src/lib.rs", &fixed);
            return Decision::Fixed;
        }
        Decision::Approve
    });
    let shown = operator.shown.clone();
    let (pipeline, end) = run_with_operator(&sample, &client, Some(operator)).await;

    client.finish();
    assert_eq!(end, SessionEnd::Finished);
    assert_replays(&pipeline);
    assert!(
        !shown
            .borrow()
            .iter()
            .any(|checkpoint| matches!(checkpoint, Checkpoint::Markers { .. }))
    );
    let library = sample.read("src/lib.rs");
    assert!(library.contains(HAND_WRITTEN_BODY), "{library}");
    assert!(!library.contains("ARBOR:"), "{library}");
    assert_eq!(
        sample.session_commit_subjects(),
        [SKELETON_COMMIT, arbor_tiers::TASK_FIX_SUBJECT]
    );
}

#[tokio::test]
async fn markers_a_fix_by_hand_adds_wait_at_the_marker_checkpoint_until_removed() {
    let sample = SampleRepo::new(FEATURE);
    let client = FixtureClient::new("escalation");
    let operator = ScriptedOperator::new(|checkpoint| {
        let library = sample.read("src/lib.rs");
        match checkpoint {
            Checkpoint::Escalation { .. } => {
                let fixed = library.replace(SKELETON_REGION, STRAY_MARKERS_AND_BODY);
                sample.write("src/lib.rs", &fixed);
            }
            Checkpoint::Markers { .. } => {
                let unmarked: String = library
                    .lines()
                    .filter(|line| !line.contains("ARBOR:"))
                    .map(|line| format!("{line}\n"))
                    .collect();
                sample.write("src/lib.rs", &unmarked);
            }
            _ => return Decision::Approve,
        }
        Decision::Fixed
    });
    let shown = operator.shown.clone();
    let (pipeline, end) = run_with_operator(&sample, &client, Some(operator)).await;

    client.finish();
    assert_eq!(end, SessionEnd::Finished);
    assert_replays(&pipeline);
    let markers = shown
        .borrow()
        .iter()
        .find_map(|checkpoint| match checkpoint {
            Checkpoint::Markers { remaining } => Some(remaining.clone()),
            _ => None,
        })
        .expect("the Marker Checkpoint opens");
    assert_eq!(markers.len(), 2, "{markers:?}");
    assert!(markers.iter().all(|marker| marker.file == "src/lib.rs"));
    assert!(!sample.read("src/lib.rs").contains("ARBOR:"));
    assert_eq!(
        sample.session_commit_subjects(),
        [
            SKELETON_COMMIT,
            arbor_tiers::TASK_FIX_SUBJECT,
            arbor_tiers::MARKER_FIX_SUBJECT
        ]
    );
}

/// Breaks the Region in src/lib.rs, as a hand edit in the worktree would, when the Autopilot
/// looks up the Writer's model: after the Skeleton is committed and before the Writer starts.
struct BreakingClient<'a> {
    inner: &'a FixtureClient,
    library_path: PathBuf,
    break_region: fn(&str) -> String,
    broken: AtomicBool,
}

impl LlmClient for BreakingClient<'_> {
    fn chat<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<ChatResponse, LlmError>> {
        self.inner.chat(request)
    }

    fn model<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ModelInfo, LlmError>> {
        if id == WRITER_MODEL && !self.broken.swap(true, Ordering::SeqCst) {
            let skeleton = std::fs::read_to_string(&self.library_path).unwrap();
            std::fs::write(&self.library_path, (self.break_region)(&skeleton)).unwrap();
        }
        self.inner.model(id)
    }
}

/// Runs the Session for the Request to its end, with `break_region` applied to the committed
/// Skeleton's src/lib.rs.
async fn run_session_breaking_the_region(
    sample: &SampleRepo,
    client: &FixtureClient,
    break_region: fn(&str) -> String,
) -> (Vec<Event>, Session, usize) {
    let breaking = BreakingClient {
        inner: client,
        library_path: sample.path().join("src/lib.rs"),
        break_region,
        broken: AtomicBool::new(false),
    };
    let config = Config::load(None, sample.path()).unwrap();
    let mut pipeline =
        Autopilot::new(&sample.worktree, &sample.mainline, &breaking, config).unwrap();
    let request = Request {
        text: REQUEST.to_owned(),
        issue: None,
    };
    assert_eq!(pipeline.run(request).await.unwrap(), SessionEnd::Finished);
    assert_replays(&pipeline);
    (
        pipeline.events().to_vec(),
        pipeline.session().clone(),
        pipeline.writer_attempts().len(),
    )
}

/// Checks that the broken Region failed its Work Unit with `problem` and no Writer attempt, that
/// the Revision saw why, and that the Session then wrote the Work Unit as usual.
fn assert_revised_after_broken_region(
    sample: &SampleRepo,
    client: &FixtureClient,
    (events, session, writer_attempts): (Vec<Event>, Session, usize),
    problem: &str,
) {
    client.finish();
    assert!(session.is_done());
    let failure = events
        .iter()
        .find_map(|event| match event {
            Event::WorkUnitFailed { failure } => Some(failure),
            _ => None,
        })
        .expect("the broken Region fails its Work Unit");
    let broken = failure.broken_region.as_ref().unwrap();
    assert_eq!(broken.problem, problem);
    assert!(failure.attempts.is_empty());
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::RevisionSubmitted { .. }))
    );
    let sent = client.sent();
    let revision_inputs = last_user_text(&sent[2]);
    assert!(revision_inputs.contains(problem), "{revision_inputs}");
    assert_eq!(writer_attempts, 1);
    let library = sample.read("src/lib.rs");
    assert!(library.contains(IMPLEMENTATION), "{library}");
    assert!(!library.contains("ARBOR:"), "{library}");
    assert_eq!(
        sample.session_commit_subjects(),
        [SKELETON_COMMIT, WRITER_COMMIT]
    );
    assert_cost(&session, Tier::Writer, 0.0012);
}

#[tokio::test]
async fn a_deleted_begin_marker_goes_to_a_revision_that_marks_the_region_again() {
    let sample = SampleRepo::new(FEATURE);
    let client = FixtureClient::new("broken_region_deleted_begin");
    let run = run_session_breaking_the_region(&sample, &client, |skeleton| {
        skeleton.replace("    // ARBOR:BEGIN WU-1\n", "")
    })
    .await;

    assert_revised_after_broken_region(
        &sample,
        &client,
        run,
        "WU-1: ARBOR:END on line 12 has no ARBOR:BEGIN",
    );
}

#[tokio::test]
async fn a_duplicated_region_goes_to_a_revision_that_leaves_one() {
    let sample = SampleRepo::new(FEATURE);
    let client = FixtureClient::new("broken_region_duplicated");
    let run = run_session_breaking_the_region(&sample, &client, |skeleton| {
        format!("{skeleton}\n// ARBOR:BEGIN WU-1\n// ARBOR:END WU-1\n")
    })
    .await;

    assert_revised_after_broken_region(
        &sample,
        &client,
        run,
        "WU-1 has more than one Region, on lines 11, 46",
    );
}
