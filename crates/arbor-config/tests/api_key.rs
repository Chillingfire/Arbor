use arbor_config::{KeySource, resolve_api_key};

const USER: &str = r#"api_key = "sk-or-user""#;

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
