use std::path::{Path, PathBuf};

use arbor_config::{Checks, ClarificationMode, FILTER_PLACEHOLDER};
use arbor_core::Tier;
use arbor_llm::ModelInfo;
use arbor_prompts::{Prompt, Template};
use arbor_tools::{Tools, Toolset, validate_check_command};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agent::{Agent, AgentSettings, Submission, UnsupportedModel, reject_if_any};

pub const SUBMIT_CHECKS: &str = "submit_checks";

/// The Checks the Scaffolder proposes for `arbor.toml`, for the Human Operator to accept or skip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProposedChecks {
    /// Builds or typechecks the project, including its tests, without running them.
    pub scaffold: Option<String>,
    /// Builds and runs all the tests.
    pub unit: Option<String>,
    /// Runs only the tests matching `{filter}`, which must appear in the command.
    pub unit_filtered: Option<String>,
    /// Where each command came from, and any assumptions, for the Human Operator.
    pub explanation: String,
}

impl ProposedChecks {
    pub fn checks(&self) -> Checks {
        Checks {
            scaffold: self.scaffold.clone(),
            unit: self.unit.clone(),
            unit_filtered: self.unit_filtered.clone(),
        }
    }
}

/// A read-only Scaffolder run that proposes Checks when automatic init found none that work.
pub struct CheckProposalRun<'a> {
    pub prompt: &'a Prompt,
    pub model: ModelInfo,
    pub temperature: f64,
    pub worktree: &'a Path,
}

impl CheckProposalRun<'_> {
    /// # Panics
    ///
    /// If the prompt isn't the check proposal's.
    pub fn start(self) -> Result<Agent<ProposedChecks>, UnsupportedModel> {
        assert_eq!(self.prompt.template, Template::CheckProposal);
        // The Human Operator reviews the whole proposal once, so the run never stops to ask.
        let settings = AgentSettings {
            tier: Tier::Scaffolder,
            model: self.model,
            temperature: self.temperature,
            clarification_mode: ClarificationMode::Assume,
        };
        let tools = Tools::new(self.worktree, Toolset::Planner, Checks::default());
        let worktree = self.worktree.to_owned();
        Agent::new(
            settings,
            submission(worktree),
            tools,
            self.prompt.messages(),
        )
    }
}

fn submission(worktree: PathBuf) -> Submission<ProposedChecks> {
    Submission::new(
        SUBMIT_CHECKS,
        "Submit the project's Check commands for arbor.toml with where each came from. Each \
         command is run without a shell. Leave out any command you have no good evidence for.",
        move |proposed: &ProposedChecks| validate_proposed_checks(&worktree, proposed),
    )
}

/// Checks each proposed command parses and its program is found, without running anything: the
/// Human Operator hasn't accepted them yet (ADR-0008).
pub fn validate_proposed_checks(worktree: &Path, proposed: &ProposedChecks) -> Result<(), String> {
    let mut problems = Vec::new();
    if proposed.scaffold.is_none() && proposed.unit.is_none() {
        problems.push("propose at least a scaffold or a unit command".to_owned());
    }
    let commands = [
        ("scaffold", &proposed.scaffold),
        ("unit", &proposed.unit),
        ("unit_filtered", &proposed.unit_filtered),
    ];
    for (name, command) in commands {
        let Some(command) = command else { continue };
        if let Err(problem) = validate_check_command(worktree, command) {
            problems.push(format!("{name}: {problem}"));
        }
    }
    if let Some(template) = &proposed.unit_filtered
        && !template.contains(FILTER_PLACEHOLDER)
    {
        problems.push(format!("unit_filtered must contain {FILTER_PLACEHOLDER}"));
    }
    reject_if_any("the proposed Checks are invalid", problems)
}
