use std::fmt;

use arbor_config::ClarificationMode;
use arbor_core::{Tier, Usage, Usd};
use arbor_llm::{
    BoxFuture, ChatRequest, ContentPart, LlmClient, LlmError, Message, ModelInfo, ToolCall,
    ToolChoice, ToolDef,
};
use arbor_tools::Tools;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;

/// How many failed submissions an agentic run gets before it escalates.
pub const MAX_SUBMIT_FAILURES: usize = 3;

/// At this share of the model's context window, the model is told to submit now (in percent).
pub const SUBMIT_NOW_PERCENT: u64 = 80;

/// Checks a parsed submission beyond its schema. An `Err` goes back to the model. It may do I/O,
/// such as the Scaffolder running its Scaffold Check.
pub type Validate<T> =
    Box<dyn for<'a> Fn(&'a T) -> BoxFuture<'a, Result<(), String>> + Send + Sync>;

/// The tool an agentic Tier ends its run with. Its parameter schema is generated from `T`.
pub struct Submission<T> {
    pub name: &'static str,
    pub description: &'static str,
    pub validate: Validate<T>,
}

impl<T: Sync + 'static> Submission<T> {
    /// A submission whose validation needs no I/O.
    pub fn new(
        name: &'static str,
        description: &'static str,
        validate: impl Fn(&T) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        Self::with_async(name, description, move |value| {
            let result = validate(value);
            Box::pin(async move { result })
        })
    }

    /// A submission whose validation is async, e.g. to run a Check.
    pub fn with_async(
        name: &'static str,
        description: &'static str,
        validate: impl for<'a> Fn(&'a T) -> BoxFuture<'a, Result<(), String>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name,
            description,
            validate: Box::new(validate),
        }
    }
}

/// What an agentic run needs to know about its Tier and model.
#[derive(Debug, Clone)]
pub struct AgentSettings {
    pub tier: Tier,
    /// The model's OpenRouter metadata, for its ID, tool support and context window.
    pub model: ModelInfo,
    pub temperature: f64,
    pub clarification_mode: ClarificationMode,
}

/// The model can't fill an agentic Tier because it doesn't support tool calling.
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

/// Rejects a model that can't call tools, which the Planner and Scaffolder need.
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

/// One model call and the tool calls it made.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn<T> {
    /// What the call used, to record as `UsageRecorded` after the outcome's own events.
    pub usage: Usage,
    /// `None` while the run goes on: call [`Agent::turn`] again.
    pub outcome: Option<Outcome<T>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome<T> {
    /// A submission that parsed and validated. The run is over.
    Submitted(T),
    /// Clarification Mode is Ask and the model asked the Human Operator these questions, all in
    /// one batch. Pass the answers to [`Agent::answer`], then keep calling [`Agent::turn`].
    Questions(String),
    /// The model failed to submit [`MAX_SUBMIT_FAILURES`] times. The run is over, and the work
    /// goes to the next level of Escalation.
    Escalated { failures: Vec<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TurnError {
    /// The call failed and the transcript is unchanged, so [`Agent::turn`] can be retried.
    Llm(LlmError),
    /// The model asked questions and [`Agent::answer`] hasn't been called yet.
    AwaitingAnswers,
    /// [`Agent::answer`] was called with no questions waiting.
    NoQuestions,
    /// The run already ended with a submission or an Escalation.
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

/// One agentic run of the Planner or Scaffolder: it explores with its tools and ends by calling
/// its submit tool.
///
/// The run advances one model call per [`Agent::turn`], so the caller can record each call's
/// outcome and usage, stop at a budget Checkpoint between calls, and later carry on with the same
/// transcript. Dropping a `turn` future cancels its call and leaves the transcript as it was.
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
    /// `messages` opens the transcript: the rendered prompt and the Session inputs.
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

    /// The transcript so far.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Makes one model call and runs the tools it asks for.
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

    /// Passes on the Human Operator's answers to the model's questions.
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
            ..ChatRequest::default()
        }
    }

    /// The model replied with text instead of calling a tool.
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
                let parsed = self.parse(&call.arguments).await;
                match parsed {
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

    async fn parse(&self, arguments: &str) -> Result<T, String> {
        let value: T = serde_json::from_str(arguments)
            .map_err(|err| format!("the arguments don't match the schema: {err}"))?;
        (self.submission.validate)(&value).await?;
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

    /// Tells the model to submit once its context is [`SUBMIT_NOW_PERCENT`] full.
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

/// The submit tool's parameters: `T`'s JSON Schema.
fn parameters<T: JsonSchema>() -> serde_json::Value {
    let mut schema =
        serde_json::to_value(schemars::schema_for!(T)).expect("a JSON Schema serializes to JSON");
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
    }
    schema
}

/// Converts the provider's report into the Session's exact accounting units.
pub fn core_usage(usage: &arbor_llm::Usage) -> Usage {
    Usage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        cost: Usd::from_dollars(usage.cost_usd),
    }
}
