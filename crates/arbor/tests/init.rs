use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::tempdir;

/// Runs `arbor init` in `dir` with no API key and an empty user config directory.
fn arbor_init(dir: &Path, config_home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_arbor"))
        .arg("init")
        .current_dir(dir)
        .env_remove("OPENROUTER_API_KEY")
        .env("APPDATA", config_home)
        .env("XDG_CONFIG_HOME", config_home)
        .env("HOME", config_home)
        .output()
        .unwrap()
}

#[test]
fn init_writes_the_detected_checks_when_they_pass() {
    let dir = tempdir().unwrap();
    let config_home = tempdir().unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src").join("lib.rs"), "pub fn one() {}\n").unwrap();

    let output = arbor_init(dir.path(), config_home.path());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{output:?}");
    assert!(
        stdout.ends_with("Wrote arbor.toml with the Checks for a Rust project.\n"),
        "{stdout}"
    );
    let written = fs::read_to_string(dir.path().join("arbor.toml")).unwrap();
    assert!(written.contains("unit = \"cargo test\""), "{written}");
}

#[test]
fn without_detected_checks_or_an_api_key_init_fails_and_writes_nothing() {
    let dir = tempdir().unwrap();
    let config_home = tempdir().unwrap();

    let output = arbor_init(dir.path(), config_home.path());

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("OPENROUTER_API_KEY"), "{stderr}");
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("No manifest Arbor recognises"));
    assert!(!dir.path().join("arbor.toml").exists());
}

#[test]
fn init_takes_no_arguments() {
    let output = Command::new(env!("CARGO_BIN_EXE_arbor"))
        .args(["init", "now"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage: arbor [init"));
}
