use std::time::Duration;

use arbor_llm::{ChatRequest, ContentPart, LlmClient, LlmError, Message, OpenRouterClient};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn request() -> ChatRequest {
    ChatRequest {
        model: "anthropic/claude-haiku-5.5".into(),
        messages: vec![Message::User {
            content: vec![ContentPart::text("Say hi")],
        }],
        ..ChatRequest::default()
    }
}

async fn chat_against(response: ResponseTemplate) -> LlmError {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(response)
        .mount(&server)
        .await;
    let client = OpenRouterClient::with_base_url("sk-or-test", server.uri());
    client.chat(&request()).await.unwrap_err()
}

fn error_body(code: u16, message: &str) -> serde_json::Value {
    json!({ "error": { "code": code, "message": message } })
}

#[tokio::test]
async fn http_errors_map_to_distinct_kinds() {
    let cases = [
        (
            401,
            LlmError::Auth {
                message: "No auth credentials found".into(),
            },
        ),
        (
            402,
            LlmError::InsufficientCredits {
                message: "No auth credentials found".into(),
            },
        ),
        (
            400,
            LlmError::BadRequest {
                status: 400,
                message: "No auth credentials found".into(),
            },
        ),
        (
            503,
            LlmError::Server {
                status: 503,
                message: "No auth credentials found".into(),
            },
        ),
    ];
    for (status, expected) in cases {
        let response = ResponseTemplate::new(status)
            .set_body_json(error_body(status, "No auth credentials found"));
        assert_eq!(chat_against(response).await, expected, "status {status}");
    }
}

#[tokio::test]
async fn rate_limits_carry_the_retry_delay() {
    let response = ResponseTemplate::new(429)
        .insert_header("retry-after", "12")
        .set_body_json(error_body(429, "Slow down"));
    assert_eq!(
        chat_against(response).await,
        LlmError::RateLimited {
            retry_after: Some(Duration::from_secs(12)),
            message: "Slow down".into(),
        }
    );
}

#[tokio::test]
async fn an_error_body_that_isnt_json_is_reported_as_text() {
    let response = ResponseTemplate::new(502).set_body_string("Bad Gateway");
    assert_eq!(
        chat_against(response).await,
        LlmError::Server {
            status: 502,
            message: "Bad Gateway".into(),
        }
    );
}

#[tokio::test]
async fn a_provider_error_inside_a_reply_is_a_server_error() {
    let response = ResponseTemplate::new(200).set_body_json(json!({
        "id": "gen-3",
        "model": "anthropic/claude-haiku-5.5",
        "choices": [{
            "finish_reason": "error",
            "error": { "code": 502, "message": "Provider returned error" }
        }]
    }));
    assert_eq!(
        chat_against(response).await,
        LlmError::Server {
            status: 502,
            message: "Provider returned error".into(),
        }
    );
}

#[tokio::test]
async fn a_malformed_reply_is_a_decode_error() {
    let response = ResponseTemplate::new(200).set_body_string("not json");
    assert!(matches!(
        chat_against(response).await,
        LlmError::Decode { .. }
    ));
}

#[tokio::test]
async fn an_unreachable_server_is_a_network_error() {
    let client = OpenRouterClient::with_base_url("sk-or-test", "http://127.0.0.1:1");
    assert!(matches!(
        client.chat(&request()).await,
        Err(LlmError::Network { .. })
    ));
}
