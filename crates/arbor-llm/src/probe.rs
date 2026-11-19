//! The probe call that checks a model and temperature before a Session relies on them (grilling
//! log Round 3 decision 6).

use std::fmt;

use crate::client::LlmClient;
use crate::types::{ChatRequest, ContentPart, LlmError, Message};

/// Some routes refuse a smaller output limit: OpenAI's Responses API takes at least 16.
const PROBE_MAX_TOKENS: u32 = 16;

/// The cheapest call that still shows whether the model accepts the request.
pub fn probe_request(model: &str, temperature: f64) -> ChatRequest {
    ChatRequest {
        model: model.to_owned(),
        messages: vec![Message::User {
            content: vec![ContentPart::text("Reply with OK.")],
        }],
        temperature: Some(temperature),
        max_tokens: Some(PROBE_MAX_TOKENS),
        ..ChatRequest::default()
    }
}

/// What a passed probe learned about the model.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeReport {
    /// Whether `supported_parameters` lists `temperature`; if not, providers may ignore it.
    pub supports_temperature: bool,
    /// Whether `supported_parameters` lists `seed`, which only the Writer sends.
    pub supports_seed: bool,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProbeFailure {
    pub model: String,
    pub temperature: f64,
    pub error: LlmError,
}

impl fmt::Display for ProbeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.error {
            LlmError::UnknownModel { .. } => {
                write!(f, "OpenRouter has no model {}", self.model)
            }
            error if error.is_temporary() => write!(
                f,
                "Could not probe {} at temperature {}: {error}",
                self.model, self.temperature
            ),
            error => write!(
                f,
                "{} refused a call at temperature {}: {error}",
                self.model, self.temperature
            ),
        }
    }
}

impl ProbeFailure {
    /// The probe didn't get an answer about the model, so trying again later may pass.
    pub fn is_temporary(&self) -> bool {
        self.error.is_temporary()
    }
}

impl std::error::Error for ProbeFailure {}

/// Looks the model up, then makes one short call at `temperature`.
pub async fn probe_model(
    client: &dyn LlmClient,
    model: &str,
    temperature: f64,
) -> Result<ProbeReport, ProbeFailure> {
    let failure = |error| ProbeFailure {
        model: model.to_owned(),
        temperature,
        error,
    };
    let model_info = client.model(model).await.map_err(failure)?;
    let probe_response = client
        .chat(&probe_request(model, temperature))
        .await
        .map_err(failure)?;
    let supports = |parameter: &str| {
        model_info
            .supported_parameters
            .iter()
            .any(|supported| supported == parameter)
    };
    Ok(ProbeReport {
        supports_temperature: supports("temperature"),
        supports_seed: supports("seed"),
        cost_usd: probe_response.usage.cost_usd,
    })
}
