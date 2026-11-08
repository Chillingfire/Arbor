mod cli;

use std::process::ExitCode;

use arbor_git::{GhCli, issue_request};
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
        Command::Issue(number) => start_from_issue(number),
    }
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
        Ok(request) => {
            // TODO: start a Session with this Request once Sessions are wired up.
            print!("{}", request.text);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("arbor: {err}");
            ExitCode::FAILURE
        }
    }
}
