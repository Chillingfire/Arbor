#![allow(dead_code)]

use std::fs;
use std::path::Path;

use arbor_config::Checks;
use arbor_tools::{ToolOutput, Tools, Toolset};
use serde_json::Value;

pub fn scaffolder(dir: &Path) -> Tools {
    Tools::new(dir, Toolset::Scaffolder, Checks::default())
}

pub fn planner(dir: &Path) -> Tools {
    Tools::new(dir, Toolset::Planner, Checks::default())
}

pub async fn call(tools: &Tools, name: &str, args: Value) -> ToolOutput {
    tools.call(name, &args.to_string()).await
}

/// Calls a tool that must succeed and returns its text.
pub async fn ok(tools: &Tools, name: &str, args: Value) -> String {
    let output = call(tools, name, args).await;
    assert!(!output.is_error, "{name} failed: {}", output.text);
    output.text
}

/// Calls a tool that must fail and returns its message.
pub async fn err(tools: &Tools, name: &str, args: Value) -> String {
    let output = call(tools, name, args).await;
    assert!(
        output.is_error,
        "{name} should have failed: {}",
        output.text
    );
    output.text
}

pub fn write(dir: &Path, path: &str, text: &str) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
