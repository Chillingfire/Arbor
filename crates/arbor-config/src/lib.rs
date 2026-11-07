//! User and project configuration, and API key resolution.

mod api_key;
mod config;
mod paths;

pub use api_key::{API_KEY_ENV, ApiKey, KeySource, resolve_api_key};
pub use config::{
    Checks, ClarificationMode, Config, ConfigError, ConfigFile, FILTER_PLACEHOLDER, RoleConfig,
    Roles,
};
pub use paths::{Platform, user_config_path};
