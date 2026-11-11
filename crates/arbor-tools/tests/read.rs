mod common;

use common::{call, err, ok, planner, write};
use serde_json::json;
use tempfile::tempdir;

fn numbered(count: usize) -> String {
    (1..=count)
        .map(|line_number| format!("line {line_number}\n"))
        .collect()
}

#[tokio::test]
async fn read_file_numbers_lines() {
    let dir = tempdir().unwrap();
    write(dir.path(), "src/a.txt", "alpha\nbeta\n");
    let listing = ok(
        &planner(dir.path()),
        "read_file",
        json!({"path": "src/a.txt"}),
    )
    .await;
    assert_eq!(listing, "     1\talpha\n     2\tbeta\n");
}

#[tokio::test]
async fn read_file_stops_at_400_lines_and_says_how_to_continue() {
    let dir = tempdir().unwrap();
    write(dir.path(), "big.txt", &numbered(1000));
    let listing = ok(
        &planner(dir.path()),
        "read_file",
        json!({"path": "big.txt"}),
    )
    .await;
    assert!(listing.contains("   400\tline 400\n"));
    assert!(!listing.contains("line 401"));
    assert!(listing.contains("offset=401"), "{listing}");
}

#[tokio::test]
async fn read_file_pages_with_offset_and_limit() {
    let dir = tempdir().unwrap();
    write(dir.path(), "big.txt", &numbered(1000));
    let tools = planner(dir.path());
    let listing = ok(
        &tools,
        "read_file",
        json!({"path": "big.txt", "offset": 998, "limit": 10}),
    )
    .await;
    assert_eq!(
        listing,
        "   998\tline 998\n   999\tline 999\n  1000\tline 1000\n"
    );

    let listing = ok(
        &tools,
        "read_file",
        json!({"path": "big.txt", "offset": 10, "limit": 2}),
    )
    .await;
    assert!(listing.starts_with("    10\tline 10\n    11\tline 11\n"));
    assert!(listing.contains("offset=12"), "{listing}");

    let listing = ok(
        &tools,
        "read_file",
        json!({"path": "big.txt", "limit": 5000}),
    )
    .await;
    assert!(!listing.contains("line 401"), "limit is capped at 400");
}

#[tokio::test]
async fn read_file_past_the_end_is_an_error() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.txt", "one\n");
    let message = err(
        &planner(dir.path()),
        "read_file",
        json!({"path": "a.txt", "offset": 5}),
    )
    .await;
    assert!(message.contains("1 line"), "{message}");
}

#[tokio::test]
async fn read_file_reports_missing_files_and_directories() {
    let dir = tempdir().unwrap();
    write(dir.path(), "src/a.txt", "");
    let tools = planner(dir.path());
    err(&tools, "read_file", json!({"path": "nope.txt"})).await;
    let message = err(&tools, "read_file", json!({"path": "src"})).await;
    assert!(message.contains("list_dir"), "{message}");
}

#[tokio::test]
async fn list_dir_marks_directories_and_respects_gitignore() {
    let dir = tempdir().unwrap();
    write(dir.path(), ".gitignore", "target/\n*.log\n");
    write(dir.path(), "Cargo.toml", "");
    write(dir.path(), "debug.log", "");
    write(dir.path(), "src/main.rs", "");
    write(dir.path(), "target/out", "");
    write(dir.path(), ".github/ci.yml", "");
    let tools = planner(dir.path());
    let listing = ok(&tools, "list_dir", json!({})).await;
    assert_eq!(listing, ".github/\n.gitignore\nCargo.toml\nsrc/\n");
    let listing = ok(&tools, "list_dir", json!({"path": "src"})).await;
    assert_eq!(listing, "src/main.rs\n");
}

#[tokio::test]
async fn list_dir_applies_the_root_gitignore_to_subdirectories() {
    let dir = tempdir().unwrap();
    write(dir.path(), ".gitignore", "generated.rs\n");
    write(dir.path(), "src/lib.rs", "");
    write(dir.path(), "src/generated.rs", "");
    let listing = ok(&planner(dir.path()), "list_dir", json!({"path": "src"})).await;
    assert_eq!(listing, "src/lib.rs\n");
}

#[tokio::test]
async fn bad_arguments_and_unknown_tools_are_errors() {
    let dir = tempdir().unwrap();
    let tools = planner(dir.path());
    let tool_output = tools.call("read_file", "not json").await;
    assert!(tool_output.is_error);
    err(&tools, "read_file", json!({})).await;
    let tool_output = call(&tools, "shell", json!({"command": "ls"})).await;
    assert!(tool_output.is_error);
    assert!(
        tool_output.text.contains("unknown tool"),
        "{}",
        tool_output.text
    );
}
