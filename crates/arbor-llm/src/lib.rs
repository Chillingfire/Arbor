//! OpenRouter client behind a trait, plus a scripted fake for tests.

mod client;
mod fake;
mod openrouter;
mod probe;
mod types;

pub use client::{BoxFuture, LlmClient};
pub use fake::{Exchange, Fixture, RecordingClient, ScriptedClient};
pub use openrouter::{OPENROUTER_BASE_URL, OpenRouterClient};
pub use probe::{ProbeFailure, ProbeReport, probe_model, probe_request};
pub use types::{
    ChatRequest, ChatResponse, ContentPart, FinishReason, LlmError, Message, ModelInfo, Pricing,
    ToolCall, ToolChoice, ToolDef, Usage,
};
