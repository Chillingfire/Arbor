mod common;

use arbor_config::{Checks, ClarificationMode};
use arbor_core::{Tier, Usd};
use arbor_llm::{LlmError, Message, ToolChoice};
use arbor_tiers::{Agent, AgentSettings, Outcome, TurnError, UnsupportedModel};
use arbor_tools::{Tools, Toolset};
use common::{Answer, QueueClient, ask, calls, calls_using, model, submission, tool_results};
use serde_json::json;
use tempfile::TempDir;

#[tokio::test]
async fn a_valid_submission_ends_the_run() {
    let dir = TempDir::new().unwrap();
    let mut agent = ask(&dir);
    let client = QueueClient::new([calls(&[("submit_answer", json!({ "answer": 42 }))])]);

    let turn = agent.turn(&client).await.unwrap();

    assert_eq!(
        turn.outcome,
        Some(Outcome::Submitted(Answer { answer: 42 }))
    );
    assert_eq!(turn.usage.input_tokens, 100);
    assert_eq!(turn.usage.cost, Usd::from_dollars(0.001));
    assert_eq!(agent.turn(&client).await, Err(TurnError::Finished));
}

#[tokio::test]
async fn requests_offer_the_tools_and_the_submit_tool_with_its_schema() {
    let dir = TempDir::new().unwrap();
    let mut agent = ask(&dir);
    let client = QueueClient::new([calls(&[("submit_answer", json!({ "answer": 1 }))])]);
    agent.turn(&client).await.unwrap();

    let request = client.last_request();
    assert_eq!(request.model, "anthropic/claude-opus-5.5");
    assert_eq!(request.temperature, Some(0.5));
    assert_eq!(request.tool_choice, Some(ToolChoice::Auto));
    let names: Vec<_> = request
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["read_file", "list_dir", "glob", "grep", "submit_answer"]
    );
    let submit = &request.tools[4];
    assert_eq!(submit.parameters["required"], json!(["answer"]));
    assert!(submit.parameters.get("$schema").is_none());
}

#[tokio::test]
async fn tool_calls_run_and_their_results_go_back_to_the_model() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "the answer is 7\n").unwrap();
    let mut agent = ask(&dir);
    let client = QueueClient::new([
        calls(&[
            ("read_file", json!({ "path": "notes.txt" })),
            ("read_file", json!({ "path": "missing.txt" })),
        ]),
        calls(&[("submit_answer", json!({ "answer": 7 }))]),
    ]);

    let first = agent.turn(&client).await.unwrap();
    assert_eq!(first.outcome, None);
    let tool_results = tool_results(agent.messages());
    assert!(
        tool_results[0].contains("the answer is 7"),
        "{}",
        tool_results[0]
    );
    assert!(
        tool_results[1].starts_with("Error: "),
        "{}",
        tool_results[1]
    );

    let second = agent.turn(&client).await.unwrap();
    assert_eq!(
        second.outcome,
        Some(Outcome::Submitted(Answer { answer: 7 }))
    );
    let sent = &client.last_request().messages;
    assert!(matches!(sent[1], Message::Assistant { .. }));
    assert!(matches!(sent[2], Message::Tool { ref tool_call_id, .. } if tool_call_id == "call-0"));
}

#[tokio::test]
async fn a_bad_submission_goes_back_as_the_tool_result_and_can_be_fixed() {
    let dir = TempDir::new().unwrap();
    let mut agent = ask(&dir);
    let client = QueueClient::new([
        calls(&[("submit_answer", json!({ "answer": "forty-two" }))]),
        calls(&[("submit_answer", json!({ "answer": 0 }))]),
        calls(&[("submit_answer", json!({ "answer": 42 }))]),
    ]);

    assert_eq!(agent.turn(&client).await.unwrap().outcome, None);
    assert_eq!(agent.turn(&client).await.unwrap().outcome, None);
    let tool_results = tool_results(agent.messages());
    assert!(tool_results[0].starts_with("Error: the arguments don't match the schema"));
    assert_eq!(tool_results[1], "Error: answer must be positive");

    let last = agent.turn(&client).await.unwrap();
    assert_eq!(
        last.outcome,
        Some(Outcome::Submitted(Answer { answer: 42 }))
    );
}

#[tokio::test]
async fn three_bad_submissions_escalate() {
    let dir = TempDir::new().unwrap();
    let mut agent = ask(&dir);
    let bad = || calls(&[("submit_answer", json!({ "answer": 0 }))]);
    let client = QueueClient::new([bad(), bad(), bad()]);

    agent.turn(&client).await.unwrap();
    agent.turn(&client).await.unwrap();
    let turn = agent.turn(&client).await.unwrap();

    let Some(Outcome::Escalated { failures }) = turn.outcome else {
        panic!("expected an Escalation, got {:?}", turn.outcome);
    };
    assert_eq!(failures, vec!["answer must be positive"; 3]);
    assert_eq!(agent.turn(&client).await, Err(TurnError::Finished));
}

