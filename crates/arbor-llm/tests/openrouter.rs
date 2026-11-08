use arbor_llm::{
    ChatRequest, ContentPart, FinishReason, LlmClient, Message, OpenRouterClient, ToolCall,
    ToolChoice, ToolDef, Usage,
};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
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

fn reply(message: serde_json::Value, finish_reason: &str) -> serde_json::Value {
    json!({
        "id": "gen-1",
        "model": "anthropic/claude-haiku-5.5",
        "choices": [{ "finish_reason": finish_reason, "message": message }],
        "usage": {
            "prompt_tokens": 12,
            "completion_tokens": 3,
            "total_tokens": 15,
            "prompt_tokens_details": { "cached_tokens": 0 },
            "cost": 0.000021
        }
    })
}

#[tokio::test]
async fn a_chat_call_sends_the_request_and_returns_the_reply() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer sk-or-test"))
        .and(body_json(json!({
            "model": "anthropic/claude-haiku-5.5",
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "Say hi" }] }],
            "usage": { "include": true }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(reply(
            json!({ "role": "assistant", "content": "hi" }),
            "stop",
        )))
        .expect(1)
        .mount(&server)
        .await;

    let client = OpenRouterClient::with_base_url("sk-or-test", server.uri());
    let response = client.chat(&request()).await.unwrap();
    assert_eq!(response.content.as_deref(), Some("hi"));
    assert_eq!(response.finish_reason, FinishReason::Stop);
}

#[tokio::test]
async fn every_request_option_is_sent_in_openrouter_format() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_json(json!({
            "model": "anthropic/claude-sonnet-5.5",
            "messages": [
                { "role": "system", "content": [
                    { "type": "text", "text": "You are the Scaffolder." },
                    { "type": "text", "text": "Conventions...", "cache_control": { "type": "ephemeral" } }
                ] },
                { "role": "user", "content": [{ "type": "text", "text": "Scaffold Task 1" }] },
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "call_1", "type": "function",
                    "function": { "name": "read_file", "arguments": "{\"path\":\"src/main.rs\"}" }
                }] },
                { "role": "tool", "tool_call_id": "call_1", "content": "fn main() {}" }
            ],
            "tools": [{ "type": "function", "function": {
                "name": "read_file",
                "description": "Read a file",
                "parameters": { "type": "object", "properties": { "path": { "type": "string" } } }
            } }],
            "tool_choice": { "type": "function", "function": { "name": "read_file" } },
            "temperature": 0.3,
            "seed": 7,
            "max_tokens": 4096,
            "provider": { "require_parameters": true },
            "usage": { "include": true }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(reply(
            json!({ "role": "assistant", "content": "done" }),
            "stop",
        )))
        .expect(1)
        .mount(&server)
        .await;

    let request = ChatRequest {
        model: "anthropic/claude-sonnet-5.5".into(),
        messages: vec![
            Message::System {
                content: vec![
                    ContentPart::text("You are the Scaffolder."),
                    ContentPart::cached("Conventions..."),
                ],
            },
            Message::User {
                content: vec![ContentPart::text("Scaffold Task 1")],
            },
            Message::Assistant {
                content: None,
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    arguments: r#"{"path":"src/main.rs"}"#.into(),
                }],
            },
            Message::Tool {
                tool_call_id: "call_1".into(),
                content: "fn main() {}".into(),
            },
        ],
        tools: vec![ToolDef {
            name: "read_file".into(),
            description: "Read a file".into(),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string" } } }),
        }],
        tool_choice: Some(ToolChoice::Tool("read_file".into())),
        temperature: Some(0.3),
        seed: Some(7),
        max_tokens: Some(4096),
        require_parameters: true,
        cache_transcript: false,
    };
    let client = OpenRouterClient::with_base_url("sk-or-test", server.uri());
    client.chat(&request).await.unwrap();
}

#[tokio::test]
async fn tool_calls_usage_and_cost_are_returned() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "gen-2",
            "model": "anthropic/claude-opus-5.5",
            "choices": [{
                "finish_reason": "tool_calls",
                "message": { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "call_9", "type": "function",
                    "function": { "name": "submit_plan", "arguments": "{\"goal\":\"x\"}" }
                }] }
            }],
            "usage": {
                "prompt_tokens": 5000,
                "completion_tokens": 400,
                "total_tokens": 5400,
                "prompt_tokens_details": { "cached_tokens": 4000, "cache_write_tokens": 600 },
                "cost": 0.0123
            }
        })))
        .mount(&server)
        .await;

    let client = OpenRouterClient::with_base_url("sk-or-test", server.uri());
    let response = client.chat(&request()).await.unwrap();
    assert_eq!(response.finish_reason, FinishReason::ToolCalls);
    assert_eq!(response.content, None);
    assert_eq!(
        response.tool_calls,
        [ToolCall {
            id: "call_9".into(),
            name: "submit_plan".into(),
            arguments: r#"{"goal":"x"}"#.into(),
        }]
    );
    assert_eq!(
        response.usage,
        Usage {
            input_tokens: 5000,
            output_tokens: 400,
            cache_read_tokens: 4000,
            cache_write_tokens: 600,
            cost_usd: 0.0123,
        }
    );
}

