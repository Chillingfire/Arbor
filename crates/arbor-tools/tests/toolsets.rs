mod common;

use common::{call, planner, scaffolder};
use serde_json::json;
use tempfile::tempdir;

fn names(tools: &arbor_tools::Tools) -> Vec<String> {
    tools
        .definitions()
        .into_iter()
        .map(|def| def.name)
        .collect()
}

#[test]
fn planner_only_reads() {
    let dir = tempdir().unwrap();
    assert_eq!(
        names(&planner(dir.path())),
        ["read_file", "list_dir", "glob", "grep"]
    );
}

#[test]
fn scaffolder_adds_writes_and_has_no_shell() {
    let dir = tempdir().unwrap();
    assert_eq!(
        names(&scaffolder(dir.path())),
        [
            "read_file",
            "list_dir",
            "glob",
            "grep",
            "write_file",
            "edit_file"
        ]
    );
}

#[test]
fn definitions_carry_object_schemas() {
    let dir = tempdir().unwrap();
    for def in scaffolder(dir.path()).definitions() {
        assert!(!def.description.is_empty(), "{}", def.name);
        assert!(!def.description.contains("  "), "{}", def.description);
        assert_eq!(def.parameters["type"], "object", "{}", def.name);
        assert!(def.parameters.get("$schema").is_none(), "{}", def.name);
    }
}

#[tokio::test]
async fn planner_is_refused_scaffolder_tools() {
    let dir = tempdir().unwrap();
    let tools = planner(dir.path());
    let output = call(
        &tools,
        "write_file",
        json!({"path": "a.txt", "content": "x"}),
    )
    .await;
    assert!(output.is_error);
    assert!(output.text.contains("Planner"), "{}", output.text);
    assert!(!dir.path().join("a.txt").exists());
    let args = json!({"path": "a.txt", "old_string": "a", "new_string": "b"});
    assert!(call(&tools, "edit_file", args).await.is_error);
}
