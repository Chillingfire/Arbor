#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::Mutex;

use arbor_config::{Checks, ClarificationMode};
use arbor_core::Tier;
use arbor_llm::{
    BoxFuture, ChatRequest, ChatResponse, ContentPart, FinishReason, LlmClient, LlmError, Message,
    ModelInfo, Pricing, ToolCall, Usage,
};
use arbor_tiers::{Agent, AgentSettings, Submission};
use arbor_tools::{Tools, Toolset};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tempfile::TempDir;

#[derive(Default)]
pub struct QueueClient {
    responses: Mutex<VecDeque<Result<ChatResponse, LlmError>>>,
    requests: Mutex<Vec<ChatRequest>>,
}

impl QueueClient {
    pub fn new(responses: impl IntoIterator<Item = ChatResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().map(Ok).collect()),
            requests: Mutex::default(),
        }
    }

    pub fn push_err(&self, err: LlmError) {
        self.responses.lock().unwrap().push_back(Err(err));
    }

    pub fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn last_request(&self) -> ChatRequest {
        self.requests().pop().expect("no request was sent")
    }
}

impl LlmClient for QueueClient {
    fn chat<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<ChatResponse, LlmError>> {
        self.requests.lock().unwrap().push(request.clone());
        let response = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("the model was called more often than scripted");
        Box::pin(async move { response })
    }

    fn model<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ModelInfo, LlmError>> {
        let id = id.to_owned();
        Box::pin(async move { Err(LlmError::UnknownModel { id }) })
    }
}

pub fn model(context_length: u64, tools: bool) -> ModelInfo {
    let mut supported = vec!["temperature".to_owned()];
    if tools {
        supported.push("tools".to_owned());
    }
    ModelInfo {
        id: "anthropic/claude-opus-5.5".into(),
        context_length,
        supported_parameters: supported,
        pricing: Pricing {
            prompt: 0.000005,
            completion: 0.000025,
            cache_read: None,
        },
    }
}

pub fn usage(input_tokens: u64, output_tokens: u64) -> Usage {
    Usage {
        input_tokens,
        output_tokens,
        cost_usd: 0.001,
        ..Usage::default()
    }
}

fn response(content: Option<&str>, tool_calls: Vec<ToolCall>, usage: Usage) -> ChatResponse {
    let finish_reason = if tool_calls.is_empty() {
        FinishReason::Stop
    } else {
        FinishReason::ToolCalls
    };
    ChatResponse {
        id: "gen-1".into(),
        model: "anthropic/claude-opus-5.5".into(),
        content: content.map(str::to_owned),
        tool_calls,
        finish_reason,
        usage,
    }
}

pub fn text(content: &str) -> ChatResponse {
    response(Some(content), vec![], usage(100, 10))
}

pub fn calls(calls: &[(&str, Value)]) -> ChatResponse {
    calls_using(calls, usage(100, 10))
}

pub fn calls_using(calls: &[(&str, Value)], usage: Usage) -> ChatResponse {
    let tool_calls = calls
        .iter()
        .enumerate()
        .map(|(i, (name, arguments))| ToolCall {
            id: format!("call-{i}"),
            name: (*name).to_owned(),
            arguments: arguments.to_string(),
        })
        .collect();
    response(None, tool_calls, usage)
}

/// A stand-in for a Plan.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
pub struct Answer {
    pub answer: u32,
}

pub fn submission() -> Submission<Answer> {
    Submission::new(
        "submit_answer",
        "Submit the answer.",
        |value: &Answer| match value.answer {
            0 => Err("answer must be positive".into()),
            _ => Ok(()),
        },
    )
}

pub fn agent(dir: &TempDir, mode: ClarificationMode, context_length: u64) -> Agent<Answer> {
    let settings = AgentSettings {
        tier: Tier::Planner,
        model: model(context_length, true),
        temperature: 0.5,
        clarification_mode: mode,
    };
    let tools = Tools::new(dir.path(), Toolset::Planner, Checks::default());
    let opening = vec![Message::User {
        content: vec![ContentPart::text("What is the answer?")],
    }];
    Agent::new(settings, submission(), tools, opening).unwrap()
}

pub fn ask(dir: &TempDir) -> Agent<Answer> {
    agent(dir, ClarificationMode::Ask, 200_000)
}

pub fn tool_results(messages: &[Message]) -> Vec<&str> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::Tool { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect()
}

pub fn last_user_text(messages: &[Message]) -> &str {
    messages
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::User { content } => Some(content[0].text.as_str()),
            _ => None,
        })
        .unwrap()
}
