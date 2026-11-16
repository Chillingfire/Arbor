use std::fmt;
use std::path::Path;

use crate::config::{ConfigError, ConfigFile, UserFile, parse};

/// The env var checked before the user config.
pub const API_KEY_ENV: &str = "OPENROUTER_API_KEY";

/// An OpenRouter API key. `Debug` never prints it.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey {
    value: String,
    source: KeySource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Env,
    UserConfig,
}

impl ApiKey {
    pub fn expose(&self) -> &str {
        &self.value
    }

    pub fn source(&self) -> KeySource {
        self.source
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiKey")
            .field("value", &"<redacted>")
            .field("source", &self.source)
            .finish()
    }
}

/// Finds the API key: the `OPENROUTER_API_KEY` value first, then the user config's text. `None`
/// means neither has one, so the operator must be prompted. Blank values count as unset.
pub fn resolve_api_key(
    env: Option<&str>,
    user: Option<&str>,
) -> Result<Option<ApiKey>, ConfigError> {
    let found = |candidate: &str, source| {
        let trimmed_key = candidate.trim();
        (!trimmed_key.is_empty()).then(|| ApiKey {
            value: trimmed_key.to_owned(),
            source,
        })
    };
    if let Some(key) = env.and_then(|env_key| found(env_key, KeySource::Env)) {
        return Ok(Some(key));
    }
    let user: UserFile = parse(user, ConfigFile::User)?;
    Ok(user
        .api_key
        .and_then(|user_key| found(&user_key, KeySource::UserConfig)))
}

/// Reads the key from `OPENROUTER_API_KEY`, then from the user config file. A user config that
/// can't be read counts as absent.
pub fn load_api_key(user_config: Option<&Path>) -> Result<Option<ApiKey>, ConfigError> {
    let env_key = std::env::var(API_KEY_ENV).ok();
    let user_text = user_config.and_then(|path| std::fs::read_to_string(path).ok());
    resolve_api_key(env_key.as_deref(), user_text.as_deref())
}
