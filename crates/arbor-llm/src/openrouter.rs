//! The OpenRouter client (ADR-0007) and its wire format.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use crate::client::{BoxFuture, LlmClient};
use crate::types::{
    ChatRequest, ChatResponse, ContentPart, FinishReason, LlmError, Message, ModelInfo, Pricing,
    ToolCall, ToolChoice, Usage,
};

pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

pub struct OpenRouterClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    /// Fetched on first use and kept for the rest of the run. A failed fetch isn't kept.
    models: OnceCell<HashMap<String, ModelInfo>>,
}

impl OpenRouterClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_url(api_key, OPENROUTER_BASE_URL)
    }

    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .expect("the TLS backend is compiled in");
        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
            models: OnceCell::new(),
        }
    }

    async fn send_chat(&self, request: &ChatRequest) -> Result<ChatResponse, LlmError> {
        let response = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&request_body(request))
            .send()
            .await
            .map_err(network)?;
        parse_chat(&checked_body(response).await?)
    }

    async fn fetch_models(&self) -> Result<HashMap<String, ModelInfo>, LlmError> {
        let response = self
            .http
            .get(format!("{}/models", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(network)?;
        parse_models(&checked_body(response).await?)
    }

    async fn lookup_model(&self, id: &str) -> Result<ModelInfo, LlmError> {
        let models = self.models.get_or_try_init(|| self.fetch_models()).await?;
        models
            .get(id)
            .cloned()
            .ok_or_else(|| LlmError::UnknownModel { id: id.to_owned() })
    }
}

impl LlmClient for OpenRouterClient {
    fn chat<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<ChatResponse, LlmError>> {
        Box::pin(self.send_chat(request))
    }

    fn model<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ModelInfo, LlmError>> {
        Box::pin(self.lookup_model(id))
    }
}

fn network(err: reqwest::Error) -> LlmError {
    LlmError::Network {
        message: err.to_string(),
    }
}

fn decode(message: impl Into<String>) -> LlmError {
    LlmError::Decode {
        message: message.into(),
    }
}

/// Returns the body of a successful response, or the error an unsuccessful one describes.
async fn checked_body(response: reqwest::Response) -> Result<String, LlmError> {
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse().ok())
        .map(Duration::from_secs);
    let body = response.text().await.map_err(network)?;
    if (200..300).contains(&status) {
        return Ok(body);
    }
    let message = serde_json::from_str::<WireErrorBody>(&body)
        .map(|wire| wire.error.message)
        .unwrap_or_else(|_| body.trim().to_owned());
    Err(match status {
        401 => LlmError::Auth { message },
        402 => LlmError::InsufficientCredits { message },
        429 => LlmError::RateLimited {
            retry_after,
            message,
        },
        400..=499 => LlmError::BadRequest { status, message },
        _ => LlmError::Server { status, message },
    })
}

/// The body OpenRouter sends with a non-2xx status.
#[derive(Deserialize)]
struct WireErrorBody {
    error: WireError,
}

/// Anthropic allows at most this many `cache_control` breakpoints in one request.
const MAX_CACHE_BREAKPOINTS: usize = 4;

/// Whether the model only caches at explicit `cache_control` breakpoints. Anthropic's models do;
/// the other providers on OpenRouter cache automatically and don't get breakpoints.
fn needs_cache_control(model: &str) -> bool {
    model.starts_with("anthropic/")
}

fn request_body(request: &ChatRequest) -> Value {
    let cache = needs_cache_control(&request.model);
    let last = request.messages.len().saturating_sub(1);
    let mut messages: Vec<Value> = request
        .messages
        .iter()
        .enumerate()
        .map(|(i, message)| {
            message_json(
                message,
                cache,
                cache && request.cache_transcript && i == last,
            )
        })
        .collect();
    limit_breakpoints(&mut messages);
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "usage": { "include": true },
    });
    if !request.tools.is_empty() {
        body["tools"] = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    }
                })
            })
            .collect();
    }
    if let Some(choice) = &request.tool_choice {
        body["tool_choice"] = match choice {
            ToolChoice::Auto => json!("auto"),
            ToolChoice::None => json!("none"),
            ToolChoice::Tool(name) => json!({ "type": "function", "function": { "name": name } }),
        };
    }
    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(seed) = request.seed {
        body["seed"] = json!(seed);
    }
    if let Some(max_tokens) = request.max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if request.require_parameters {
        body["provider"] = json!({ "require_parameters": true });
    }
    body
}

/// `cache` keeps the parts' own breakpoints; `breakpoint` adds one at the end of the message.
fn message_json(message: &Message, cache: bool, breakpoint: bool) -> Value {
    match message {
        Message::System { content } => {
            json!({ "role": "system", "content": parts_json(content, cache, breakpoint) })
        }
        Message::User { content } => {
            json!({ "role": "user", "content": parts_json(content, cache, breakpoint) })
        }
        Message::Assistant {
            content,
            tool_calls,
        } => {
            let content = match content {
                Some(text) if breakpoint => cached_text(text),
                _ => json!(content),
            };
            let mut message = json!({ "role": "assistant", "content": content });
            if !tool_calls.is_empty() {
                message["tool_calls"] = tool_calls
                    .iter()
                    .map(|call| {
                        json!({
                            "id": call.id,
                            "type": "function",
                            "function": { "name": call.name, "arguments": call.arguments },
                        })
                    })
                    .collect();
            }
            message
        }
        Message::Tool {
            tool_call_id,
            content,
        } => {
            let content = if breakpoint {
                cached_text(content)
            } else {
                json!(content)
            };
            json!({ "role": "tool", "tool_call_id": tool_call_id, "content": content })
        }
    }
}

