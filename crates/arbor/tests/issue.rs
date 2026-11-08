use std::process::Command;

fn arbor() -> Command {
    Command::new(env!("CARGO_BIN_EXE_arbor"))
}

#[test]
fn without_arguments_arbor_prints_its_version() {
    let output = arbor().output().unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("arbor {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn a_bad_issue_number_is_a_usage_error() {
    let output = arbor().args(["--issue", "abc"]).output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage: arbor"));
}

#[cfg(unix)]
mod with_a_fake_path {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use tempfile::{TempDir, tempdir};

    use super::*;

    fn path_with_gh(script: Option<&str>) -> TempDir {
        let dir = tempdir().unwrap();
        if let Some(body) = script {
            let gh = dir.path().join("gh");
            fs::write(&gh, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        }
        dir
    }

    #[test]
    fn the_issue_is_printed_as_the_request() {
        let path = path_with_gh(Some(
            r#"echo '{"title":"Fix it","body":"Now.","comments":[{"author":{"login":"ann"},"body":"+1"}]}'"#,
        ));

        let output = arbor()
            .args(["--issue", "7"])
            .env("PATH", path.path())
            .current_dir(path.path())
            .output()
            .unwrap();

        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "# Issue #7: Fix it\n\nNow.\n\n## Comments\n\n### @ann\n\n+1\n"
        );
    }

    #[test]
    fn a_missing_gh_says_to_install_it() {
        let path = path_with_gh(None);

        let output = arbor()
            .args(["--issue", "7"])
            .env("PATH", path.path())
            .output()
            .unwrap();

        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("`gh` is not installed"), "{stderr}");
    }

    #[test]
    fn a_logged_out_gh_says_to_log_in() {
        let path = path_with_gh(Some(
            "echo 'To get started with GitHub CLI, please run:  gh auth login' >&2\nexit 4",
        ));

        let output = arbor()
            .args(["--issue", "7"])
            .env("PATH", path.path())
            .output()
            .unwrap();

        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("run `gh auth login`"), "{stderr}");
    }

    #[test]
    fn a_repository_without_a_github_remote_points_to_handover_files() {
        let path = path_with_gh(Some("echo 'no git remotes found' >&2\nexit 1"));

        let output = arbor()
            .args(["--issue", "7"])
            .env("PATH", path.path())
            .output()
            .unwrap();

        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("no GitHub remote"), "{stderr}");
        assert!(stderr.contains("--handover-file <path>"), "{stderr}");
    }
}

#[test]
fn a_handover_file_is_printed_as_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("next.md");
    std::fs::write(&path, "# Add a flag\n\nNo GitHub needed.\n").unwrap();

    let output = arbor().arg("--handover-file").arg(&path).output().unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "# Add a flag\n\nNo GitHub needed.\n"
    );
}

#[test]
fn a_handover_can_come_from_stdin() {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = arbor()
        .args(["--handover-file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"Fix the parser.\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Fix the parser.\n"
    );
}

#[test]
fn a_missing_handover_file_is_an_error() {
    let output = arbor()
        .args(["--handover-file", "does-not-exist.md"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("could not read the handover file does-not-exist.md")
    );
}
