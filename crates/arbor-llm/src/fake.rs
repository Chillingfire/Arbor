//! Recording real sessions to fixtures and replaying them deterministically in tests.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{BoxFuture, LlmClient};
use crate::types::{ChatRequest, ChatResponse, LlmError, ModelInfo};

/// Every chat call of a session, in order, plus the models it looked up.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Fixture {
    pub exchanges: Vec<Exchange>,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Exchange {
    pub request: ChatRequest,
    pub response: Result<ChatResponse, LlmError>,
}

impl Fixture {
    pub fn load(path: &Path) -> io::Result<Self> {
        serde_json::from_str(&fs::read_to_string(path)?).map_err(io::Error::other)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        fs::write(path, json + "\n")
    }
}

/// Passes calls through to `inner` and records them.
pub struct RecordingClient<C> {
    inner: C,
    fixture: Mutex<Fixture>,
}

impl<C: LlmClient> RecordingClient<C> {
    pub fn new(inner: C) -> Self {
        Self {
            inner,
            fixture: Mutex::new(Fixture::default()),
        }
    }

    /// Everything recorded so far.
    pub fn fixture(&self) -> Fixture {
        self.fixture.lock().unwrap().clone()
    }
}

impl<C: LlmClient> LlmClient for RecordingClient<C> {
    fn chat<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<ChatResponse, LlmError>> {
        Box::pin(async move {
            let response = self.inner.chat(request).await;
            self.fixture.lock().unwrap().exchanges.push(Exchange {
                request: request.clone(),
                response: response.clone(),
            });
            response
        })
    }

    fn model<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ModelInfo, LlmError>> {
        Box::pin(async move {
            let model = self.inner.model(id).await;
            if let Ok(info) = &model {
                let models = &mut self.fixture.lock().unwrap().models;
                if !models.iter().any(|known| known.id == info.id) {
                    models.push(info.clone());
                }
            }
            model
        })
    }
}

/// Replays a fixture's exchanges in order. A request must match its recording exactly.
pub struct ScriptedClient {
    fixture: Fixture,
    next: Mutex<usize>,
}

impl ScriptedClient {
    pub fn new(fixture: Fixture) -> Self {
        Self {
            fixture,
            next: Mutex::new(0),
        }
    }

    /// Exchanges not yet replayed.
    pub fn remaining(&self) -> usize {
        self.fixture.exchanges.len() - *self.next.lock().unwrap()
    }

    fn replay(&self, request: &ChatRequest) -> Result<ChatResponse, LlmError> {
        let mut next = self.next.lock().unwrap();
        let fixture_error = |message| Err(LlmError::Fixture { message });
        let Some(exchange) = self.fixture.exchanges.get(*next) else {
            return fixture_error(format!(
                "call {} has no recorded exchange; the fixture has {}",
                *next + 1,
                self.fixture.exchanges.len()
            ));
        };
        if let Some(path) = first_difference(&to_value(&exchange.request), &to_value(request)) {
            return fixture_error(format!(
                "call {} differs from the recording at `{path}`",
                *next + 1
            ));
        }
        *next += 1;
        exchange.response.clone()
    }
}

impl LlmClient for ScriptedClient {
    fn chat<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<ChatResponse, LlmError>> {
        let response = self.replay(request);
        Box::pin(async move { response })
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

fn to_value(request: &ChatRequest) -> Value {
    serde_json::to_value(request).expect("requests serialize")
}

/// The path of the first place `a` and `b` differ, e.g. `messages[1].content[0].text`.
fn first_difference(a: &Value, b: &Value) -> Option<String> {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            let mut keys: Vec<_> = a.keys().chain(b.keys()).collect();
            keys.sort_unstable();
            keys.dedup();
            keys.into_iter().find_map(|key| {
                match (a.get(key), b.get(key)) {
                    (Some(a), Some(b)) => first_difference(a, b),
                    _ => Some(String::new()),
                }
                .map(|rest| join(key, &rest))
            })
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => a
            .iter()
            .zip(b)
            .enumerate()
            .find_map(|(i, (a, b))| first_difference(a, b).map(|rest| format!("[{i}]{rest}"))),
        _ => (a != b).then(String::new),
    }
}

fn join(key: &str, rest: &str) -> String {
    if rest.is_empty() || rest.starts_with('[') {
        format!("{key}{rest}")
    } else {
        format!("{key}.{rest}")
    }
}
