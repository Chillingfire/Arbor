use std::path::{Path, PathBuf};

use arbor_config::{Platform, user_config_path};

fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |name| {
        vars.iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| (*value).to_owned())
    }
}

fn under(base: &str, parts: &[&str]) -> PathBuf {
    parts
        .iter()
        .fold(Path::new(base).to_path_buf(), |path, part| path.join(part))
}

#[test]
fn unix_uses_dot_config_under_home() {
    let path = user_config_path(Platform::Unix, env(&[("HOME", "/home/dan")]));
    assert_eq!(
        path,
        Some(under("/home/dan", &[".config", "arbor", "config.toml"]))
    );
}

#[test]
fn unix_respects_xdg_config_home() {
    let vars = [("HOME", "/home/dan"), ("XDG_CONFIG_HOME", "/xdg")];
    let path = user_config_path(Platform::Unix, env(&vars));
    assert_eq!(path, Some(under("/xdg", &["arbor", "config.toml"])));

    let vars = [("HOME", "/home/dan"), ("XDG_CONFIG_HOME", "")];
    let path = user_config_path(Platform::Unix, env(&vars));
    assert_eq!(
        path,
        Some(under("/home/dan", &[".config", "arbor", "config.toml"]))
    );
}

#[test]
fn windows_uses_appdata() {
    let vars = [("APPDATA", r"C:\Users\dan\AppData\Roaming")];
    let path = user_config_path(Platform::Windows, env(&vars));
    assert_eq!(
        path,
        Some(under(
            r"C:\Users\dan\AppData\Roaming",
            &["arbor", "config.toml"]
        ))
    );
}

#[test]
fn no_home_directory_means_no_user_config() {
    assert_eq!(user_config_path(Platform::Unix, env(&[])), None);
    assert_eq!(user_config_path(Platform::Windows, env(&[])), None);
}
