//! Hand-rolled because Arbor takes two subcommands and a few options; switch to `clap` if more
//! arrive.

use std::fmt;

use arbor_git::TreeSource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Opens the TUI to type a Request.
    Tui,
    Version,
    /// Writes the project's Checks to `arbor.toml`.
    Init,
    Issue(u64),
    /// `-` reads the handover from stdin.
    HandoverFile(String),
    CheckMarkers(CheckMarkers),
}

/// `arbor check-markers`: lists the Markers in a tree, for git hooks, CI and manual use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckMarkers {
    pub source: TreeSource,
    /// The pre-commit hook's exception: a Session worktree's commits hold Markers by design.
    pub allow_session_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError(String);

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}\nusage: arbor [init | --version | --issue <number> | --handover-file <path>]\n       \
             arbor check-markers [--staged | --rev <rev>] [--allow-session-worktree]",
            self.0
        )
    }
}

/// Parses the arguments after the program name.
pub fn parse<I, S>(args: I) -> Result<Command, UsageError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut remaining_args = args.into_iter().map(Into::into);
    let Some(first) = remaining_args.next() else {
        return Ok(Command::Tui);
    };
    if first == "--version" || first == "-V" {
        return without_more_arguments(remaining_args, Command::Version);
    }
    if first == "init" {
        return without_more_arguments(remaining_args, Command::Init);
    }
    if first == "check-markers" {
        return parse_check_markers(remaining_args).map(Command::CheckMarkers);
    }
    let (option, inline_value) = match first.split_once('=') {
        Some((option, inline)) => (option.to_owned(), Some(inline.to_owned())),
        None => (first.clone(), None),
    };
    if option != "--issue" && option != "--handover-file" {
        return Err(UsageError(format!("unknown argument `{first}`")));
    }
    let option_value = match inline_value {
        Some(inline) => inline,
        None => remaining_args
            .next()
            .ok_or_else(|| UsageError(format!("{option} needs a value")))?,
    };
    if let Some(extra) = remaining_args.next() {
        return Err(UsageError(format!("unexpected argument `{extra}`")));
    }
    if option == "--handover-file" {
        return if option_value.is_empty() {
            Err(UsageError("--handover-file needs a path".to_owned()))
        } else {
            Ok(Command::HandoverFile(option_value))
        };
    }
    match option_value.parse::<u64>() {
        Ok(number) if number > 0 => Ok(Command::Issue(number)),
        _ => Err(UsageError(format!(
            "`{option_value}` is not an issue number; give a positive whole number such as 21"
        ))),
    }
}

fn parse_check_markers(
    mut remaining_args: impl Iterator<Item = String>,
) -> Result<CheckMarkers, UsageError> {
    let mut check = CheckMarkers {
        source: TreeSource::WorkingTree,
        allow_session_worktree: false,
    };
    let mut source_chosen = false;
    while let Some(arg) = remaining_args.next() {
        let source = match arg.as_str() {
            "--allow-session-worktree" => {
                check.allow_session_worktree = true;
                continue;
            }
            "--staged" => TreeSource::Staged,
            "--rev" => TreeSource::Revision(
                remaining_args
                    .next()
                    .ok_or_else(|| UsageError("--rev needs a revision".to_owned()))?,
            ),
            _ => match arg.strip_prefix("--rev=") {
                Some(revision) => TreeSource::Revision(revision.to_owned()),
                None => return Err(UsageError(format!("unexpected argument `{arg}`"))),
            },
        };
        if source_chosen {
            return Err(UsageError(
                "give at most one of --staged and --rev".to_owned(),
            ));
        }
        if source == TreeSource::Revision(String::new()) {
            return Err(UsageError("--rev needs a revision".to_owned()));
        }
        check.source = source;
        source_chosen = true;
    }
    Ok(check)
}

/// The command, unless an argument follows it.
fn without_more_arguments(
    mut remaining_args: impl Iterator<Item = String>,
    command: Command,
) -> Result<Command, UsageError> {
    match remaining_args.next() {
        Some(extra) => Err(UsageError(format!("unexpected argument `{extra}`"))),
        None => Ok(command),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_open_the_tui() {
        assert_eq!(parse(Vec::<String>::new()), Ok(Command::Tui));
    }

    #[test]
    fn the_version_flag_prints_the_version() {
        assert_eq!(parse(["--version"]), Ok(Command::Version));
        assert_eq!(parse(["-V"]), Ok(Command::Version));
    }

    #[test]
    fn an_issue_number_is_accepted_in_either_form() {
        assert_eq!(parse(["--issue", "21"]), Ok(Command::Issue(21)));
        assert_eq!(parse(["--issue=21"]), Ok(Command::Issue(21)));
    }

    #[test]
    fn a_handover_file_is_accepted_in_either_form() {
        let path = || Ok(Command::HandoverFile("notes/next.md".into()));
        assert_eq!(parse(["--handover-file", "notes/next.md"]), path());
        assert_eq!(parse(["--handover-file=notes/next.md"]), path());
        assert_eq!(
            parse(["--handover-file", "-"]),
            Ok(Command::HandoverFile("-".into()))
        );
    }

    #[test]
    fn init_takes_no_arguments() {
        assert_eq!(parse(["init"]), Ok(Command::Init));
    }

    #[test]
    fn check_markers_reads_the_working_tree_unless_told_otherwise() {
        let check = |source, allow_session_worktree| {
            Ok(Command::CheckMarkers(CheckMarkers {
                source,
                allow_session_worktree,
            }))
        };
        assert_eq!(
            parse(["check-markers"]),
            check(TreeSource::WorkingTree, false)
        );
        assert_eq!(
            parse(["check-markers", "--staged", "--allow-session-worktree"]),
            check(TreeSource::Staged, true)
        );
        let at_head = || check(TreeSource::Revision("HEAD".into()), false);
        assert_eq!(parse(["check-markers", "--rev", "HEAD"]), at_head());
        assert_eq!(parse(["check-markers", "--rev=HEAD"]), at_head());
    }

    #[test]
    fn bad_arguments_are_usage_errors() {
        for args in [
            vec!["--issue"],
            vec!["--issue", "abc"],
            vec!["--issue", "0"],
            vec!["--issue", "-3"],
            vec!["--issue=", ""],
            vec!["--issues", "21"],
            vec!["--issue", "21", "extra"],
            vec!["21"],
            vec!["--handover-file"],
            vec!["--handover-file="],
            vec!["--handover-file", "a.md", "b.md"],
            vec!["--handover", "a.md"],
            vec!["init", "now"],
            vec!["Init"],
            vec!["check-markers", "--rev"],
            vec!["check-markers", "--rev="],
            vec!["check-markers", "--staged", "--rev", "HEAD"],
            vec!["check-markers", "HEAD"],
            vec!["check-markers", "--no-verify"],
        ] {
            let err = parse(args.clone()).unwrap_err();
            assert!(err.to_string().contains("usage: arbor"), "{args:?}");
        }
    }
}