#[tokio::test]
async fn the_first_valid_submission_in_a_reply_wins() {
    let dir = TempDir::new().unwrap();
    let mut agent = ask(&dir);
    let client = QueueClient::new([calls(&[
        ("submit_answer", json!({ "answer": 0 })),
        ("submit_answer", json!({ "answer": 5 })),
        ("submit_answer", json!({ "answer": 6 })),
    ])]);

    let turn = agent.turn(&client).await.unwrap();

    assert_eq!(turn.outcome, Some(Outcome::Submitted(Answer { answer: 5 })));
    let tool_results = tool_results(agent.messages());
    assert_eq!(tool_results.len(), 3, "every tool call gets a result");
    assert!(tool_results[2].starts_with("Ignored"));
}

#[tokio::test]
async fn every_request_caches_the_growing_transcript() {
    let dir = TempDir::new().unwrap();
    let mut agent = ask(&dir);
    let client = QueueClient::new([
        calls(&[("list_dir", json!({ "path": "." }))]),
        calls(&[("submit_answer", json!({ "answer": 3 }))]),
    ]);
    agent.turn(&client).await.unwrap();
    agent.turn(&client).await.unwrap();

    let requests = client.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| request.cache_transcript));
    // The breakpoint is added per request, so the stored transcript has no stale ones.
    assert!(matches!(
        requests[1].messages.last(),
        Some(Message::Tool { .. })
    ));
    assert!(agent.messages().iter().all(|message| match message {
        Message::User { content } | Message::System { content } =>
            content.iter().all(|part| !part.cache),
        _ => true,
    }));
}

#[tokio::test]
async fn cache_tokens_are_carried_into_the_recorded_usage() {
    let dir = TempDir::new().unwrap();
    let mut agent = ask(&dir);
    let usage = arbor_llm::Usage {
        input_tokens: 5000,
        output_tokens: 400,
        cache_read_tokens: 4000,
        cache_write_tokens: 600,
        cost_usd: 0.0123,
    };
    let client = QueueClient::new([calls_using(
        &[("submit_answer", json!({ "answer": 1 }))],
        usage,
    )]);

    let turn = agent.turn(&client).await.unwrap();

    assert_eq!(
        turn.usage,
        arbor_core::Usage {
            input_tokens: 5000,
            output_tokens: 400,
            cache_read_tokens: 4000,
            cache_write_tokens: 600,
            cost: Usd::from_dollars(0.0123),
        }
    );
}

#[tokio::test]
async fn a_failed_call_leaves_the_transcript_unchanged_so_it_can_be_retried() {
    let dir = TempDir::new().unwrap();
    let mut agent = ask(&dir);
    let client = QueueClient::default();
    client.push_err(LlmError::Network {
        message: "reset".into(),
    });

    let before = agent.messages().to_vec();
    assert!(matches!(agent.turn(&client).await, Err(TurnError::Llm(_))));
    assert_eq!(agent.messages(), before);
}

#[tokio::test]
async fn a_used_up_tool_call_budget_refuses_more_calls_and_makes_the_model_submit() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "the answer is 7\n").unwrap();
    let mut agent = ask(&dir).with_tool_call_budget(2);
    let read = || ("read_file", json!({ "path": "notes.txt" }));
    let client = QueueClient::new([
        calls(&[read()]),
        calls(&[read(), read()]),
        calls(&[("submit_answer", json!({ "answer": 7 }))]),
    ]);

    agent.turn(&client).await.unwrap();
    agent.turn(&client).await.unwrap();
    let tool_results = tool_results(agent.messages());
    assert!(
        tool_results[1].contains("the answer is 7"),
        "{}",
        tool_results[1]
    );
    assert_eq!(
        tool_results[2],
        "Error: the tool-call budget of 2 is used up; call `submit_answer` now."
    );
    assert!(
        common::last_user_text(agent.messages()).contains("tool-call budget of 2"),
        "the model is told to submit"
    );

    let turn = agent.turn(&client).await.unwrap();
    assert_eq!(turn.outcome, Some(Outcome::Submitted(Answer { answer: 7 })));
    assert_eq!(
        client.last_request().tool_choice,
        Some(ToolChoice::Tool("submit_answer".into()))
    );
}

#[test]
fn a_model_without_tool_calling_is_rejected() {
    let dir = TempDir::new().unwrap();
    let settings = AgentSettings {
        tier: Tier::Scaffolder,
        model: model(200_000, false),
        temperature: 0.3,
        clarification_mode: ClarificationMode::Ask,
    };
    let tools = Tools::new(dir.path(), Toolset::Planner, Checks::default());

    let new_agent = Agent::new(settings, submission(), tools, vec![]);

    assert_eq!(
        new_agent.err(),
        Some(UnsupportedModel {
            tier: Tier::Scaffolder,
            model: "anthropic/claude-opus-5.5".into(),
        })
    );
}
