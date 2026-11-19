//! The Escalation Checkpoint's choices beyond a fix by hand and a Replan, on the `escalation`
//! scenario, where WU-1's Revision needs a shared change and so goes straight to the Human
//! Operator.

use super::*;

const STRONGER_MODEL: &str = "anthropic/claude-sonnet-5.5";

const HINT: &str = "Keep the first of tied words: compare with `>` so a tie never replaces it.";

/// Every user message part of the request, joined, so the inputs appended after the prompt show.
fn user_inputs(request: &ChatRequest) -> String {
    request
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User { content } => Some(content),
            _ => None,
        })
        .flatten()
        .map(|part| part.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn deciding_at_escalation<'a>(decision: Decision) -> ScriptedOperator<'a> {
    ScriptedOperator::new(move |checkpoint| match checkpoint {
        Checkpoint::Escalation { .. } => decision.clone(),
        _ => Decision::Approve,
    })
}

#[tokio::test]
async fn a_skipped_task_is_reset_and_the_session_finishes_without_it() {
    let sample = SampleRepo::new(FEATURE);
    let client = FixtureClient::new("escalation");
    let operator = deciding_at_escalation(Decision::Skip);
    let shown = operator.shown.clone();
    let (pipeline, end) = run_with_operator(&sample, &client, Some(operator)).await;

    client.finish();
    assert_eq!(end, SessionEnd::Finished);
    assert_replays(&pipeline);
    assert_eq!(pipeline.session().skipped_tasks(), [0]);
    assert!(pipeline.events().contains(&Event::TaskSkipped));
    let later_tasks = shown
        .borrow()
        .iter()
        .find_map(|checkpoint| match checkpoint {
            Checkpoint::Escalation { later_tasks, .. } => Some(later_tasks.clone()),
            _ => None,
        });
    assert_eq!(later_tasks, Some(Vec::new()), "the Plan has one Task");
    assert!(sample.session_commit_subjects().is_empty());
    assert_eq!(sample.uncommitted_changes(), "");
}

#[tokio::test]
async fn an_aborted_session_ends_for_good_and_keeps_the_branch() {
    let sample = SampleRepo::new(FEATURE);
    let client = FixtureClient::new("escalation");
    let operator = deciding_at_escalation(Decision::Abort);
    let (pipeline, end) = run_with_operator(&sample, &client, Some(operator)).await;

    client.finish();
    assert_eq!(end, SessionEnd::Aborted);
    assert_replays(&pipeline);
    assert_eq!(pipeline.events().last(), Some(&Event::SessionAborted));
    assert_eq!(sample.session_commit_subjects(), [SKELETON_COMMIT]);
}

#[tokio::test]
async fn a_stronger_writer_gets_a_failure_brief_and_its_pass_is_tagged_with_its_model() {
    let sample = SampleRepo::new(FEATURE);
    let client = FixtureClient::new("escalation_stronger_writer");
    let operator = deciding_at_escalation(Decision::RetryWithStrongerWriter);
    let shown = operator.shown.clone();
    let (pipeline, end) = run_with_operator(&sample, &client, Some(operator)).await;

    client.finish();
    assert_eq!(end, SessionEnd::Finished);
    assert_replays(&pipeline);
    let offer = shown
        .borrow()
        .iter()
        .find_map(|checkpoint| match checkpoint {
            Checkpoint::Escalation {
                stronger_writer, ..
            } => stronger_writer.clone(),
            _ => None,
        })
        .expect("a failed Work Unit can go to a stronger Writer");
    assert_eq!(offer.model, STRONGER_MODEL);
    assert!(offer.cost_per_attempt > Usd::default(), "{offer:?}");
    assert_eq!(offer.max_attempts, arbor_tiers::MAX_WRITER_ATTEMPTS);

    let stronger_request = client.sent().pop().unwrap();
    assert_eq!(stronger_request.model, STRONGER_MODEL);
    let scaffolder = Config::load(None, sample.path()).unwrap().roles.scaffolder;
    assert_eq!(stronger_request.temperature, Some(scaffolder.temperature));
    let inputs = user_inputs(&stronger_request);
    assert!(inputs.contains("# Failure Brief"), "{inputs}");
    assert!(inputs.contains(MISSING_ITEM), "{inputs}");
    assert!(inputs.contains("take the text by `String`"), "{inputs}");
    assert!(pipeline.events().contains(&Event::WorkUnitPassed {
        id: WorkUnitId(1),
        model: Some(STRONGER_MODEL.to_owned()),
    }));
    let library = sample.read("src/lib.rs");
    assert!(library.contains(IMPLEMENTATION), "{library}");
    assert_eq!(
        sample.session_commit_subjects(),
        [SKELETON_COMMIT, WRITER_COMMIT]
    );
}

#[tokio::test]
async fn a_writer_hint_reaches_the_writer_of_the_failed_work_unit() {
    let sample = SampleRepo::new(FEATURE);
    let client = FixtureClient::new("escalation_writer_hint");
    let operator = deciding_at_escalation(Decision::RetryWithHint {
        tier: Tier::Writer,
        hint: HINT.to_owned(),
    });
    let (pipeline, end) = run_with_operator(&sample, &client, Some(operator)).await;

    client.finish();
    assert_eq!(end, SessionEnd::Finished);
    assert_replays(&pipeline);
    let hinted_request = client.sent().pop().unwrap();
    assert_eq!(hinted_request.model, WRITER_MODEL);
    let inputs = user_inputs(&hinted_request);
    assert!(inputs.contains("# The Human Operator's hint"), "{inputs}");
    assert!(inputs.contains(HINT), "{inputs}");
    assert!(!inputs.contains("# Failure Brief"), "{inputs}");
    assert!(sample.read("src/lib.rs").contains(IMPLEMENTATION));
}
