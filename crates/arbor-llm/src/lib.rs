//! OpenRouter client behind a trait, plus a scripted fake for tests.

mod client;
mod types;

pub use client::{BoxFuture, LlmClient};
pub use types::{
    ChatRequest, ChatResponse, ContentPart, FinishReason, LlmError, Message, ModelInfo, Pricing,
    ToolCall, ToolChoice, ToolDef, Usage,
};
