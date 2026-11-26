use arbor_config::{API_KEY_ENV, KeySource, load_api_key, resolve_api_key};

const USER: &str = r#"api_key = "sk-or-user""#;

#[test]
fn loading_reads_the_injected_env_before_the_user_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let user_config = dir.path().join("config.toml");
    std::fs::write(&user_config, USER).unwrap();
    let env_with_key = |name: &str| (name == API_KEY_ENV).then(|| "sk-or-env".to_owned());

    let key = load_api_key(Some(&user_config), env_with_key)
        .unwrap()
        .unwrap();
    assert_eq!(key.source(), KeySource::Env);

    let key = load_api_key(Some(&user_config), |_| None).unwrap().unwrap();
    assert_eq!(key.expose(), "sk-or-user");
    assert!(load_api_key(None, |_| None).unwrap().is_none());
}

#[test]
fn the_env_var_comes_first() {
    let key = resolve_api_key(Some("sk-or-env"), Some(USER))
        .unwrap()
        .unwrap();
    assert_eq!(key.expose(), "sk-or-env");
    assert_eq!(key.source(), KeySource::Env);
}

#[test]
fn the_user_config_is_the_fallback() {
    for env in [None, Some(""), Some("  ")] {
        let key = resolve_api_key(env, Some(USER)).unwrap().unwrap();
        assert_eq!(key.expose(), "sk-or-user");
        assert_eq!(key.source(), KeySource::UserConfig);
    }
}

#[test]
fn no_key_anywhere_means_prompt() {
    assert!(resolve_api_key(None, None).unwrap().is_none());
    assert!(
        resolve_api_key(None, Some("api_key = \"\""))
            .unwrap()
            .is_none()
    );
    assert!(
        resolve_api_key(None, Some("clarification_mode = \"ask\""))
            .unwrap()
            .is_none()
    );
}

#[test]
fn debug_output_hides_the_key() {
    let key = resolve_api_key(Some("sk-or-env"), None).unwrap().unwrap();
    assert!(!format!("{key:?}").contains("sk-or-env"));
}
