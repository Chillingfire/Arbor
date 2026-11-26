# The LLM layer

`arbor-llm` holds the model client. All model calls go through OpenRouter, from a single API key.
This is ADR-0007.

## The client trait

Everything that calls a model goes through one trait, so the real client can be swapped for a fake
in tests.

```rust
pub trait LlmClient: Send + Sync {
    fn chat<'a>(&'a self, request: &'a ChatRequest)
        -> BoxFuture<'a, Result<ChatResponse, LlmError>>;
    fn model<'a>(&'a self, id: &'a str)
        -> BoxFuture<'a, Result<ModelInfo, LlmError>>;
}
```

`OpenRouterClient` is the real one. `ScriptedClient` and `RecordingClient` are the test ones. The
pipeline tests replay recorded fixtures through `ScriptedClient`, so the hardest logic runs with no
network.

## Model metadata

`model(id)` returns a `ModelInfo`: the context window, the supported parameters, and the pricing.
The client fetches the full model list once and caches it for the run. The supported parameters
matter for the Writer: it sends temperature and seed only when the model lists them, because the
call also sets `require_parameters`.

## Request building

`request_body` turns a `ChatRequest` into the OpenRouter JSON. The parts that matter:

- `usage.include` is always set, so OpenRouter reports token counts and cost on every call.
- `provider.require_parameters` is set when the request asks for it, which routes the call only to
  providers that honour every parameter sent. The Writer uses this so its temperature and seed are
  actually respected.
- Tools are sent in the OpenAI function format.
- `tool_choice` is `auto`, `none`, or a named function. The agent loop sets it to the submit tool to
  force a submission.

## Prompt caching

OpenRouter caches prompts, which cuts the cost of the repeated prefix in a long agent run. Anthropic
caches only at explicit breakpoints, so the client adds `cache_control` markers for `anthropic/`
models and caps them at four, keeping the newest ones because they cover the longest prefixes.
Other providers cache automatically. The prompt templates are built so the stable part comes first,
which is what makes the cache hit; see [Prompts and Conventions](09-prompts-and-conventions.md).

## Errors

`checked_body` maps the HTTP status to one `LlmError` variant per boundary case, carrying what
callers branch on: `Auth` (401), `InsufficientCredits` (402), `RateLimited` (429, with the
`Retry-After`), `BadRequest` (other 4xx), `Server` (5xx), plus `Network` and `Decode`. A failed
model list fetch is not cached, so a later call can retry it.

## Cost accounting

Every response carries a `Usage`: input tokens, output tokens, cache read and write tokens, and the
cost in dollars. The Autopilot records each call as a `UsageRecorded` event, so the Session's total
and each Tier's total come from replaying the log. The TUI shows per-Tier cost and tokens in its
status bar. The Budget Cap is checked against this total.

## Why OpenRouter only

Any OpenRouter model can fill any Tier from one API key, and per-model metadata, including the
supported parameters and pricing, comes from one place. Arbor depends on OpenRouter features:
`provider.require_parameters` for the Writer, and the `cache_control` pass-through for caching.
Adding direct provider clients later would mean reimplementing those features.
