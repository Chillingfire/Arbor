//! OpenRouter client behind a trait, plus a scripted fake for tests.

mod client;
mod openrouter;
mod types;

pub use client::{BoxFuture, LlmClient};
pub use openrouter::{OPENROUTER_BASE_URL, OpenRouterClient};
pub use types::{
    ChatRequest, ChatResponse, ContentPart, FinishReason, LlmError, Message, ModelInfo, Pricing,
    ToolCall, ToolChoice, ToolDef, Usage,
};
