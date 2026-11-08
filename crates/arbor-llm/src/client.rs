use std::future::Future;
use std::pin::Pin;

use crate::types::{ChatRequest, ChatResponse, LlmError, ModelInfo};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A chat-completions model provider. Dropping a returned future cancels the call.
pub trait LlmClient: Send + Sync {
    fn chat<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<ChatResponse, LlmError>>;

    fn model<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ModelInfo, LlmError>>;
}
