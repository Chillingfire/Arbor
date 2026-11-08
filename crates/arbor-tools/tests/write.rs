mod common;

use std::fs;

use common::{err, ok, scaffolder, write};
use serde_json::json;
use tempfile::tempdir;

#[tokio::test]
async fn write_file_creates_parent_directories() {
    let dir = tempdir().unwrap();
    let tools = scaffolder(dir.path());
    ok(
        &tools,
        "write_file",
        json!({"path": "src/new/mod.rs", "content": "pub fn f() {}\n"}),
    )
    .await;
    let text = fs::read_to_string(dir.path().join("src/new/mod.rs")).unwrap();
    assert_eq!(text, "pub fn f() {}\n");
}

#[tokio::test]
async fn write_file_overwrites_files_but_not_directories() {
    let dir = tempdir().unwrap();
    write(dir.path(), "src/a.rs", "old");
    let tools = scaffolder(dir.path());
    ok(
        &tools,
        "write_file",
        json!({"path": "src/a.rs", "content": "new"}),
    )
    .await;
    assert_eq!(
        fs::read_to_string(dir.path().join("src/a.rs")).unwrap(),
        "new"
    );
    err(&tools, "write_file", json!({"path": "src", "content": "x"})).await;
}

#[tokio::test]
async fn writes_are_confined_to_the_worktree() {
    let outer = tempdir().unwrap();
    let work = outer.path().join("work");
    fs::create_dir(&work).unwrap();
    write(&work, ".git/hooks/README", "");
    let tools = scaffolder(&work);
    err(
        &tools,
        "write_file",
        json!({"path": "../evil.txt", "content": "x"}),
    )
    .await;
    err(
        &tools,
        "write_file",
        json!({"path": ".git/hooks/pre-commit", "content": "x"}),
    )
    .await;
    let absolute = outer.path().join("evil.txt");
    err(
        &tools,
        "write_file",
        json!({"path": absolute, "content": "x"}),
    )
    .await;
    err(
        &tools,
        "edit_file",
        json!({"path": "../evil.txt", "old_string": "a", "new_string": "b"}),
    )
    .await;
    assert!(!outer.path().join("evil.txt").exists());
    assert!(!work.join(".git/hooks/pre-commit").exists());
}

#[tokio::test]
async fn edit_file_replaces_a_unique_snippet() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.rs", "fn a() { todo!() }\nfn b() { 1 }\n");
    let tools = scaffolder(dir.path());
    let args = json!({"path": "a.rs", "old_string": "todo!()", "new_string": "42"});
    ok(&tools, "edit_file", args).await;
    let text = fs::read_to_string(dir.path().join("a.rs")).unwrap();
    assert_eq!(text, "fn a() { 42 }\nfn b() { 1 }\n");
}

#[tokio::test]
async fn edit_file_refuses_missing_or_repeated_snippets() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.rs", "x = 1;\nx = 1;\n");
    let tools = scaffolder(dir.path());
    let message = err(
        &tools,
        "edit_file",
        json!({"path": "a.rs", "old_string": "y = 2;", "new_string": "z"}),
    )
    .await;
    assert!(message.contains("not found"), "{message}");
    let message = err(
        &tools,
        "edit_file",
        json!({"path": "a.rs", "old_string": "x = 1;", "new_string": "z"}),
    )
    .await;
    assert!(message.contains("2 times"), "{message}");
    err(
        &tools,
        "edit_file",
        json!({"path": "a.rs", "old_string": "", "new_string": "z"}),
    )
    .await;
    err(
        &tools,
        "edit_file",
        json!({"path": "nope.rs", "old_string": "a", "new_string": "b"}),
    )
    .await;
    assert_eq!(
        fs::read_to_string(dir.path().join("a.rs")).unwrap(),
        "x = 1;\nx = 1;\n"
    );
}

#[tokio::test]
async fn edit_file_matches_lf_snippets_in_crlf_files() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.rs", "fn a() {\r\n    todo!()\r\n}\r\n");
    let tools = scaffolder(dir.path());
    let args =
        json!({"path": "a.rs", "old_string": "{\n    todo!()\n}", "new_string": "{\n    1\n}"});
    ok(&tools, "edit_file", args).await;
    let text = fs::read_to_string(dir.path().join("a.rs")).unwrap();
    assert_eq!(text, "fn a() {\r\n    1\r\n}\r\n");
}

#[cfg(unix)]
#[tokio::test]
async fn writes_through_symlinks_outside_are_rejected() {
    use std::os::unix::fs::symlink;

    let outer = tempdir().unwrap();
    let work = tempdir().unwrap();
    write(outer.path(), "target.txt", "original");
    symlink(outer.path(), work.path().join("linkdir")).unwrap();
    symlink(
        outer.path().join("target.txt"),
        work.path().join("link.txt"),
    )
    .unwrap();
    symlink(
        outer.path().join("missing.txt"),
        work.path().join("dangling.txt"),
    )
    .unwrap();
    let tools = scaffolder(work.path());
    err(
        &tools,
        "write_file",
        json!({"path": "linkdir/new.txt", "content": "x"}),
    )
    .await;
    err(
        &tools,
        "write_file",
        json!({"path": "link.txt", "content": "x"}),
    )
    .await;
    err(
        &tools,
        "write_file",
        json!({"path": "dangling.txt", "content": "x"}),
    )
    .await;
    let args = json!({"path": "link.txt", "old_string": "original", "new_string": "x"});
    err(&tools, "edit_file", args).await;
    assert_eq!(
        fs::read_to_string(outer.path().join("target.txt")).unwrap(),
        "original"
    );
    assert!(!outer.path().join("new.txt").exists());
    assert!(!outer.path().join("missing.txt").exists());
}
