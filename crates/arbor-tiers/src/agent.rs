use std::fmt;

use arbor_config::ClarificationMode;
use arbor_core::{Tier, Usage, Usd};
use arbor_llm::{
    ChatRequest, ContentPart, LlmClient, LlmError, Message, ModelInfo, ToolCall, ToolChoice,
    ToolDef,
};
use arbor_tools::Tools;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;

pub const MAX_SUBMIT_FAILURES: usize = 3;

pub const SUBMIT_NOW_PERCENT: u64 = 80;

/// The tool that ends an agentic run. Its parameter schema is generated from `T`.
pub struct Submission<T> {
    pub name: &'static str,
    pub description: &'static str,
    /// Checks what the schema can't. An `Err` goes back to the model.
    pub validate: fn(&T) -> Result<(), String>,
}

#[derive(Debug, Clone)]
pub struct AgentSettings {
    pub tier: Tier,
    pub model: ModelInfo,
    pub temperature: f64,
    pub clarification_mode: ClarificationMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedModel {
    pub tier: Tier,
    pub model: String,
}

impl fmt::Display for UnsupportedModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} can't be the {:?}: it doesn't support tool calling",
            self.model, self.tier
        )
    }
}

impl std::error::Error for UnsupportedModel {}

pub fn require_tool_calling(tier: Tier, model: &ModelInfo) -> Result<(), UnsupportedModel> {
    if model.supported_parameters.iter().any(|p| p == "tools") {
        Ok(())
    } else {
        Err(UnsupportedModel {
            tier,
            model: model.id.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Turn<T> {
    /// Record as `UsageRecorded` after the outcome's own events.
    pub usage: Usage,
    pub outcome: Option<Outcome<T>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome<T> {
    Submitted(T),
    /// Answer with [`Agent::answer`], then keep calling [`Agent::turn`].
    Questions(String),
    Escalated {
        failures: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TurnError {
    /// The transcript is unchanged, so the turn can be retried.
    Llm(LlmError),
    AwaitingAnswers,
    NoQuestions,
    Finished,
}

impl fmt::Display for TurnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Llm(err) => err.fmt(f),
            Self::AwaitingAnswers => f.write_str("the model's questions haven't been answered"),
            Self::NoQuestions => f.write_str("the model has no questions waiting"),
            Self::Finished => f.write_str("the run has already ended"),
        }
    }
}

impl std::error::Error for TurnError {}

impl From<LlmError> for TurnError {
    fn from(err: LlmError) -> Self {
        Self::Llm(err)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Running,
    AwaitingAnswers,
    Finished,
}

/// One model call per [`Agent::turn`], so the caller can record each call and stop at a budget
/// Checkpoint between calls.
pub struct Agent<T> {
    settings: AgentSettings,
    submission: Submission<T>,
    tools: Tools,
    definitions: Vec<ToolDef>,
    messages: Vec<Message>,
    state: State,
    asked: bool,
    failures: Vec<String>,
    told_to_submit: bool,
    force_submit: bool,
}

impl<T: DeserializeOwned + JsonSchema> Agent<T> {
    pub fn new(
        settings: AgentSettings,
        submission: Submission<T>,
        tools: Tools,
        messages: Vec<Message>,
    ) -> Result<Self, UnsupportedModel> {
        require_tool_calling(settings.tier, &settings.model)?;
        let mut definitions = tools.definitions();
        definitions.push(ToolDef {
            name: submission.name.to_owned(),
            description: submission.description.to_owned(),
            parameters: parameters::<T>(),
        });
        Ok(Self {
            settings,
            submission,
            tools,
            definitions,
            messages,
            state: State::Running,
            asked: false,
            failures: Vec::new(),
            told_to_submit: false,
            force_submit: false,
        })
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub async fn turn(&mut self, client: &dyn LlmClient) -> Result<Turn<T>, TurnError> {
        match self.state {
            State::Running => {}
            State::AwaitingAnswers => return Err(TurnError::AwaitingAnswers),
            State::Finished => return Err(TurnError::Finished),
        }
        let response = client.chat(&self.request()).await?;
        self.messages.push(Message::Assistant {
            content: response.content.clone(),
            tool_calls: response.tool_calls.clone(),
        });
        let outcome = if response.tool_calls.is_empty() {
            self.on_reply(response.content.as_deref().unwrap_or_default())
        } else {
            self.on_tool_calls(&response.tool_calls).await
        };
        if outcome.is_none() {
            let used = response.usage.input_tokens + response.usage.output_tokens;
            self.check_context(used);
        }
        Ok(Turn {
            usage: core_usage(&response.usage),
            outcome,
        })
    }

    pub fn answer(&mut self, answers: &str) -> Result<(), TurnError> {
        match self.state {
            State::AwaitingAnswers => {}
            State::Running => return Err(TurnError::NoQuestions),
            State::Finished => return Err(TurnError::Finished),
        }
        self.user(format!("The Human Operator's answers:\n\n{answers}"));
        self.state = State::Running;
        Ok(())
    }

    fn request(&self) -> ChatRequest {
        let tool_choice = if self.force_submit {
            ToolChoice::Tool(self.submission.name.to_owned())
        } else {
            ToolChoice::Auto
        };
        ChatRequest {
            model: self.settings.model.id.clone(),
            messages: self.messages.clone(),
            tools: self.definitions.clone(),
            tool_choice: Some(tool_choice),
            temperature: Some(self.settings.temperature),
            cache_transcript: true,
            ..ChatRequest::default()
        }
    }

    fn on_reply(&mut self, text: &str) -> Option<Outcome<T>> {
        let may_ask = self.settings.clarification_mode == ClarificationMode::Ask && !self.asked;
        if may_ask && !text.trim().is_empty() {
            self.asked = true;
            self.state = State::AwaitingAnswers;
            return Some(Outcome::Questions(text.to_owned()));
        }
        let name = self.submission.name;
        let reason = if self.asked {
            "You have already asked the Human Operator your questions in this run."
        } else if self.settings.clarification_mode == ClarificationMode::Assume {
            "Clarification Mode is Assume, so don't ask questions."
        } else {
            "Your reply was empty."
        };
        self.user(format!(
            "{reason} Make reasonable assumptions, record each one as your instructions describe, \
             and call `{name}` now."
        ));
        self.force_submit = true;
        self.fail(format!("replied without calling {name}"))
    }

    async fn on_tool_calls(&mut self, calls: &[ToolCall]) -> Option<Outcome<T>> {
        let mut submitted = None;
        let mut failed = false;
        for call in calls {
            let result = if call.name != self.submission.name {
                let output = self.tools.call(&call.name, &call.arguments).await;
                if output.is_error {
                    format!("Error: {}", output.text)
                } else {
                    output.text
                }
            } else if submitted.is_some() {
                "Ignored: a submission was already accepted.".to_owned()
            } else {
                match self.parse(&call.arguments) {
                    Ok(value) => {
                        submitted = Some(value);
                        "Submitted.".to_owned()
                    }
                    Err(message) => {
                        failed = true;
                        self.failures.push(message.clone());
                        format!("Error: {message}")
                    }
                }
            };
            self.messages.push(Message::Tool {
                tool_call_id: call.id.clone(),
                content: result,
            });
        }
        if let Some(value) = submitted {
            self.state = State::Finished;
            return Some(Outcome::Submitted(value));
        }
        (failed && self.failures.len() >= MAX_SUBMIT_FAILURES).then(|| self.escalate())
    }

    fn parse(&self, arguments: &str) -> Result<T, String> {
        let value: T = serde_json::from_str(arguments)
            .map_err(|err| format!("the arguments don't match the schema: {err}"))?;
        (self.submission.validate)(&value)?;
        Ok(value)
    }

    fn fail(&mut self, message: String) -> Option<Outcome<T>> {
        self.failures.push(message);
        (self.failures.len() >= MAX_SUBMIT_FAILURES).then(|| self.escalate())
    }

    fn escalate(&mut self) -> Outcome<T> {
        self.state = State::Finished;
        Outcome::Escalated {
            failures: std::mem::take(&mut self.failures),
        }
    }

    fn check_context(&mut self, used_tokens: u64) {
        let window = self.settings.model.context_length;
        if self.told_to_submit || used_tokens * 100 < window * SUBMIT_NOW_PERCENT {
            return;
        }
        self.told_to_submit = true;
        self.force_submit = true;
        self.user(format!(
            "You have used {SUBMIT_NOW_PERCENT}% of your context window. Stop exploring and \
             call `{}` now with your best result.",
            self.submission.name
        ));
    }

    fn user(&mut self, text: String) {
        self.messages.push(Message::User {
            content: vec![ContentPart::text(text)],
        });
    }
}

fn parameters<T: JsonSchema>() -> serde_json::Value {
    let mut schema =
        serde_json::to_value(schemars::schema_for!(T)).expect("a JSON Schema serializes to JSON");
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
    }
    schema
}

pub fn core_usage(usage: &arbor_llm::Usage) -> Usage {
    Usage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        cost: Usd::from_dollars(usage.cost_usd),
    }
}