fn parts_json(parts: &[ContentPart], cache: bool, breakpoint: bool) -> Value {
    let last = parts.len().saturating_sub(1);
    parts
        .iter()
        .enumerate()
        .map(|(i, part)| {
            let mut json = json!({ "type": "text", "text": part.text });
            if (cache && part.cache) || (breakpoint && i == last) {
                json["cache_control"] = json!({ "type": "ephemeral" });
            }
            json
        })
        .collect()
}

/// Plain text as a single content part with a breakpoint after it.
fn cached_text(text: &str) -> Value {
    json!([{ "type": "text", "text": text, "cache_control": { "type": "ephemeral" } }])
}

/// Drops the earliest breakpoints beyond [`MAX_CACHE_BREAKPOINTS`], keeping the newest ones,
/// which cover the longest prefixes.
fn limit_breakpoints(messages: &mut [Value]) {
    let mut kept = 0;
    for message in messages.iter_mut().rev() {
        let Some(parts) = message["content"].as_array_mut() else {
            continue;
        };
        for part in parts.iter_mut().rev() {
            let Some(part) = part.as_object_mut() else {
                continue;
            };
            if part.contains_key("cache_control") {
                if kept < MAX_CACHE_BREAKPOINTS {
                    kept += 1;
                } else {
                    part.remove("cache_control");
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct WireResponse {
    id: String,
    model: String,
    choices: Vec<WireChoice>,
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireChoice {
    finish_reason: Option<String>,
    message: Option<WireMessage>,
    error: Option<WireError>,
}

#[derive(Deserialize)]
struct WireMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<WireToolCall>,
}

#[derive(Deserialize)]
struct WireToolCall {
    id: String,
    function: WireFunction,
}

#[derive(Deserialize)]
struct WireFunction {
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct WireError {
    code: Option<Value>,
    message: String,
}

#[derive(Deserialize)]
struct WireUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    prompt_tokens_details: Option<WirePromptDetails>,
    cost: Option<f64>,
}

#[derive(Deserialize)]
struct WirePromptDetails {
    cached_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
}

fn parse_chat(body: &str) -> Result<ChatResponse, LlmError> {
    let wire: WireResponse = serde_json::from_str(body).map_err(|err| decode(err.to_string()))?;
    let choice = wire
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| decode("no choices in the response"))?;
    if let Some(error) = choice.error {
        return Err(LlmError::Server {
            status: error
                .code
                .and_then(|code| code.as_u64())
                .and_then(|code| u16::try_from(code).ok())
                .unwrap_or(500),
            message: error.message,
        });
    }
    let message = choice
        .message
        .ok_or_else(|| decode("the choice has no message"))?;
    let usage = wire.usage.map_or_else(Usage::default, |usage| {
        let details = usage.prompt_tokens_details;
        Usage {
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            cache_read_tokens: details.as_ref().and_then(|d| d.cached_tokens).unwrap_or(0),
            cache_write_tokens: details
                .as_ref()
                .and_then(|d| d.cache_write_tokens)
                .unwrap_or(0),
            cost_usd: usage.cost.unwrap_or(0.0),
        }
    });
    Ok(ChatResponse {
        id: wire.id,
        model: wire.model,
        content: message.content,
        tool_calls: message
            .tool_calls
            .into_iter()
            .map(|call| ToolCall {
                id: call.id,
                name: call.function.name,
                arguments: call.function.arguments,
            })
            .collect(),
        finish_reason: match choice.finish_reason.as_deref() {
            Some("stop") => FinishReason::Stop,
            Some("tool_calls") => FinishReason::ToolCalls,
            Some("length") => FinishReason::Length,
            Some("content_filter") => FinishReason::ContentFilter,
            other => FinishReason::Other(other.unwrap_or_default().to_owned()),
        },
        usage,
    })
}

#[derive(Deserialize)]
struct WireModels {
    data: Vec<WireModel>,
}

#[derive(Deserialize)]
struct WireModel {
    id: String,
    context_length: Option<u64>,
    pricing: WirePricing,
    #[serde(default)]
    supported_parameters: Vec<String>,
}

/// Prices arrive as decimal strings in USD per token.
#[derive(Deserialize)]
struct WirePricing {
    prompt: String,
    completion: String,
    input_cache_read: Option<String>,
}

fn parse_models(body: &str) -> Result<HashMap<String, ModelInfo>, LlmError> {
    let wire: WireModels = serde_json::from_str(body).map_err(|err| decode(err.to_string()))?;
    let price = |id: &str, value: &str| {
        value
            .parse::<f64>()
            .map_err(|_| decode(format!("model {id} has an invalid price {value:?}")))
    };
    wire.data
        .into_iter()
        .map(|model| {
            let pricing = Pricing {
                prompt: price(&model.id, &model.pricing.prompt)?,
                completion: price(&model.id, &model.pricing.completion)?,
                cache_read: model
                    .pricing
                    .input_cache_read
                    .as_deref()
                    .map(|value| price(&model.id, value))
                    .transpose()?,
            };
            let info = ModelInfo {
                id: model.id.clone(),
                context_length: model.context_length.unwrap_or(0),
                supported_parameters: model.supported_parameters,
                pricing,
            };
            Ok((model.id, info))
        })
        .collect()
}
