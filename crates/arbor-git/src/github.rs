//! Reading a GitHub issue as a Request through the `gh` CLI.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

use arbor_core::Request;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhOutput {
    /// `None` if `gh` was killed by a signal.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl GhOutput {
    fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// A seam so tests can fake `gh` without a network.
pub trait GhRunner {
    /// An [`io::ErrorKind::NotFound`] error means `gh` isn't installed.
    fn run(&self, dir: &Path, gh_args: &[&str]) -> io::Result<GhOutput>;
}

#[derive(Debug, Clone)]
pub struct GhCli {
    program: OsString,
}

impl GhCli {
    pub fn with_program(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
        }
    }
}

impl Default for GhCli {
    fn default() -> Self {
        Self::with_program("gh")
    }
}

impl GhRunner for GhCli {
    fn run(&self, dir: &Path, gh_args: &[&str]) -> io::Result<GhOutput> {
        let gh_output = Command::new(&self.program)
            .args(gh_args)
            .current_dir(dir)
            // Arbor runs under a TUI, so gh must never stop to ask the terminal anything.
            .env("GH_PROMPT_DISABLED", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?;
        Ok(GhOutput {
            code: gh_output.status.code(),
            stdout: String::from_utf8_lossy(&gh_output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&gh_output.stderr).trim().to_owned(),
        })
    }
}

#[derive(Debug)]
pub enum IssueError {
    GhNotInstalled,
    Spawn(io::Error),
    NoGitHubRemote { stderr: String },
    NotAuthenticated { stderr: String },
    NotFound { number: u64, stderr: String },
    Failed { number: u64, stderr: String },
    Malformed { number: u64, reason: String },
}

impl fmt::Display for IssueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GhNotInstalled => f.write_str(
                "the GitHub CLI `gh` is not installed or not on PATH; \
                 install it from https://cli.github.com and run `gh auth login`",
            ),
            Self::Spawn(err) => write!(f, "could not run the GitHub CLI `gh`: {err}"),
            Self::NoGitHubRemote { stderr } => write!(
                f,
                "this repository has no GitHub remote, so its issues can't be read{}",
                detail(stderr)
            ),
            Self::NotAuthenticated { stderr } => write!(
                f,
                "the GitHub CLI `gh` is not logged in; run `gh auth login` and try again{}",
                detail(stderr)
            ),
            Self::NotFound { number, stderr } => write!(
                f,
                "issue #{number} was not found in this repository's GitHub remote; \
                 check the number and that you have access to the repository{}",
                detail(stderr)
            ),
            Self::Failed { number, stderr } => write!(
                f,
                "`gh issue view {number}` failed; run it yourself to see why{}",
                detail(stderr)
            ),
            Self::Malformed { number, reason } => write!(
                f,
                "`gh issue view {number}` printed output Arbor could not read ({reason}); \
                 check that your `gh` is up to date"
            ),
        }
    }
}

fn detail(stderr: &str) -> String {
    if stderr.is_empty() {
        String::new()
    } else {
        format!(" (gh said: {stderr})")
    }
}

impl std::error::Error for IssueError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn(err) => Some(err),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
struct IssueJson {
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    comments: Vec<CommentJson>,
}

#[derive(Deserialize)]
struct CommentJson {
    #[serde(default)]
    author: Option<AuthorJson>,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Deserialize)]
struct AuthorJson {
    login: String,
}

/// Read-only, so it needs no approval (ADR-0006).
pub fn issue_request(
    runner: &impl GhRunner,
    dir: &Path,
    number: u64,
) -> Result<Request, IssueError> {
    let number_arg = number.to_string();
    let gh_args = [
        "issue",
        "view",
        &number_arg,
        "--json",
        "title,body,comments",
    ];
    let gh_output = runner.run(dir, &gh_args).map_err(spawn_error)?;
    if !gh_output.success() {
        return Err(classify_failure(runner, dir, number, gh_output));
    }
    let issue: IssueJson =
        serde_json::from_str(&gh_output.stdout).map_err(|err| IssueError::Malformed {
            number,
            reason: err.to_string(),
        })?;
    Ok(Request {
        text: render_markdown(number, &issue),
        issue: Some(number),
    })
}

fn spawn_error(err: io::Error) -> IssueError {
    if err.kind() == io::ErrorKind::NotFound {
        IssueError::GhNotInstalled
    } else {
        IssueError::Spawn(err)
    }
}

