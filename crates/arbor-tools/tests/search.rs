mod common;

use common::{err, ok, planner, write};
use serde_json::json;
use tempfile::tempdir;

#[tokio::test]
async fn glob_matches_paths_and_respects_gitignore() {
    let dir = tempdir().unwrap();
    write(dir.path(), ".gitignore", "target/\n");
    write(dir.path(), "src/main.rs", "");
    write(dir.path(), "src/deep/mod.rs", "");
    write(dir.path(), "README.md", "");
    write(dir.path(), "target/build.rs", "");
    let tools = planner(dir.path());
    let matches = ok(&tools, "glob", json!({"pattern": "**/*.rs"})).await;
    assert_eq!(matches, "src/deep/mod.rs\nsrc/main.rs\n");
    let matches = ok(&tools, "glob", json!({"pattern": "*.rs"})).await;
    assert!(
        matches.contains("No files match"),
        "* doesn't cross /: {matches}"
    );
    let matches = ok(&tools, "glob", json!({"pattern": "*.rs", "path": "src"})).await;
    assert_eq!(matches, "src/main.rs\n");
}

#[tokio::test]
async fn glob_caps_results_with_a_note() {
    let dir = tempdir().unwrap();
    for file_number in 0..250 {
        write(dir.path(), &format!("f{file_number:03}.txt"), "");
    }
    let matches = ok(&planner(dir.path()), "glob", json!({"pattern": "*.txt"})).await;
    assert_eq!(matches.lines().count(), 201);
    assert!(
        matches.ends_with("[50 more files not shown. Narrow the search.]\n"),
        "{matches}"
    );
}

#[tokio::test]
async fn glob_rejects_bad_patterns_and_paths() {
    let dir = tempdir().unwrap();
    let tools = planner(dir.path());
    err(&tools, "glob", json!({"pattern": "[unclosed"})).await;
    err(&tools, "glob", json!({"pattern": "*", "path": "../"})).await;
}

#[tokio::test]
async fn grep_reports_path_line_and_text() {
    let dir = tempdir().unwrap();
    write(dir.path(), "src/a.rs", "fn main() {}\nfn helper() {}\n");
    write(dir.path(), "src/b.rs", "struct Helper;\n");
    let tools = planner(dir.path());
    let matches = ok(&tools, "grep", json!({"pattern": "fn \\w+"})).await;
    assert_eq!(
        matches,
        "src/a.rs:1: fn main() {}\nsrc/a.rs:2: fn helper() {}\n"
    );
    let matches = ok(
        &tools,
        "grep",
        json!({"pattern": "helper", "case_insensitive": true}),
    )
    .await;
    assert_eq!(
        matches,
        "src/a.rs:2: fn helper() {}\nsrc/b.rs:1: struct Helper;\n"
    );
}

#[tokio::test]
async fn grep_filters_by_path_and_glob() {
    let dir = tempdir().unwrap();
    write(dir.path(), "src/a.rs", "needle\n");
    write(dir.path(), "src/a.md", "needle\n");
    write(dir.path(), "docs/b.rs", "needle\n");
    let tools = planner(dir.path());
    let matches = ok(&tools, "grep", json!({"pattern": "needle", "glob": "*.rs"})).await;
    assert_eq!(matches, "docs/b.rs:1: needle\nsrc/a.rs:1: needle\n");
    let matches = ok(&tools, "grep", json!({"pattern": "needle", "path": "src"})).await;
    assert_eq!(matches, "src/a.md:1: needle\nsrc/a.rs:1: needle\n");
    let matches = ok(
        &tools,
        "grep",
        json!({"pattern": "needle", "path": "src/a.md"}),
    )
    .await;
    assert_eq!(matches, "src/a.md:1: needle\n");
}

#[tokio::test]
async fn grep_skips_ignored_binary_and_git_files() {
    let dir = tempdir().unwrap();
    write(dir.path(), ".gitignore", "ignored.txt\n");
    write(dir.path(), "ignored.txt", "needle\n");
    write(dir.path(), "kept.txt", "needle\n");
    write(dir.path(), ".git/config", "needle\n");
    std::fs::write(dir.path().join("blob.bin"), b"needle\0\x01").unwrap();
    let matches = ok(&planner(dir.path()), "grep", json!({"pattern": "needle"})).await;
    assert_eq!(matches, "kept.txt:1: needle\n");
}

#[tokio::test]
async fn grep_caps_matches_with_a_note() {
    let dir = tempdir().unwrap();
    let many_hits: String = (0..300)
        .map(|hit_number| format!("hit {hit_number}\n"))
        .collect();
    write(dir.path(), "many.txt", &many_hits);
    let matches = ok(&planner(dir.path()), "grep", json!({"pattern": "hit"})).await;
    assert_eq!(matches.lines().count(), 201);
    assert!(
        matches.ends_with("[100 more matches not shown. Narrow the search.]\n"),
        "{matches}"
    );
}

#[tokio::test]
async fn grep_reports_no_matches_and_bad_patterns() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.txt", "hay\n");
    let tools = planner(dir.path());
    let matches = ok(&tools, "grep", json!({"pattern": "needle"})).await;
    assert!(matches.contains("No matches"), "{matches}");
    err(&tools, "grep", json!({"pattern": "(unclosed"})).await;
}
