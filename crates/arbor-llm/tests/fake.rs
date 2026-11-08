use arbor_llm::{
    ChatRequest, ChatResponse, ContentPart, Exchange, FinishReason, Fixture, LlmClient, LlmError,
    Message, OpenRouterClient, RecordingClient, ScriptedClient, Usage,
};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn request(text: &str) -> ChatRequest {
    ChatRequest {
        model: "anthropic/claude-haiku-5.5".into(),
        messages: vec![Message::User {
            content: vec![ContentPart::text(text)],
        }],
        temperature: Some(0.0),
        ..ChatRequest::default()
    }
}

fn response(content: &str) -> ChatResponse {
    ChatResponse {
        id: "gen-1".into(),
        model: "anthropic/claude-haiku-5.5".into(),
        content: Some(content.into()),
        tool_calls: vec![],
        finish_reason: FinishReason::Stop,
        usage: Usage::default(),
    }
}

/// A server that answers the first chat, rate-limits the second, and lists one model.
async fn openrouter() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "gen-1",
            "model": "anthropic/claude-haiku-5.5",
            "choices": [{ "finish_reason": "stop", "message": { "content": "hi" } }],
            "usage": { "prompt_tokens": 3, "completion_tokens": 1, "cost": 0.000001 }
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_json(json!({ "error": { "message": "Slow down" } })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [{
            "id": "anthropic/claude-haiku-5.5",
            "context_length": 200000,
            "pricing": { "prompt": "0.0000005", "completion": "0.0000025" },
            "supported_parameters": ["temperature"]
        }] })))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn a_recorded_session_replays_identically() {
    let server = openrouter().await;
    let recorder =
        RecordingClient::new(OpenRouterClient::with_base_url("sk-or-test", server.uri()));
    let first = recorder.chat(&request("one")).await;
    let second = recorder.chat(&request("two")).await;
    let model = recorder.model("anthropic/claude-haiku-5.5").await;
    assert!(first.is_ok() && matches!(second, Err(LlmError::RateLimited { .. })));

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.json");
    recorder.fixture().save(&path).unwrap();
    let replay = ScriptedClient::new(Fixture::load(&path).unwrap());

    assert_eq!(replay.chat(&request("one")).await, first);
    assert_eq!(replay.chat(&request("two")).await, second);
    assert_eq!(replay.model("anthropic/claude-haiku-5.5").await, model);
    assert_eq!(replay.remaining(), 0);
}

fn scripted(exchanges: Vec<Exchange>) -> ScriptedClient {
    ScriptedClient::new(Fixture {
        exchanges,
        models: vec![],
    })
}

#[tokio::test]
async fn a_request_that_differs_from_the_recording_is_refused() {
    let replay = scripted(vec![Exchange {
        request: request("one"),
        response: Ok(response("hi")),
    }]);
    let changed = ChatRequest {
        temperature: Some(0.5),
        ..request("one")
    };
    let err = replay.chat(&changed).await.unwrap_err();
    assert!(
        matches!(&err, LlmError::Fixture { message } if message.contains("temperature")),
        "{err}"
    );
    assert_eq!(replay.remaining(), 1);
}

#[tokio::test]
async fn running_past_the_end_of_the_recording_is_an_error() {
    let replay = scripted(vec![Exchange {
        request: request("one"),
        response: Ok(response("hi")),
    }]);
    replay.chat(&request("one")).await.unwrap();
    assert!(matches!(
        replay.chat(&request("two")).await,
        Err(LlmError::Fixture { .. })
    ));
}

#[tokio::test]
async fn an_unrecorded_model_is_unknown() {
    let replay = scripted(vec![]);
    assert_eq!(
        replay.model("x/other").await,
        Err(LlmError::UnknownModel {
            id: "x/other".into()
        })
    );
}
