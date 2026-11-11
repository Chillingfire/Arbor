//! Agent tools for the Planner and Scaffolder, confined to the Session worktree.
//!
//! There is deliberately no shell tool (ADR-0008).

mod checks;
mod read;
mod search;
mod worktree;
mod write;

use std::path::{Path, PathBuf};

use arbor_config::Checks;
use arbor_llm::ToolDef;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;

pub use crate::checks::CheckName;
use crate::worktree::Worktree;

/// Tools only the Scaffolder may call.
const SCAFFOLDER_ONLY: &[&str] = &["write_file", "edit_file", "run_checks"];

/// Which Tier the tools serve. The Planner may only read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toolset {
    Planner,
    Scaffolder,
}

/// Runs one configured Check for Arbor itself rather than for a model. `Err` means the Check
/// couldn't run at all: it isn't configured, its command can't be parsed or started, or the filter
/// is invalid. A Check that runs and fails is `Ok` with `is_error` set.
pub async fn run_check(
    worktree: &Path,
    checks: &Checks,
    check: CheckName,
    filter: Option<&str>,
) -> Result<ToolOutput, String> {
    let worktree = Worktree::new(worktree.to_owned());
    checks::run_check(&worktree, checks, check, filter).await
}

/// Checks that a Check command parses into a program and arguments without a shell, and that the
/// program is found, without running it.
pub fn validate_check_command(worktree: &Path, command: &str) -> Result<(), String> {
    checks::validate_command(&Worktree::new(worktree.to_owned()), command)
}

/// A tool's result, sent back to the model as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

/// The tools for one Tier in one Session worktree.
pub struct Tools {
    worktree: Worktree,
    toolset: Toolset,
    checks: Checks,
}

impl Tools {
    /// `checks` are the commands loaded at Session start. Later edits to `arbor.toml` in the
    /// worktree don't change them.
    pub fn new(worktree: impl Into<PathBuf>, toolset: Toolset, checks: Checks) -> Self {
        Self {
            worktree: Worktree::new(worktree.into()),
            toolset,
            checks,
        }
    }

    pub fn toolset(&self) -> Toolset {
        self.toolset
    }

    /// The tools this Tier may call.
    pub fn definitions(&self) -> Vec<ToolDef> {
        let mut definitions = vec![
            definition::<read::ReadFileArgs>(
                "read_file",
                "Read a text file with numbered lines, at most 400 lines at a time. Use offset \
                 and limit to page through longer files.",
            ),
            definition::<read::ListDirArgs>(
                "list_dir",
                "List a directory's entries. Directories end with /. Files excluded by \
                 .gitignore are left out.",
            ),
            definition::<search::GlobArgs>(
                "glob",
                "Find files whose paths match a glob, skipping files excluded by .gitignore. \
                 Results are capped.",
            ),
            definition::<search::GrepArgs>(
                "grep",
                "Search file contents for a regular expression. Returns path:line: text for \
                 each matching line, skipping files excluded by .gitignore. Results are capped.",
            ),
        ];
        if self.toolset == Toolset::Scaffolder {
            definitions.extend([
                definition::<write::WriteFileArgs>(
                    "write_file",
                    "Create or overwrite a file in the worktree with the given content.",
                ),
                definition::<write::EditFileArgs>(
                    "edit_file",
                    "Replace one exact snippet in a file. The snippet must occur exactly once.",
                ),
                definition::<checks::RunChecksArgs>(
                    "run_checks",
                    "Run one of the project's configured checks in the worktree and return its \
                     exit code and output. scaffold verifies the build; unit runs all tests; \
                     unit_filtered runs the tests matching filter.",
                ),
            ]);
        }
        definitions
    }

    /// Runs a tool the model called. `arguments` is the JSON text the model wrote. Failures,
    /// including refusals, come back as error output for the model rather than as `Err`.
    pub async fn call(&self, name: &str, arguments: &str) -> ToolOutput {
        let worktree = &self.worktree;
        let tool_result = match name {
            "read_file" => parse(name, arguments).and_then(|args| read::read_file(worktree, args)),
            "list_dir" => parse(name, arguments).and_then(|args| read::list_dir(worktree, args)),
            "glob" => parse(name, arguments).and_then(|args| search::glob(worktree, args)),
            "grep" => parse(name, arguments).and_then(|args| search::grep(worktree, args)),
            _ if self.toolset == Toolset::Planner && SCAFFOLDER_ONLY.contains(&name) => Err(
                format!("{name} is not available to the Planner, which only reads"),
            ),
            "write_file" => {
                parse(name, arguments).and_then(|args| write::write_file(worktree, args))
            }
            "edit_file" => parse(name, arguments).and_then(|args| write::edit_file(worktree, args)),
            "run_checks" => {
                let check_result = match parse(name, arguments) {
                    Ok(args) => checks::run_checks(worktree, &self.checks, args).await,
                    Err(message) => Err(message),
                };
                return check_result.unwrap_or_else(ToolOutput::error);
            }
            _ => Err(format!("unknown tool {name}")),
        };
        tool_result.map_or_else(ToolOutput::error, ToolOutput::success)
    }
}

impl ToolOutput {
    fn success(output_text: String) -> Self {
        Self {
            text: output_text,
            is_error: false,
        }
    }

    fn error(output_text: String) -> Self {
        Self {
            text: output_text,
            is_error: true,
        }
    }
}

fn definition<T: JsonSchema>(name: &str, description: &str) -> ToolDef {
    let mut parameters =
        serde_json::to_value(schemars::schema_for!(T)).expect("a JSON Schema serializes to JSON");
    if let Some(object) = parameters.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
    }
    ToolDef {
        name: name.to_owned(),
        description: description.to_owned(),
        parameters,
    }
}

fn parse<T: DeserializeOwned>(name: &str, arguments: &str) -> Result<T, String> {
    let arguments = if arguments.trim().is_empty() {
        "{}"
    } else {
        arguments
    };
    serde_json::from_str(arguments).map_err(|err| format!("invalid arguments for {name}: {err}"))
}
