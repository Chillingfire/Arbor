//! Calls the real OpenRouter API. Skipped by default; run with
//! `OPENROUTER_API_KEY=... cargo test -p arbor-llm --test live -- --ignored`.

use arbor_llm::{ChatRequest, ContentPart, LlmClient, Message, OpenRouterClient};

const MODEL: &str = "anthropic/claude-haiku-5.5";

fn client() -> OpenRouterClient {
    let key = std::env::var("OPENROUTER_API_KEY").expect("live tests need OPENROUTER_API_KEY");
    OpenRouterClient::new(key)
}

#[tokio::test]
#[ignore = "live: calls OpenRouter and costs a fraction of a cent"]
async fn a_real_chat_call_returns_text_usage_and_cost() {
    let request = ChatRequest {
        model: MODEL.into(),
        messages: vec![Message::User {
            content: vec![ContentPart::text("Reply with the single word OK.")],
        }],
        temperature: Some(0.0),
        max_tokens: Some(5),
        ..ChatRequest::default()
    };
    let chat_response = client().chat(&request).await.unwrap();
    assert!(!chat_response.content.unwrap_or_default().trim().is_empty());
    assert!(chat_response.usage.input_tokens > 0);
    assert!(chat_response.usage.output_tokens > 0);
    assert!(chat_response.usage.cost_usd > 0.0);
}

#[tokio::test]
#[ignore = "live: calls OpenRouter"]
async fn real_model_metadata_has_a_context_window_and_prices() {
    let model = client().model(MODEL).await.unwrap();
    assert!(model.context_length > 0);
    assert!(model.pricing.prompt > 0.0);
    assert!(
        model
            .supported_parameters
            .iter()
            .any(|p| p == "temperature")
    );
}
