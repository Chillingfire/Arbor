//! Checks that `estimate_tokens` never undercounts the input tokens OpenRouter reports for the
//! default models, and prints the measured bytes per token.
//! Skipped by default; run with
//! `OPENROUTER_API_KEY=... cargo test -p arbor-prompts --test live_tokens -- --ignored --nocapture`.

use std::path::Path;

use arbor_config::{ClarificationMode, Config};
use arbor_core::Request;
use arbor_llm::{ChatRequest, ContentPart, LlmClient, Message, OpenRouterClient};
use arbor_prompts::{Conventions, PlannerInputs, Prompts, estimate_tokens};

/// Input tokens the model reports for a single user message holding `text`.
async fn input_tokens(client: &OpenRouterClient, model: &str, text: &str) -> u64 {
    let request = ChatRequest {
        model: model.to_owned(),
        messages: vec![Message::User {
            content: vec![ContentPart::text(text)],
        }],
        temperature: Some(0.0),
        max_tokens: Some(1),
        ..ChatRequest::default()
    };
    let response = client.chat(&request).await.unwrap();
    response.usage.input_tokens
}

#[tokio::test]
#[ignore = "live: calls OpenRouter for each default model and costs a few cents"]
async fn the_estimate_never_undercounts_real_token_counts() {
    let key = std::env::var("OPENROUTER_API_KEY").expect("live tests need OPENROUTER_API_KEY");
    let client = OpenRouterClient::new(key);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");

    let conventions = Conventions::load(&root, &[]).unwrap();
    let request = Request {
        text: "Add a --verbose flag that logs each tool call.".to_owned(),
        issue: None,
    };
    let planner = Prompts::builtin()
        .planner(
            &conventions,
            &PlannerInputs {
                request: &request,
                clarification_mode: ClarificationMode::Ask,
            },
        )
        .unwrap();
    let code =
        std::fs::read_to_string(root.join("crates/arbor-prompts/src/conventions.rs")).unwrap();
    let samples = [
        ("conventions", conventions.text().to_owned()),
        ("planner instructions", planner.instructions.clone()),
        ("rust source", code),
    ];

    let roles = Config::default().roles;
    let models = [
        roles.planner.model,
        roles.scaffolder.model,
        roles.writer.model,
    ];
    println!("model | sample | bytes | estimate | real | bytes per token | estimate / real");
    let mut worst = f64::INFINITY;
    for model in &models {
        // The message wrapper costs a few tokens; measure it with a one-character message.
        let overhead = input_tokens(&client, model, "x").await.saturating_sub(1);
        for (name, text) in &samples {
            let real = input_tokens(&client, model, text).await - overhead;
            let estimate = u64::from(estimate_tokens(text));
            let ratio = estimate as f64 / real as f64;
            worst = worst.min(ratio);
            println!(
                "{model} | {name} | {} | {estimate} | {real} | {:.2} | {ratio:.2}",
                text.len(),
                text.len() as f64 / real as f64,
            );
        }
    }
    println!("lowest estimate / real: {worst:.2}");
    assert!(worst >= 1.0, "the estimate undercounts real tokens");
}
