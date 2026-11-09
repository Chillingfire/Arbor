use std::fmt;
use std::io;
use std::path::PathBuf;

use serde::Deserialize;

/// The merged configuration: defaults, then the user config, then the project's `arbor.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub roles: Roles,
    pub clarification_mode: ClarificationMode,
    pub checks: Checks,
    /// Overrides the mainline taken from `origin/HEAD`.
    pub mainline: Option<String>,
    /// Extra Conventions files for the Planner and Scaffolder.
    pub context_files: Vec<String>,
    /// The Writer Cap.
    pub writer_cap_tokens: u32,
    /// The Failure Brief allowance, on top of the Writer Cap.
    pub failure_brief_tokens: u32,
    /// Sessions idle this long are pruned. 0 means never.
    pub retention_days: u32,
    /// The tool calls one Scaffolder Revision may make before it must submit.
    pub revision_tool_call_budget: u32,
    /// Pause at a Checkpoint after each Revision, not only before the Planner.
    pub revision_checkpoints: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Roles {
    pub planner: RoleConfig,
    pub scaffolder: RoleConfig,
    pub writer: RoleConfig,
}

/// The model that fills a Tier, as an OpenRouter model ID.
#[derive(Debug, Clone, PartialEq)]
pub struct RoleConfig {
    pub model: String,
    pub temperature: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClarificationMode {
    #[default]
    Ask,
    Assume,
}

/// Commands from `arbor.toml`. `unit_filtered` contains `{filter}`, replaced by a Work Unit's
/// test filter.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Checks {
    pub scaffold: Option<String>,
    pub unit: Option<String>,
    pub unit_filtered: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            roles: Roles {
                planner: RoleConfig {
                    model: "anthropic/claude-opus-5.5".into(),
                    temperature: 0.5,
                },
                scaffolder: RoleConfig {
                    model: "anthropic/claude-sonnet-5.5".into(),
                    temperature: 0.3,
                },
                writer: RoleConfig {
                    model: "anthropic/claude-haiku-5.5".into(),
                    temperature: 0.0,
                },
            },
            clarification_mode: ClarificationMode::default(),
            checks: Checks::default(),
            mainline: None,
            context_files: Vec::new(),
            writer_cap_tokens: 8000,
            failure_brief_tokens: 2000,
            retention_days: 30,
            revision_tool_call_budget: 15,
            revision_checkpoints: false,
        }
    }
}

/// Replaced by a Work Unit's test filter in `checks.unit_filtered`.
pub const FILTER_PLACEHOLDER: &str = "{filter}";

/// Which config file an error came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFile {
    User,
    Project,
}

#[derive(Debug)]
pub enum ConfigError {
    /// Holds only the parser's message and line, never the quoted source line, which could hold
    /// the API key.
    Parse {
        file: ConfigFile,
        line: Option<usize>,
        message: String,
    },
    /// The key belongs only in the user config, so it never lands in a repo.
    ApiKeyInProject,
    MissingFilterPlaceholder {
        template: String,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse {
                file,
                line: Some(line),
                message,
            } => write!(f, "invalid {file:?} config at line {line}: {message}"),
            Self::Parse {
                file,
                line: None,
                message,
            } => write!(f, "invalid {file:?} config: {message}"),
            Self::ApiKeyInProject => write!(
                f,
                "arbor.toml must not contain api_key; put it in the user config or OPENROUTER_API_KEY"
            ),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::MissingFilterPlaceholder { template } => write!(
                f,
                "checks.unit_filtered must contain {FILTER_PLACEHOLDER}: {template:?}"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

/// The user config file. Every field is optional and unknown keys are errors.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct UserFile {
    pub(crate) api_key: Option<String>,
    clarification_mode: Option<ClarificationMode>,
    #[serde(default)]
    roles: RolesFile,
}

/// The project's `arbor.toml`.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ProjectFile {
    clarification_mode: Option<ClarificationMode>,
    #[serde(default)]
    roles: RolesFile,
    #[serde(default)]
    checks: ChecksFile,
    mainline: Option<String>,
    #[serde(default)]
    context: ContextFile,
    writer_cap_tokens: Option<u32>,
    failure_brief_tokens: Option<u32>,
    retention_days: Option<u32>,
    #[serde(default)]
    revisions: RevisionsFile,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RevisionsFile {
    tool_call_budget: Option<u32>,
    checkpoints: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RolesFile {
    planner: Option<RoleFile>,
    scaffolder: Option<RoleFile>,
    writer: Option<RoleFile>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RoleFile {
    model: Option<String>,
    temperature: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ChecksFile {
    scaffold: Option<String>,
    unit: Option<String>,
    unit_filtered: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ContextFile {
    #[serde(default)]
    files: Vec<String>,
}

pub(crate) fn parse<T: for<'de> Deserialize<'de> + Default>(
    text: Option<&str>,
    file: ConfigFile,
) -> Result<T, ConfigError> {
    text.map_or_else(
        || Ok(T::default()),
        |text| {
            toml::from_str(text).map_err(|err| ConfigError::Parse {
                file,
                line: err
                    .span()
                    .map(|span| text[..span.start].matches('\n').count() + 1),
                message: err.message().trim_end().to_owned(),
            })
        },
    )
}

fn has_api_key(text: &str) -> bool {
    text.parse::<toml::Table>()
        .is_ok_and(|table| table.contains_key("api_key"))
}

impl RoleConfig {
    fn merge(&mut self, file: Option<RoleFile>) {
        if let Some(file) = file {
            if let Some(model) = file.model {
                self.model = model;
            }
            if let Some(temperature) = file.temperature {
                self.temperature = temperature;
            }
        }
    }
}

impl Roles {
    fn merge(&mut self, file: RolesFile) {
        self.planner.merge(file.planner);
        self.scaffolder.merge(file.scaffolder);
        self.writer.merge(file.writer);
    }
}

impl Config {
    /// Merges the user config and the project's `arbor.toml`, given as their text. `None` means
    /// the file doesn't exist.
    pub fn from_toml(user: Option<&str>, project: Option<&str>) -> Result<Self, ConfigError> {
        let user: UserFile = parse(user, ConfigFile::User)?;
        if project.is_some_and(has_api_key) {
            return Err(ConfigError::ApiKeyInProject);
        }
        let project: ProjectFile = parse(project, ConfigFile::Project)?;
        let mut config = Self::default();

        config.roles.merge(user.roles);
        config.roles.merge(project.roles);
        config.clarification_mode = project
            .clarification_mode
            .or(user.clarification_mode)
            .unwrap_or_default();
        if let Some(template) = &project.checks.unit_filtered
            && !template.contains(FILTER_PLACEHOLDER)
        {
            return Err(ConfigError::MissingFilterPlaceholder {
                template: template.clone(),
            });
        }
        config.checks = Checks {
            scaffold: project.checks.scaffold,
            unit: project.checks.unit,
            unit_filtered: project.checks.unit_filtered,
        };
        config.mainline = project.mainline;
        config.context_files = project.context.files;
        if let Some(tokens) = project.writer_cap_tokens {
            config.writer_cap_tokens = tokens;
        }
        if let Some(tokens) = project.failure_brief_tokens {
            config.failure_brief_tokens = tokens;
        }
        if let Some(days) = project.retention_days {
            config.retention_days = days;
        }
        if let Some(budget) = project.revisions.tool_call_budget {
            config.revision_tool_call_budget = budget;
        }
        if let Some(checkpoints) = project.revisions.checkpoints {
            config.revision_checkpoints = checkpoints;
        }
        Ok(config)
    }
}
