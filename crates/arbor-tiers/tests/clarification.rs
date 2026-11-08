mod common;

use arbor_config::ClarificationMode;
use arbor_llm::{Message, ToolChoice};
use arbor_tiers::{Outcome, TurnError};
use common::{Answer, QueueClient, agent, ask, calls, calls_using, last_user_text, text, usage};
use serde_json::json;
use tempfile::TempDir;

#[tokio::test]
async fn in_ask_mode_questions_are_batched_once_per_run() {
    let dir = TempDir::new().unwrap();
    let mut agent = ask(&dir);
    let client = QueueClient::new([
        text("1. Which module?\n2. Should it be async?"),
        text("One more question: which edition?"),
        calls(&[("submit_answer", json!({ "answer": 3 }))]),
    ]);

    let turn = agent.turn(&client).await.unwrap();
    assert_eq!(
        turn.outcome,
        Some(Outcome::Questions(
            "1. Which module?\n2. Should it be async?".into()
        ))
    );
    assert_eq!(agent.turn(&client).await, Err(TurnError::AwaitingAnswers));

    agent.answer("1. core\n2. no").unwrap();
    assert!(last_user_text(agent.messages()).ends_with("1. core\n2. no"));
    assert_eq!(agent.answer("again"), Err(TurnError::NoQuestions));

    // A second round of questions isn't allowed: the model is told to submit.
    assert_eq!(agent.turn(&client).await.unwrap().outcome, None);
    assert!(last_user_text(agent.messages()).contains("already asked"));
    let forced = agent.turn(&client).await.unwrap();
    assert_eq!(
        client.last_request().tool_choice,
        Some(ToolChoice::Tool("submit_answer".into()))
    );
    assert_eq!(
        forced.outcome,
        Some(Outcome::Submitted(Answer { answer: 3 }))
    );
}

#[tokio::test]
async fn in_assume_mode_a_text_reply_is_a_failed_submission() {
    let dir = TempDir::new().unwrap();
    let mut agent = agent(&dir, ClarificationMode::Assume, 200_000);
    let client = QueueClient::new([text("Which module?"), text("Hmm."), text("Still unsure.")]);

    assert_eq!(agent.turn(&client).await.unwrap().outcome, None);
    assert!(last_user_text(agent.messages()).contains("Clarification Mode is Assume"));
    assert_eq!(agent.turn(&client).await.unwrap().outcome, None);
    let turn = agent.turn(&client).await.unwrap();

    assert!(
        matches!(turn.outcome, Some(Outcome::Escalated { ref failures }) if failures.len() == 3)
    );
}

#[tokio::test]
async fn at_80_percent_of_the_context_window_the_model_is_told_to_submit() {
    let dir = TempDir::new().unwrap();
    let mut agent = agent(&dir, ClarificationMode::Ask, 10_000);
    let client = QueueClient::new([
        calls_using(&[("list_dir", json!({ "path": "." }))], usage(7_000, 900)),
        calls_using(&[("list_dir", json!({ "path": "." }))], usage(7_950, 50)),
        calls_using(&[("list_dir", json!({ "path": "." }))], usage(8_500, 50)),
    ]);

    agent.turn(&client).await.unwrap();
    assert_eq!(client.last_request().tool_choice, Some(ToolChoice::Auto));
    agent.turn(&client).await.unwrap();
    assert!(last_user_text(agent.messages()).contains("80% of your context window"));
    let told = agent
        .messages()
        .iter()
        .filter(|m| matches!(m, Message::User { content } if content[0].text.contains("80%")))
        .count();

    agent.turn(&client).await.unwrap();
    assert_eq!(
        client.last_request().tool_choice,
        Some(ToolChoice::Tool("submit_answer".into()))
    );
    let told_again = agent
        .messages()
        .iter()
        .filter(|m| matches!(m, Message::User { content } if content[0].text.contains("80%")))
        .count();
    assert_eq!((told, told_again), (1, 1), "the model is told only once");
}
