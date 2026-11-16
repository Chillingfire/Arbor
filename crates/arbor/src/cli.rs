//! Hand-rolled because Arbor takes one bare subcommand and two options; switch to `clap` once a
//! subcommand takes arguments.

use std::fmt;

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError(String);

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}\nusage: arbor [init | --version | --issue <number> | --handover-file <path>]",
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
        ] {
            let err = parse(args.clone()).unwrap_err();
            assert!(err.to_string().contains("usage: arbor"), "{args:?}");
        }
    }
}
