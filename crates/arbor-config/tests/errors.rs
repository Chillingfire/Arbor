use arbor_config::{Config, ConfigError, ConfigFile};

#[test]
fn invalid_toml_names_the_file() {
    let err = Config::from_toml(Some("roles = ["), None).unwrap_err();
    assert!(matches!(
        err,
        ConfigError::Parse {
            file: ConfigFile::User,
            ..
        }
    ));
}

#[test]
fn a_misspelled_key_is_an_error() {
    let err = Config::from_toml(None, Some("writer_cap = 6000")).unwrap_err();
    assert!(matches!(
        err,
        ConfigError::Parse {
            file: ConfigFile::Project,
            ..
        }
    ));
    assert!(err.to_string().contains("writer_cap"), "{err}");
}

#[test]
fn project_settings_are_refused_in_the_user_config() {
    let err = Config::from_toml(Some("[checks]\nunit = \"cargo test\""), None).unwrap_err();
    assert!(matches!(
        err,
        ConfigError::Parse {
            file: ConfigFile::User,
            ..
        }
    ));
}

#[test]
fn an_api_key_in_arbor_toml_is_refused() {
    let err = Config::from_toml(None, Some("api_key = \"sk-or-leak\"")).unwrap_err();
    assert!(matches!(err, ConfigError::ApiKeyInProject));
    assert!(!err.to_string().contains("sk-or-leak"), "{err}");
}

#[test]
fn a_filtered_check_without_its_placeholder_is_refused() {
    let err =
        Config::from_toml(None, Some("[checks]\nunit_filtered = \"cargo test\"")).unwrap_err();
    assert!(matches!(
        err,
        ConfigError::MissingFilterPlaceholder { template } if template == "cargo test"
    ));
}

#[test]
fn a_parse_error_in_the_user_config_never_shows_the_key() {
    let err = Config::from_toml(Some("api_key = sk-or-secret"), None).unwrap_err();
    let message = err.to_string();
    assert!(!message.contains("sk-or-secret"), "{message}");
    assert!(message.contains("line 1"), "{message}");
}
