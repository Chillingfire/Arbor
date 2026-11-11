//! The scripted fake LLM for the pipeline tests: a [`ScriptedClient`] replaying a recorded
//! fixture from `tests/fixtures/pipeline`.
//!
//! The fixture holds every response the scenario's models give, and the exact request each one
//! answers. To record the requests again after a prompt or the pipeline changes, run
//!
//! ```text
//! ARBOR_RECORD_FIXTURES=1 cargo test -p arbor --test pipeline
//! ```
//!
//! which keeps each fixture's responses, in order, and rewrites its requests from what the
//! pipeline sends now. Check the diff of the fixtures before committing it.
//!
//! Check output holds paths and timings that differ between machines, so it is left out of the
//! requests on both sides; [`FixtureClient::sent`] keeps the requests as they were sent.

use std::path::PathBuf;
use std::sync::Mutex;

use arbor_llm::{
    BoxFuture, ChatRequest, ChatResponse, Fixture, LlmClient, LlmError, Message, ModelInfo,
    RecordingClient, ScriptedClient,
};

const RECORD_VARIABLE: &str = "ARBOR_RECORD_FIXTURES";

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pipeline");

/// Stands in for the lines of a Check's output after its exit code.
pub const CHECK_OUTPUT_LEFT_OUT: &str = "[Check output left out of the fixture]";

pub struct FixtureClient {
    path: PathBuf,
    mode: Mode,
    sent: Mutex<Vec<ChatRequest>>,
}

enum Mode {
    Replay(ScriptedClient),
    Record {
        recorder: RecordingClient<RecordedResponses>,
        models: Vec<ModelInfo>,
    },
}

impl FixtureClient {
    /// Replays `tests/fixtures/pipeline/<scenario>.json`, or records its requests again when
    /// `ARBOR_RECORD_FIXTURES` is set.
    pub fn new(scenario: &str) -> Self {
        let path = PathBuf::from(FIXTURES).join(format!("{scenario}.json"));
        let fixture = Fixture::load(&path)
            .unwrap_or_else(|err| panic!("can't load {}: {err}", path.display()));
        let mode = if std::env::var_os(RECORD_VARIABLE).is_some() {
            Mode::Record {
                models: fixture.models.clone(),
                recorder: RecordingClient::new(RecordedResponses::new(fixture)),
            }
        } else {
            Mode::Replay(ScriptedClient::new(fixture))
        };
        Self {
            path,
            mode,
            sent: Mutex::default(),
        }
    }

    /// Every request the pipeline sent, in order, with the Check output still in it.
    pub fn sent(&self) -> Vec<ChatRequest> {
        self.sent.lock().unwrap().clone()
    }

    /// Fails unless the pipeline used every recorded exchange. When recording, saves the fixture.
    pub fn finish(&self) {
        match &self.mode {
            Mode::Replay(scripted) => assert_eq!(
                scripted.remaining(),
                0,
                "the pipeline stopped before replaying all of {}",
                self.path.display()
            ),
            Mode::Record { recorder, models } => {
                let fixture = Fixture {
                    models: models.clone(),
                    ..recorder.fixture()
                };
                fixture.save(&self.path).unwrap();
            }
        }
    }
}

impl LlmClient for FixtureClient {
    fn chat<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<ChatResponse, LlmError>> {
        self.sent.lock().unwrap().push(request.clone());
        let comparable = without_check_output(request);
        Box::pin(async move {
            let reply = match &self.mode {
                Mode::Replay(scripted) => scripted.chat(&comparable).await,
                Mode::Record { recorder, .. } => recorder.chat(&comparable).await,
            };
            if let Err(LlmError::Fixture { message }) = &reply {
                panic!(
                    "{message} in {}. If the change to the requests is intended, record them \
                     again with {RECORD_VARIABLE}=1.",
                    self.path.display()
                );
            }
            reply
        })
    }

    fn model<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ModelInfo, LlmError>> {
        match &self.mode {
            Mode::Replay(scripted) => scripted.model(id),
            Mode::Record { recorder, .. } => recorder.model(id),
        }
    }
}

/// A fixture's responses in order, whatever the requests. Only used to record requests again.
struct RecordedResponses {
    fixture: Fixture,
    next: Mutex<usize>,
}

impl RecordedResponses {
    fn new(fixture: Fixture) -> Self {
        Self {
            fixture,
            next: Mutex::new(0),
        }
    }
}

impl LlmClient for RecordedResponses {
    fn chat<'a>(
        &'a self,
        _request: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<ChatResponse, LlmError>> {
        let mut next = self.next.lock().unwrap();
        let exchange = self
            .fixture
            .exchanges
            .get(*next)
            .expect("the pipeline made more calls than the fixture has responses");
        *next += 1;
        let recorded_response = exchange.response.clone();
        Box::pin(async move { recorded_response })
    }

    fn model<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ModelInfo, LlmError>> {
        let model = self
            .fixture
            .models
            .iter()
            .find(|model| model.id == id)
            .cloned()
            .ok_or_else(|| LlmError::UnknownModel { id: id.to_owned() });
        Box::pin(async move { model })
    }
}

fn without_check_output(request: &ChatRequest) -> ChatRequest {
    let mut comparable = request.clone();
    for message in &mut comparable.messages {
        match message {
            Message::System { content } | Message::User { content } => {
                for part in content {
                    part.text = leave_out_check_output(&part.text);
                }
            }
            Message::Assistant {
                content: Some(text),
                ..
            }
            | Message::Tool { content: text, .. } => *text = leave_out_check_output(text),
            Message::Assistant { content: None, .. } => {}
        }
    }
    comparable
}

/// Keeps a Check's `$ cargo ...` line and its exit code, and replaces the output after them, up to
/// the fence closing it or the end of the text, with [`CHECK_OUTPUT_LEFT_OUT`].
pub fn leave_out_check_output(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut lines = text.split_inclusive('\n');
    while let Some(line) = lines.next() {
        kept.push_str(line);
        if !line.starts_with("$ cargo ") {
            continue;
        }
        if let Some(exit_code) = lines.next() {
            kept.push_str(exit_code);
        }
        kept.push_str(CHECK_OUTPUT_LEFT_OUT);
        kept.push('\n');
        for output_line in lines.by_ref() {
            if output_line.starts_with("```") {
                kept.push_str(output_line);
                break;
            }
        }
    }
    kept
}
