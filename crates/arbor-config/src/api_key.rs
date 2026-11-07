use std::fmt;

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
    let found = |value: &str, source| {
        let value = value.trim();
        (!value.is_empty()).then(|| ApiKey {
            value: value.to_owned(),
            source,
        })
    };
    if let Some(key) = env.and_then(|value| found(value, KeySource::Env)) {
        return Ok(Some(key));
    }
    let user: UserFile = parse(user, ConfigFile::User)?;
    Ok(user
        .api_key
        .and_then(|value| found(&value, KeySource::UserConfig)))
}