const GH_AUTH_EXIT_CODE: i32 = 4;

const NO_REMOTE_MESSAGES: &[&str] = &[
    "no git remotes found",
    "none of the git remotes configured for this repository point to a known GitHub host",
    "not a git repository",
];

fn classify_failure(
    runner: &impl GhRunner,
    dir: &Path,
    number: u64,
    gh_output: GhOutput,
) -> IssueError {
    let stderr = gh_output.stderr;
    // Checked first: gh's no-remote message also suggests `gh auth login`.
    if NO_REMOTE_MESSAGES
        .iter()
        .any(|message| stderr.contains(message))
    {
        return IssueError::NoGitHubRemote { stderr };
    }
    if gh_output.code == Some(GH_AUTH_EXIT_CODE) || stderr.contains("gh auth login") {
        return IssueError::NotAuthenticated { stderr };
    }
    if stderr.contains("Could not resolve to an issue")
        || stderr.contains("Could not resolve to an Issue")
    {
        return IssueError::NotFound { number, stderr };
    }
    // Older or newer gh versions word failures differently, so ask gh directly.
    match runner.run(dir, &["auth", "status"]) {
        Ok(status) if !status.success() => IssueError::NotAuthenticated { stderr },
        _ => IssueError::Failed { number, stderr },
    }
}

fn render_markdown(number: u64, issue: &IssueJson) -> String {
    let mut markdown = format!("# Issue #{number}: {}\n\n", issue.title.trim());
    markdown.push_str(non_empty(issue.body.as_deref()).unwrap_or("_No description provided._"));
    markdown.push('\n');
    if !issue.comments.is_empty() {
        markdown.push_str("\n## Comments\n");
        for comment in &issue.comments {
            let author = comment
                .author
                .as_ref()
                .map_or("unknown", |author| author.login.as_str());
            let body = non_empty(comment.body.as_deref()).unwrap_or("_Empty comment._");
            markdown.push_str(&format!("\n### @{author}\n\n{body}\n"));
        }
    }
    markdown
}

