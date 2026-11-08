//! Hand-rolled because Arbor takes two options; switch to `clap` once it has subcommands.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Version,
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
            "{}\nusage: arbor [--issue <number> | --handover-file <path>]",
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
    let mut args = args.into_iter().map(Into::into);
    let Some(first) = args.next() else {
        return Ok(Command::Version);
    };
    let (option, inline_value) = match first.split_once('=') {
        Some((option, value)) => (option.to_owned(), Some(value.to_owned())),
        None => (first.clone(), None),
    };
    if option != "--issue" && option != "--handover-file" {
        return Err(UsageError(format!("unknown argument `{first}`")));
    }
    let value = match inline_value {
        Some(value) => value,
        None => args
            .next()
            .ok_or_else(|| UsageError(format!("{option} needs a value")))?,
    };
    if let Some(extra) = args.next() {
        return Err(UsageError(format!("unexpected argument `{extra}`")));
    }
    if option == "--handover-file" {
        return match value.is_empty() {
            true => Err(UsageError("--handover-file needs a path".to_owned())),
            false => Ok(Command::HandoverFile(value)),
        };
    }
    match value.parse::<u64>() {
        Ok(number) if number > 0 => Ok(Command::Issue(number)),
        _ => Err(UsageError(format!(
            "`{value}` is not an issue number; give a positive whole number such as 21"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_prints_the_version() {
        assert_eq!(parse(Vec::<String>::new()), Ok(Command::Version));
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
        ] {
            let err = parse(args.clone()).unwrap_err();
            assert!(err.to_string().contains("usage: arbor"), "{args:?}");
        }
    }
}
