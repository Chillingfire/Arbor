use arbor_config::{ClarificationMode, Config, RoleConfig};

#[test]
fn defaults_apply_when_neither_file_exists() {
    let config = Config::from_toml(None, None).unwrap();
    assert_eq!(
        config.roles.planner,
        RoleConfig {
            model: "anthropic/claude-opus-5.5".into(),
            temperature: 0.5
        }
    );
    assert_eq!(
        config.roles.scaffolder,
        RoleConfig {
            model: "anthropic/claude-sonnet-5.5".into(),
            temperature: 0.3
        }
    );
    assert_eq!(
        config.roles.writer,
        RoleConfig {
            model: "anthropic/claude-haiku-5.5".into(),
            temperature: 0.0
        }
    );
    assert_eq!(config.clarification_mode, ClarificationMode::Ask);
    assert_eq!(config.writer_cap_tokens, 8000);
    assert_eq!(config.failure_brief_tokens, 2000);
    assert_eq!(config.retention_days, 30);
    assert_eq!(config.checks.scaffold, None);
    assert_eq!(config.checks.unit, None);
    assert_eq!(config.checks.unit_filtered, None);
    assert_eq!(config.mainline, None);
    assert!(config.context_files.is_empty());
}

const USER: &str = r#"
api_key = "sk-or-user"
clarification_mode = "assume"

[roles.writer]
model = "google/gemini-3-flash"
temperature = 0.2
"#;

#[test]
fn user_config_overrides_defaults() {
    let config = Config::from_toml(Some(USER), None).unwrap();
    assert_eq!(
        config.roles.writer,
        RoleConfig {
            model: "google/gemini-3-flash".into(),
            temperature: 0.2
        }
    );
    assert_eq!(config.roles.planner.model, "anthropic/claude-opus-5.5");
    assert_eq!(config.clarification_mode, ClarificationMode::Assume);
}

#[test]
fn project_config_overrides_user_config_field_by_field() {
    let project = r#"
clarification_mode = "ask"

[roles.writer]
temperature = 0.0
"#;
    let config = Config::from_toml(Some(USER), Some(project)).unwrap();
    assert_eq!(
        config.roles.writer,
        RoleConfig {
            model: "google/gemini-3-flash".into(),
            temperature: 0.0
        }
    );
    assert_eq!(config.clarification_mode, ClarificationMode::Ask);
}

#[test]
fn project_only_settings_are_read_from_arbor_toml() {
    let project = r#"
mainline = "develop"
writer_cap_tokens = 6000
failure_brief_tokens = 1500
retention_days = 0

[checks]
scaffold = "cargo check"
unit = "cargo test"
unit_filtered = "cargo test {filter}"

[context]
files = ["docs/style.md"]
"#;
    let config = Config::from_toml(None, Some(project)).unwrap();
    assert_eq!(config.mainline.as_deref(), Some("develop"));
    assert_eq!(config.writer_cap_tokens, 6000);
    assert_eq!(config.failure_brief_tokens, 1500);
    assert_eq!(config.retention_days, 0);
    assert_eq!(config.checks.scaffold.as_deref(), Some("cargo check"));
    assert_eq!(config.checks.unit.as_deref(), Some("cargo test"));
    assert_eq!(
        config.checks.unit_filtered.as_deref(),
        Some("cargo test {filter}")
    );
    assert_eq!(config.context_files, ["docs/style.md"]);
}
