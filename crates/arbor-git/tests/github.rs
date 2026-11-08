use arbor_git::{GhCli, IssueError, issue_request};
use tempfile::tempdir;

#[test]
fn a_gh_that_is_not_installed_is_reported_as_such() {
    let dir = tempdir().unwrap();
    let gh = GhCli::with_program(dir.path().join("no-such-gh"));

    let err = issue_request(&gh, dir.path(), 7).unwrap_err();

    assert!(matches!(err, IssueError::GhNotInstalled), "{err:?}");
}

#[cfg(unix)]
mod fake_gh {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use super::*;

    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("gh");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn the_issue_json_gh_prints_becomes_the_request() {
        let dir = tempdir().unwrap();
        let gh = script(
            dir.path(),
            r#"[ "$*" = "issue view 7 --json title,body,comments" ] || exit 9
[ "$GH_PROMPT_DISABLED" = 1 ] || exit 8
touch ran-here
echo '{"title":"Fix it","body":"Now.","comments":[{"author":{"login":"ann"},"body":"+1"}]}'"#,
        );

        let request = issue_request(&GhCli::with_program(gh), dir.path(), 7).unwrap();

        assert_eq!(request.issue, Some(7));
        assert_eq!(
            request.text,
            "# Issue #7: Fix it\n\nNow.\n\n## Comments\n\n### @ann\n\n+1\n"
        );
        assert!(dir.path().join("ran-here").exists());
    }

    #[test]
    fn a_logged_out_gh_is_reported_as_unauthenticated() {
        let dir = tempdir().unwrap();
        let gh = script(
            dir.path(),
            "echo 'To get started with GitHub CLI, please run:  gh auth login' >&2\nexit 4",
        );

        let err = issue_request(&GhCli::with_program(gh), dir.path(), 7).unwrap_err();

        assert!(
            matches!(err, IssueError::NotAuthenticated { .. }),
            "{err:?}"
        );
    }
}
