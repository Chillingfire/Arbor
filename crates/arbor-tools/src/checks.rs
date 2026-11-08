use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use arbor_config::{Checks, FILTER_PLACEHOLDER};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::process::Command;

use crate::ToolOutput;
use crate::read::cut;
use crate::worktree::Worktree;

/// A check still running after this long is killed.
const CHECK_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Output longer than this keeps its first and last `KEEP_LINES` lines.
const KEEP_LINES: usize = 100;
const MAX_OUTPUT_LINE_CHARS: usize = 500;

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum CheckName {
    /// Verifies that the Skeleton builds.
    Scaffold,
    /// Builds and runs all tests.
    Unit,
    /// Builds and runs only the tests that match `filter`.
    UnitFiltered,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct RunChecksArgs {
    /// Which configured check to run.
    check: CheckName,
    /// The test filter. Required for `unit_filtered`, ignored otherwise.
    filter: Option<String>,
}

pub(crate) async fn run_checks(
    worktree: &Worktree,
    checks: &Checks,
    args: RunChecksArgs,
) -> Result<ToolOutput, String> {
    let (name, template) = match args.check {
        CheckName::Scaffold => ("scaffold", &checks.scaffold),
        CheckName::Unit => ("unit", &checks.unit),
        CheckName::UnitFiltered => ("unit_filtered", &checks.unit_filtered),
    };
    let template = template
        .as_deref()
        .ok_or_else(|| format!("no {name} check is configured in arbor.toml"))?;
    let filter = match args.check {
        CheckName::UnitFiltered => Some(valid_filter(args.filter.as_deref())?),
        _ => None,
    };
    let argv = command_line(template, filter)?;
    let root = worktree.root()?;
    let program = resolve_program(&argv[0], &root)?;

    let child = Command::new(program)
        .args(&argv[1..])
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| format!("could not start {}: {err}", argv[0]))?;
    let shown = shell_words::join(&argv);
    // On timeout the child is dropped, which kills it.
    let Ok(output) = tokio::time::timeout(CHECK_TIMEOUT, child.wait_with_output()).await else {
        return Ok(ToolOutput {
            text: format!("$ {shown}\ntimed out after 10 minutes and was killed"),
            is_error: true,
        });
    };
    let output = output.map_err(|err| format!("{shown}: {err}"))?;

    let status = match output.status.code() {
        Some(code) => format!("exit code {code}"),
        None => "killed by a signal".to_owned(),
    };
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(ToolOutput {
        text: format!("$ {shown}\n{status}\n\n{}", trim_output(&combined)),
        is_error: !output.status.success(),
    })
}

/// A filter the model supplied. It can't start with `-`, so it can't pass the test runner an
/// option.
fn valid_filter(filter: Option<&str>) -> Result<&str, String> {
    match filter {
        None | Some("") => Err("unit_filtered needs a filter".to_owned()),
        Some(filter) if filter.trim_start().starts_with('-') => {
            Err(format!("a filter may not start with -: {filter:?}"))
        }
        Some(filter) => Ok(filter),
    }
}

/// Splits the configured command into arguments, then puts the filter into the arguments that
/// hold the placeholder. Splitting first means the filter is never parsed as shell syntax.
fn command_line(template: &str, filter: Option<&str>) -> Result<Vec<String>, String> {
    let words = shell_words::split(template)
        .map_err(|err| format!("the configured command {template:?} can't be parsed: {err}"))?;
    if words.is_empty() {
        return Err("the configured command is empty".to_owned());
    }
    Ok(words
        .into_iter()
        .map(|word| match filter {
            Some(filter) => word.replace(FILTER_PLACEHOLDER, filter),
            None => word,
        })
        .collect())
}

/// Finds the program on `PATH`, or relative to the worktree. On Windows this finds `npm.cmd`
/// for `npm`.
fn resolve_program(program: &str, root: &Path) -> Result<std::path::PathBuf, String> {
    which::which_in(program, std::env::var_os("PATH"), root)
        .map_err(|_| format!("{program}: program not found on PATH"))
}

/// Keeps the start and end of long output, where compilers and test runners put what matters.
fn trim_output(output: &str) -> String {
    let lines: Vec<String> = output
        .trim()
        .lines()
        .map(|line| cut(line, MAX_OUTPUT_LINE_CHARS))
        .collect();
    if lines.len() <= 2 * KEEP_LINES {
        return lines.join("\n");
    }
    let omitted = lines.len() - 2 * KEEP_LINES;
    format!(
        "{}\n[{omitted} lines omitted]\n{}",
        lines[..KEEP_LINES].join("\n"),
        lines[lines.len() - KEEP_LINES..].join("\n")
    )
}
