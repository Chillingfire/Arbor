//! User and project configuration, and API key resolution.

mod api_key;
mod config;
mod detect;
mod files;
mod paths;

pub use api_key::{API_KEY_ENV, ApiKey, KeySource, resolve_api_key};
pub use config::{
    Checks, ClarificationMode, Config, ConfigError, ConfigFile, FILTER_PLACEHOLDER, RoleConfig,
    Roles,
};
pub use detect::{
    Detection, PROJECT_TYPES, ProjectType, detect_project_type, detect_project_type_in,
};
pub use files::{PROJECT_CONFIG_FILE, save_api_key, save_project_checks};
pub use paths::{Platform, user_config_path};
