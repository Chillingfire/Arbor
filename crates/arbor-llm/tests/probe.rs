use arbor_llm::{
    ChatResponse, Exchange, FinishReason, Fixture, LlmError, ModelInfo, OpenRouterClient, Pricing,
    ScriptedClient, Usage, probe_model, probe_request,
};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MODEL: &str = "anthropic/claude-haiku-5.5";

fn model_info(supported_parameters: &[&str]) -> ModelInfo {
    ModelInfo {
        id: MODEL.to_owned(),
        context_length: 200_000,
        supported_parameters: supported_parameters
            .iter()
            .map(|parameter| (*parameter).to_owned())
            .collect(),
        pricing: Pricing {
            prompt: 0.000001,
            completion: 0.000005,
            cache_read: None,
        },
    }
}

fn probe_reply() -> ChatResponse {
    ChatResponse {
        id: "gen-probe".to_owned(),
        model: MODEL.to_owned(),
        content: Some("OK".to_owned()),
        tool_calls: vec![],
        finish_reason: FinishReason::Length,
        usage: Usage {
            input_tokens: 12,
            output_tokens: 1,
            cost_usd: 0.000017,
            ..Usage::default()
        },
    }
}

fn scripted(
    supported_parameters: &[&str],
    probe_response: Result<ChatResponse, LlmError>,
) -> ScriptedClient {
    ScriptedClient::new(Fixture {
        exchanges: vec![Exchange {
            request: probe_request(MODEL, 0.3),
            response: probe_response,
        }],
        models: vec![model_info(supported_parameters)],
    })
}

#[test]
fn the_probe_asks_for_sixteen_tokens_at_the_temperature() {
    let request = probe_request(MODEL, 0.3);
    assert_eq!(request.model, MODEL);
    assert_eq!(request.max_tokens, Some(16));
    assert_eq!(request.temperature, Some(0.3));
    assert!(request.tools.is_empty());
}

#[tokio::test]
async fn a_passed_probe_reports_the_supported_parameters_and_cost() {
    let client = scripted(&["temperature", "seed"], Ok(probe_reply()));
    let report = probe_model(&client, MODEL, 0.3).await.unwrap();
    assert!(report.supports_temperature);
    assert!(report.supports_seed);
    assert_eq!(report.cost_usd, 0.000017);
    assert_eq!(client.remaining(), 0);
}

#[tokio::test]
async fn a_model_without_temperature_or_seed_still_passes_but_says_so() {
    let client = scripted(&["tools"], Ok(probe_reply()));
    let report = probe_model(&client, MODEL, 0.3).await.unwrap();
    assert!(!report.supports_temperature);
    assert!(!report.supports_seed);
}

#[tokio::test]
async fn a_refused_probe_fails_with_the_model_and_temperature() {
    let refusal = LlmError::BadRequest {
        status: 400,
        message: "temperature must be at most 1".to_owned(),
    };
    let client = scripted(&["temperature"], Err(refusal.clone()));
    let failure = probe_model(&client, MODEL, 0.3).await.unwrap_err();
    assert_eq!(failure.error, refusal);
    assert_eq!(
        failure.to_string(),
        "anthropic/claude-haiku-5.5 refused a call at temperature 0.3: request refused (400): \
         temperature must be at most 1"
    );
}

#[tokio::test]
async fn an_unreachable_or_busy_openrouter_is_a_temporary_failure_not_a_refusal() {
    let outage = LlmError::Server {
        status: 503,
        message: "upstream unavailable".to_owned(),
    };
    let client = scripted(&["temperature"], Err(outage));
    let failure = probe_model(&client, MODEL, 0.3).await.unwrap_err();
    assert!(failure.is_temporary());
    assert_eq!(
        failure.to_string(),
        "Could not probe anthropic/claude-haiku-5.5 at temperature 0.3: server error (503): \
         upstream unavailable"
    );
}

#[test]
fn only_errors_that_say_nothing_about_the_request_are_temporary() {
    let temporary = [
        LlmError::Network {
            message: "connection refused".to_owned(),
        },
        LlmError::Server {
            status: 502,
            message: "bad gateway".to_owned(),
        },
        LlmError::RateLimited {
            retry_after: None,
            message: "slow down".to_owned(),
        },
        LlmError::InsufficientCredits {
            message: "top up".to_owned(),
        },
    ];
    let refusals = [
        LlmError::Auth {
            message: "No auth".to_owned(),
        },
        LlmError::BadRequest {
            status: 400,
            message: "temperature must be at most 1".to_owned(),
        },
        LlmError::UnknownModel {
            id: "x/no-such-model".to_owned(),
        },
        LlmError::Decode {
            message: "not JSON".to_owned(),
        },
    ];
    assert!(temporary.iter().all(LlmError::is_temporary));
    assert!(!refusals.iter().any(LlmError::is_temporary));
}

#[tokio::test]
async fn an_unknown_model_fails_before_any_chat_call() {
    let client = scripted(&["temperature"], Ok(probe_reply()));
    let failure = probe_model(&client, "x/no-such-model", 0.3)
        .await
        .unwrap_err();
    assert_eq!(
        failure.to_string(),
        "OpenRouter has no model x/no-such-model"
    );
    assert_eq!(client.remaining(), 1);
}

#[tokio::test]
async fn the_key_check_asks_the_key_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/key"))
        .and(header("authorization", "Bearer sk-or-good"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "data": { "label": "sk-or-…" } })),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/key"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({ "error": { "code": 401, "message": "No auth" } })),
        )
        .mount(&server)
        .await;

    let good = OpenRouterClient::with_base_url("sk-or-good", server.uri());
    assert_eq!(good.check_key().await, Ok(()));
    let bad = OpenRouterClient::with_base_url("sk-or-bad", server.uri());
    assert_eq!(
        bad.check_key().await,
        Err(LlmError::Auth {
            message: "No auth".to_owned()
        })
    );
}
