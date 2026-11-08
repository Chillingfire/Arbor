use arbor_llm::{LlmClient, LlmError, ModelInfo, OpenRouterClient, Pricing};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn models_body() -> serde_json::Value {
    json!({
        "data": [
            {
                "id": "anthropic/claude-haiku-5.5",
                "name": "Claude Haiku 5.5",
                "context_length": 200000,
                "pricing": { "prompt": "0.0000005", "completion": "0.0000025", "input_cache_read": "0.00000005" },
                "supported_parameters": ["max_tokens", "temperature", "tools"],
                "top_provider": { "context_length": 200000, "is_moderated": false }
            },
            {
                "id": "x/no-cache",
                "name": "No cache",
                "context_length": 8192,
                "pricing": { "prompt": "0.000001", "completion": "0.000002" },
                "supported_parameters": ["seed"]
            }
        ]
    })
}

async fn server_with_models(expected_fetches: u64) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", "Bearer sk-or-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(models_body()))
        .expect(expected_fetches)
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn model_metadata_is_returned_with_prices_per_token() {
    let server = server_with_models(1).await;
    let client = OpenRouterClient::with_base_url("sk-or-test", server.uri());
    assert_eq!(
        client.model("anthropic/claude-haiku-5.5").await.unwrap(),
        ModelInfo {
            id: "anthropic/claude-haiku-5.5".into(),
            context_length: 200_000,
            supported_parameters: vec!["max_tokens".into(), "temperature".into(), "tools".into()],
            pricing: Pricing {
                prompt: 0.000_000_5,
                completion: 0.000_002_5,
                cache_read: Some(0.000_000_05),
            },
        }
    );
    assert_eq!(
        client.model("x/no-cache").await.unwrap().pricing.cache_read,
        None
    );
}

#[tokio::test]
async fn models_are_fetched_once_per_run() {
    let server = server_with_models(1).await;
    let client = OpenRouterClient::with_base_url("sk-or-test", server.uri());
    for id in [
        "anthropic/claude-haiku-5.5",
        "x/no-cache",
        "anthropic/claude-haiku-5.5",
    ] {
        client.model(id).await.unwrap();
    }
    // The server checks `expect(1)` when it is dropped.
}

#[tokio::test]
async fn an_unknown_model_is_an_error() {
    let server = server_with_models(1).await;
    let client = OpenRouterClient::with_base_url("sk-or-test", server.uri());
    assert_eq!(
        client.model("nope/missing").await,
        Err(LlmError::UnknownModel {
            id: "nope/missing".into()
        })
    );
}

#[tokio::test]
async fn a_failed_fetch_is_retried_on_the_next_lookup() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(503).set_body_string("busy"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(models_body()))
        .mount(&server)
        .await;
    let client = OpenRouterClient::with_base_url("sk-or-test", server.uri());
    assert!(matches!(
        client.model("x/no-cache").await,
        Err(LlmError::Server { status: 503, .. })
    ));
    assert_eq!(
        client.model("x/no-cache").await.unwrap().context_length,
        8192
    );
}
