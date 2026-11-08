use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// One chat-completions call. Optional settings left as `None` are not sent.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ChatRequest {
    /// An OpenRouter model ID.
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    pub tool_choice: Option<ToolChoice>,
    pub temperature: Option<f64>,
    pub seed: Option<u64>,
    pub max_tokens: Option<u32>,
    /// Route only to providers that honour every parameter, e.g. the Writer's temperature and seed.
    pub require_parameters: bool,
    /// Place a rolling prompt-cache breakpoint on the newest message, so the next call can reuse
    /// this call's whole prefix. Only models that need explicit breakpoints get one.
    #[serde(default)]
    pub cache_transcript: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    System {
        content: Vec<ContentPart>,
    },
    User {
        content: Vec<ContentPart>,
    },
    Assistant {
        content: Option<String>,
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

/// A block of text. `cache` places a prompt-cache breakpoint after it, for models that need
/// explicit breakpoints; other providers cache automatically and ignore it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentPart {
    pub text: String,
    pub cache: bool,
}

impl ContentPart {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cache: false,
        }
    }

    pub fn cached(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cache: true,
        }
    }
}

/// A tool the model may call. `parameters` is a JSON Schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolChoice {
    Auto,
    None,
    /// Force a call to the named tool.
    Tool(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// The arguments as the model wrote them: JSON text that may not be valid.
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    /// The model that actually answered.
    pub model: String,
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: FinishReason,
    pub usage: Usage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishReason {
    Stop,
    ToolCalls,
    /// Cut off by `max_tokens` or the context window.
    Length,
    ContentFilter,
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Input tokens read from the prompt cache. Included in `input_tokens`.
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
}

/// A model's metadata from OpenRouter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub context_length: u64,
    /// Request parameters the model accepts, e.g. `temperature`, `seed`, `tools`.
    pub supported_parameters: Vec<String>,
    pub pricing: Pricing,
}

/// Prices in USD per token.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pricing {
    pub prompt: f64,
    pub completion: f64,
    pub cache_read: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LlmError {
    /// 401: the API key is missing or invalid.
    Auth {
        message: String,
    },
    /// 402: the account is out of credits.
    InsufficientCredits {
        message: String,
    },
    RateLimited {
        retry_after: Option<Duration>,
        message: String,
    },
    /// Any other 4xx: the request itself was refused.
    BadRequest {
        status: u16,
        message: String,
    },
    /// 5xx, or an error the provider reported inside a response.
    Server {
        status: u16,
        message: String,
    },
    Network {
        message: String,
    },
    /// The response wasn't in the expected shape.
    Decode {
        message: String,
    },
    UnknownModel {
        id: String,
    },
    /// The scripted fake couldn't match a request to its fixture.
    Fixture {
        message: String,
    },
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth { message } => write!(f, "OpenRouter rejected the API key: {message}"),
            Self::InsufficientCredits { message } => {
                write!(f, "OpenRouter account is out of credits: {message}")
            }
            Self::RateLimited { message, .. } => write!(f, "rate limited: {message}"),
            Self::BadRequest { status, message } => {
                write!(f, "request refused ({status}): {message}")
            }
            Self::Server { status, message } => write!(f, "server error ({status}): {message}"),
            Self::Network { message } => write!(f, "network error: {message}"),
            Self::Decode { message } => write!(f, "unexpected response: {message}"),
            Self::UnknownModel { id } => write!(f, "unknown model {id}"),
            Self::Fixture { message } => write!(f, "fixture mismatch: {message}"),
        }
    }
}

impl std::error::Error for LlmError {}
