//! User and project configuration, and API key resolution.

mod config;

pub use config::{
    Checks, ClarificationMode, Config, ConfigError, ConfigFile, FILTER_PLACEHOLDER, RoleConfig,
    Roles,
};
