mod cli;
mod handover;
mod init;
mod live;
mod plan_editor;

use std::io::IsTerminal;
use std::process::ExitCode;

use arbor_core::Request;
use arbor_git::{GhCli, IssueError, issue_request};
use cli::Command;

fn main() -> ExitCode {
    let command = match cli::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(err) => {
            eprintln!("arbor: {err}");
            return ExitCode::from(2);
        }
    };
    match command {
        Command::Version => {
            println!("arbor {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Tui if !std::io::stdout().is_terminal() => {
            eprintln!(
                "arbor: the TUI needs a terminal; use --version, init, --issue or --handover-file"
            );
            ExitCode::from(2)
        }
        Command::Tui => live::run(None),
        Command::Init => init::run(),
        Command::Issue(number) => start_from_issue(number),
        Command::HandoverFile(path) => match handover::read(&path) {
            Ok(request) => start(request),
            Err(err) => {
                eprintln!("arbor: {err}");
                ExitCode::FAILURE
            }
        },
    }
}

/// Runs the Session in the TUI. Without a terminal, e.g. when piped, prints the Request instead.
fn start(request: Request) -> ExitCode {
    if !std::io::stdout().is_terminal() {
        print!("{}", request.text);
        return ExitCode::SUCCESS;
    }
    live::run(Some(request))
}

fn start_from_issue(number: u64) -> ExitCode {
    let dir = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("arbor: could not read the current directory: {err}");
            return ExitCode::FAILURE;
        }
    };
    match issue_request(&GhCli::default(), &dir, number) {
        Ok(request) => start(request),
        Err(err) => {
            eprintln!("arbor: {err}");
            if matches!(
                err,
                IssueError::NoGitHubRemote { .. }
                    | IssueError::GhNotInstalled
                    | IssueError::NotAuthenticated { .. }
            ) {
                eprintln!(
                    "arbor: to start without GitHub, write the Request to a file and use --handover-file <path>"
                );
            }
            ExitCode::FAILURE
        }
    }
}