fn non_empty(issue_text: Option<&str>) -> Option<&str> {
    issue_text
        .map(str::trim)
        .filter(|trimmed| !trimmed.is_empty())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::path::PathBuf;

    use super::*;

    struct FakeGh {
        replies: RefCell<VecDeque<io::Result<GhOutput>>>,
        calls: RefCell<Vec<(PathBuf, Vec<String>)>>,
    }

    impl FakeGh {
        fn new(replies: Vec<io::Result<GhOutput>>) -> Self {
            Self {
                replies: RefCell::new(replies.into()),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.borrow().iter().map(|(_, a)| a.clone()).collect()
        }
    }

    impl GhRunner for FakeGh {
        fn run(&self, dir: &Path, gh_args: &[&str]) -> io::Result<GhOutput> {
            self.calls.borrow_mut().push((
                dir.to_owned(),
                gh_args.iter().map(|gh_arg| gh_arg.to_string()).collect(),
            ));
            self.replies
                .borrow_mut()
                .pop_front()
                .expect("an unexpected gh call")
        }
    }

    fn ok(stdout: &str) -> io::Result<GhOutput> {
        Ok(GhOutput {
            code: Some(0),
            stdout: stdout.to_owned(),
            stderr: String::new(),
        })
    }

    fn fail(code: i32, stderr: &str) -> io::Result<GhOutput> {
        Ok(GhOutput {
            code: Some(code),
            stdout: String::new(),
            stderr: stderr.to_owned(),
        })
    }

    fn fetch(gh: &FakeGh) -> Result<Request, IssueError> {
        issue_request(gh, Path::new("/repo"), 21)
    }

    #[test]
    fn an_issue_becomes_a_request_with_title_body_and_comments() {
        let gh = FakeGh::new(vec![ok(r#"{
            "title": "Start a Session from a GitHub issue",
            "body": "Pull the issue via gh.\n",
            "comments": [
                {"author": {"login": "alice"}, "body": "Mind Windows."},
                {"author": {"login": "bob"}, "body": "And auth errors."}
            ]
        }"#)]);

        let request = fetch(&gh).unwrap();

        assert_eq!(request.issue, Some(21));
        assert_eq!(
            request.text,
            "# Issue #21: Start a Session from a GitHub issue\n\n\
             Pull the issue via gh.\n\n\
             ## Comments\n\n\
             ### @alice\n\nMind Windows.\n\n\
             ### @bob\n\nAnd auth errors.\n"
        );
    }

    #[test]
    fn it_runs_a_read_only_issue_view_in_the_given_directory() {
        let gh = FakeGh::new(vec![ok(r#"{"title": "t", "body": "", "comments": []}"#)]);

        fetch(&gh).unwrap();

        let calls = gh.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, Path::new("/repo"));
        assert_eq!(
            calls[0].1,
            ["issue", "view", "21", "--json", "title,body,comments"]
        );
    }

    #[test]
    fn an_issue_without_body_or_comments_says_so() {
        let gh = FakeGh::new(vec![ok(
            r#"{"title": "Bare", "body": "  ", "comments": []}"#,
        )]);

        let request = fetch(&gh).unwrap();

        assert_eq!(
            request.text,
            "# Issue #21: Bare\n\n_No description provided._\n"
        );
    }

    #[test]
    fn a_comment_from_a_deleted_account_has_an_unknown_author() {
        let gh = FakeGh::new(vec![ok(
            r#"{"title": "t", "body": "b", "comments": [{"author": null, "body": "hi"}]}"#,
        )]);

        let request = fetch(&gh).unwrap();

        assert!(request.text.contains("### @unknown\n\nhi\n"));
    }

    #[test]
    fn a_missing_gh_says_to_install_it() {
        let gh = FakeGh::new(vec![Err(io::Error::from(io::ErrorKind::NotFound))]);

        let err = fetch(&gh).unwrap_err();

        assert!(matches!(err, IssueError::GhNotInstalled));
        assert!(err.to_string().contains("https://cli.github.com"));
    }

    #[test]
    fn an_unauthenticated_gh_says_to_log_in() {
        let gh = FakeGh::new(vec![fail(
            4,
            "To get started with GitHub CLI, please run:  gh auth login",
        )]);

        let err = fetch(&gh).unwrap_err();

        assert!(matches!(err, IssueError::NotAuthenticated { .. }));
        assert!(err.to_string().contains("run `gh auth login`"));
        assert_eq!(gh.calls().len(), 1);
    }

    #[test]
    fn an_unrecognised_failure_is_an_auth_error_when_auth_status_fails() {
        let gh = FakeGh::new(vec![
            fail(1, "HTTP 401: Bad credentials"),
            fail(1, "You are not logged into any GitHub hosts."),
        ]);

        let err = fetch(&gh).unwrap_err();

        assert!(matches!(err, IssueError::NotAuthenticated { .. }));
        assert_eq!(gh.calls()[1], ["auth", "status"]);
    }

    #[test]
    fn an_unknown_issue_is_not_found() {
        let gh = FakeGh::new(vec![fail(
            1,
            "GraphQL: Could not resolve to an issue or pull request with the number of 21. (repository.issue)",
        )]);

        let err = fetch(&gh).unwrap_err();

        assert!(matches!(err, IssueError::NotFound { number: 21, .. }));
        assert!(err.to_string().contains("issue #21 was not found"));
    }

    #[test]
    fn a_repository_without_a_github_remote_says_so() {
        for stderr in [
            "no git remotes found",
            "none of the git remotes configured for this repository point to a known GitHub \
             host. To tell gh about a new GitHub host, please use `gh auth login`",
            "failed to run git: fatal: not a git repository (or any of the parent directories)",
        ] {
            let gh = FakeGh::new(vec![fail(1, stderr)]);

            let err = fetch(&gh).unwrap_err();

            assert!(
                matches!(err, IssueError::NoGitHubRemote { .. }),
                "{stderr}: {err:?}"
            );
            assert!(err.to_string().contains("no GitHub remote"));
        }
    }

    #[test]
    fn other_failures_keep_what_gh_said() {
        let gh = FakeGh::new(vec![fail(1, "HTTP 502: Bad Gateway"), ok("")]);

        let err = fetch(&gh).unwrap_err();

        assert!(matches!(err, IssueError::Failed { number: 21, .. }));
        assert!(err.to_string().contains("gh said: HTTP 502: Bad Gateway"));
    }

    #[test]
    fn output_that_is_not_issue_json_is_malformed() {
        for stdout in ["not json", r#"{"body": "no title"}"#, ""] {
            let gh = FakeGh::new(vec![ok(stdout)]);

            let err = fetch(&gh).unwrap_err();

            assert!(
                matches!(err, IssueError::Malformed { number: 21, .. }),
                "{stdout:?} gave {err:?}"
            );
        }
    }
}
