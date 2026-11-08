use std::fs;

use arbor_config::{Config, ConfigError, KeySource, resolve_api_key, save_api_key};
use tempfile::tempdir;

#[test]
fn missing_files_load_as_defaults() {
    let dir = tempdir().unwrap();
    let config = Config::load(Some(&dir.path().join("nope.toml")), dir.path()).unwrap();
    assert_eq!(config, Config::default());
    assert_eq!(Config::load(None, dir.path()).unwrap(), Config::default());
}

#[test]
fn load_reads_the_user_config_and_arbor_toml() {
    let dir = tempdir().unwrap();
    let user = dir.path().join("config.toml");
    fs::write(&user, "[roles.writer]\nmodel = \"x/writer\"\n").unwrap();
    fs::write(dir.path().join("arbor.toml"), "retention_days = 7\n").unwrap();
    let config = Config::load(Some(&user), dir.path()).unwrap();
    assert_eq!(config.roles.writer.model, "x/writer");
    assert_eq!(config.retention_days, 7);
}

#[test]
fn saving_the_key_creates_the_user_config() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("arbor").join("config.toml");
    save_api_key(&path, "sk-or-new").unwrap();
    let text = fs::read_to_string(&path).unwrap();
    let key = resolve_api_key(None, Some(&text)).unwrap().unwrap();
    assert_eq!(key.expose(), "sk-or-new");
    assert_eq!(key.source(), KeySource::UserConfig);
}

#[test]
fn saving_the_key_keeps_the_rest_of_the_user_config() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let original =
        "# my settings\napi_key = \"sk-or-old\"\n\n[roles.writer]\nmodel = \"x/writer\" # cheap\n";
    fs::write(&path, original).unwrap();
    save_api_key(&path, "sk-or-new").unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text, original.replace("sk-or-old", "sk-or-new"));
}

#[test]
fn saving_the_key_never_overwrites_an_unreadable_config() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "roles = [").unwrap();
    assert!(matches!(
        save_api_key(&path, "sk-or-new"),
        Err(ConfigError::Parse { .. })
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), "roles = [");
}

#[cfg(unix)]
#[test]
fn the_saved_user_config_is_readable_only_by_its_owner() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    save_api_key(&path, "sk-or-new").unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}