fn transcript(model: &str) -> ChatRequest {
    ChatRequest {
        model: model.into(),
        messages: vec![
            Message::System {
                content: vec![
                    ContentPart::text("You are the Planner."),
                    ContentPart::cached("Conventions..."),
                ],
            },
            Message::User {
                content: vec![ContentPart::text("Plan the Request")],
            },
            Message::Assistant {
                content: None,
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    arguments: "{}".into(),
                }],
            },
            Message::Tool {
                tool_call_id: "call_1".into(),
                content: "fn main() {}".into(),
            },
        ],
        cache_transcript: true,
        ..ChatRequest::default()
    }
}

async fn sent_body(request: &ChatRequest) -> serde_json::Value {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(reply(
            json!({ "role": "assistant", "content": "ok" }),
            "stop",
        )))
        .expect(1)
        .mount(&server)
        .await;
    let client = OpenRouterClient::with_base_url("sk-or-test", server.uri());
    client.chat(request).await.unwrap();
    let received = server.received_requests().await.unwrap();
    serde_json::from_slice(&received[0].body).unwrap()
}

fn breakpoints(body: &serde_json::Value) -> usize {
    body.to_string().matches("cache_control").count()
}

#[tokio::test]
async fn anthropic_models_get_a_breakpoint_after_the_conventions_and_on_the_newest_message() {
    let body = sent_body(&transcript("anthropic/claude-opus-5.5")).await;

    assert_eq!(
        body["messages"],
        json!([
            { "role": "system", "content": [
                { "type": "text", "text": "You are the Planner." },
                { "type": "text", "text": "Conventions...", "cache_control": { "type": "ephemeral" } }
            ] },
            { "role": "user", "content": [{ "type": "text", "text": "Plan the Request" }] },
            { "role": "assistant", "content": null, "tool_calls": [{
                "id": "call_1", "type": "function",
                "function": { "name": "read_file", "arguments": "{}" }
            }] },
            { "role": "tool", "tool_call_id": "call_1", "content": [
                { "type": "text", "text": "fn main() {}", "cache_control": { "type": "ephemeral" } }
            ] }
        ])
    );
}

#[tokio::test]
async fn the_transcript_breakpoint_goes_on_the_last_part_of_a_newest_user_message() {
    let mut request = transcript("anthropic/claude-opus-5.5");
    request.messages.push(Message::User {
        content: vec![ContentPart::text("Answers:"), ContentPart::text("yes")],
    });

    let body = sent_body(&request).await;

    assert_eq!(
        body["messages"][4]["content"],
        json!([
            { "type": "text", "text": "Answers:" },
            { "type": "text", "text": "yes", "cache_control": { "type": "ephemeral" } }
        ])
    );
    assert_eq!(body["messages"][3]["content"], json!("fn main() {}"));
    assert_eq!(breakpoints(&body), 2);
}

#[tokio::test]
async fn models_that_cache_automatically_get_no_breakpoints() {
    let body = sent_body(&transcript("openai/gpt-5.5")).await;

    assert_eq!(breakpoints(&body), 0, "{body}");
    assert_eq!(
        body["messages"][0]["content"][1],
        json!({ "type": "text", "text": "Conventions..." })
    );
    assert_eq!(body["messages"][3]["content"], json!("fn main() {}"));
}

#[tokio::test]
async fn at_most_four_breakpoints_are_sent_keeping_the_newest() {
    let mut request = transcript("anthropic/claude-opus-5.5");
    request.messages[1] = Message::User {
        content: (1..=4)
            .map(|i| ContentPart::cached(format!("part {i}")))
            .collect(),
    };

    let body = sent_body(&request).await;

    assert_eq!(breakpoints(&body), 4, "{body}");
    assert!(
        body["messages"][0]["content"][1]
            .get("cache_control")
            .is_none()
    );
    assert!(
        body["messages"][1]["content"][0]
            .get("cache_control")
            .is_none()
    );
    assert!(
        body["messages"][3]["content"][0]
            .get("cache_control")
            .is_some()
    );
}
