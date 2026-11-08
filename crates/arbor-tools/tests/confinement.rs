mod common;

use common::{err, planner, write};
use serde_json::json;
use tempfile::tempdir;

#[tokio::test]
async fn absolute_paths_are_rejected() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.txt", "secret");
    let absolute = dir.path().join("a.txt");
    let tools = planner(dir.path());
    err(&tools, "read_file", json!({"path": absolute})).await;
    err(&tools, "read_file", json!({"path": "/etc/passwd"})).await;
}

#[tokio::test]
async fn parent_escapes_are_rejected() {
    let outer = tempdir().unwrap();
    write(outer.path(), "secret.txt", "secret");
    write(outer.path(), "work/a.txt", "");
    let tools = planner(&outer.path().join("work"));
    let message = err(&tools, "read_file", json!({"path": "../secret.txt"})).await;
    assert!(!message.contains("secret\n"));
    err(&tools, "read_file", json!({"path": "sub/../../secret.txt"})).await;
    err(&tools, "list_dir", json!({"path": ".."})).await;
}

#[tokio::test]
async fn git_directory_is_off_limits() {
    let dir = tempdir().unwrap();
    write(dir.path(), ".git/config", "[core]\n");
    write(dir.path(), "src/a.txt", "");
    let tools = planner(dir.path());
    err(&tools, "read_file", json!({"path": ".git/config"})).await;
    err(&tools, "read_file", json!({"path": "./.GIT/config"})).await;
    err(&tools, "list_dir", json!({"path": ".git"})).await;
    let listing = common::ok(&tools, "list_dir", json!({})).await;
    assert!(!listing.contains(".git"), "{listing}");
}

#[cfg(unix)]
mod symlinks {
    use super::*;
    use std::os::unix::fs::symlink;

    #[tokio::test]
    async fn file_symlink_outside_is_rejected() {
        let outer = tempdir().unwrap();
        let work = tempdir().unwrap();
        write(outer.path(), "secret.txt", "secret");
        symlink(
            outer.path().join("secret.txt"),
            work.path().join("link.txt"),
        )
        .unwrap();
        let message = err(
            &planner(work.path()),
            "read_file",
            json!({"path": "link.txt"}),
        )
        .await;
        assert!(message.contains("outside"), "{message}");
    }

    #[tokio::test]
    async fn directory_symlink_outside_is_rejected() {
        let outer = tempdir().unwrap();
        let work = tempdir().unwrap();
        write(outer.path(), "secret.txt", "secret");
        symlink(outer.path(), work.path().join("linkdir")).unwrap();
        let tools = planner(work.path());
        err(&tools, "read_file", json!({"path": "linkdir/secret.txt"})).await;
        err(&tools, "list_dir", json!({"path": "linkdir"})).await;
    }

    #[tokio::test]
    async fn symlink_into_git_is_rejected() {
        let work = tempdir().unwrap();
        write(work.path(), ".git/config", "[core]\n");
        symlink(work.path().join(".git"), work.path().join("innocent")).unwrap();
        err(
            &planner(work.path()),
            "read_file",
            json!({"path": "innocent/config"}),
        )
        .await;
    }

    #[tokio::test]
    async fn symlink_inside_the_worktree_is_allowed() {
        let work = tempdir().unwrap();
        write(work.path(), "real.txt", "hello\n");
        symlink(work.path().join("real.txt"), work.path().join("alias.txt")).unwrap();
        let text = common::ok(
            &planner(work.path()),
            "read_file",
            json!({"path": "alias.txt"}),
        )
        .await;
        assert!(text.contains("hello"));
    }
}
