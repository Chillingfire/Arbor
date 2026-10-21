# OpenRouter is the only model provider

All model calls go through OpenRouter, so any OpenRouter model can fill any Tier from a single API key, with per-model metadata (context window, supported parameters, pricing) available from one place. Arbor depends on OpenRouter-specific features, including `provider.require_parameters` to route Writer calls only to providers that honour temperature and seed, and pass-through of `cache_control` for prompt caching. Adding direct provider clients later would mean reimplementing those features.
