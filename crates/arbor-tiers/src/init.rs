//! Automatic `arbor.toml` init: detect the project type, validate its Checks by running them,
//! and fall back to a Scaffolder proposal the Human Operator accepts or skips.

use std::fmt;
use std::path::{Path, PathBuf};

use arbor_config::{
    Checks, ConfigError, Detection, PROJECT_CONFIG_FILE, ProjectType, detect_project_type_in,
    save_project_checks,
};
use arbor_core::Usage;
use arbor_llm::{BoxFuture, LlmClient, ModelInfo};
use arbor_prompts::{CheckProposalInputs, Conventions, PromptError, Prompts, TriedCheck};
use arbor_tools::{CheckName, run_check};

use crate::agent::{Outcome, TurnError, UnsupportedModel};
use crate::check_proposal::{CheckProposalRun, ProposedChecks};

/// One detected Check command run once in the worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRun {
    pub check: CheckName,
    pub command: String,
    pub passed: bool,
    /// Already trimmed by `run_check` to its first and last 100 lines, like the Writer's.
    pub output: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AutoDetection {
    /// The project type was detected and its commands passed.
    Validated(&'static ProjectType),
    /// Detection found no single project type, or its commands failed.
    Failed {
        detection: Detection,
        check_runs: Vec<CheckRun>,
    },
}

/// Detects the project type and runs its scaffold and unit commands once in `worktree`.
pub async fn detect_checks(worktree: &Path) -> AutoDetection {
    let detection = detect_project_type_in(worktree);
    let Detection::Found(project_type) = detection else {
        return AutoDetection::Failed {
            detection,
            check_runs: Vec::new(),
        };
    };
    let check_runs = validate_checks(worktree, &project_type.checks()).await;
    if check_runs.iter().all(|run| run.passed) {
        AutoDetection::Validated(project_type)
    } else {
        AutoDetection::Failed {
            detection,
            check_runs,
        }
    }
}

/// Runs the scaffold Check, then the unit Check, stopping at the first that fails. The
/// `unit_filtered` template isn't run: no filter is known to match a test in every project.
pub async fn validate_checks(worktree: &Path, checks: &Checks) -> Vec<CheckRun> {
    let mut check_runs = Vec::new();
    let configured = [
        (CheckName::Scaffold, &checks.scaffold),
        (CheckName::Unit, &checks.unit),
    ];
    for (check, command) in configured {
        let Some(command) = command else { continue };
        let (passed, output) = match run_check(worktree, checks, check, None).await {
            Ok(check_output) => (!check_output.is_error, check_output.text),
            Err(could_not_run) => (false, could_not_run),
        };
        check_runs.push(CheckRun {
            check,
            command: command.clone(),
            passed,
            output,
        });
        if !passed {
            break;
        }
    }
    check_runs
}

/// Proposes Checks when detection couldn't find working ones. The Scaffolder does this in
/// Arbor; tests and callers without a model can supply their own.
pub trait ProposeChecks: Send {
    fn propose<'a>(
        &'a mut self,
        detection: &'a Detection,
        failed_runs: &'a [CheckRun],
    ) -> BoxFuture<'a, Result<ProposedChecks, ProposalError>>;
}

/// The Human Operator's one confirmation of proposed Checks: a CLI prompt now, a TUI modal later.
pub trait ConfirmChecks: Send {
    fn confirm<'a>(&'a mut self, proposal: &'a ProposedChecks) -> BoxFuture<'a, Confirmation>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    Accept,
    /// Checks stay unconfigured, so Arbor skips them with its usual warning.
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitOutcome {
    Detected {
        project_type: &'static ProjectType,
        path: PathBuf,
    },
    Accepted {
        path: PathBuf,
    },
    Skipped,
}

impl InitOutcome {
    /// The one line to show the Human Operator.
    pub fn notice(&self) -> String {
        match self {
            Self::Detected { project_type, .. } => format!(
                "Wrote {PROJECT_CONFIG_FILE} with the Checks for a {} project.",
                project_type.name
            ),
            Self::Accepted { .. } => format!("Wrote the proposed Checks to {PROJECT_CONFIG_FILE}."),
            Self::Skipped => format!(
                "No Checks are configured in {PROJECT_CONFIG_FILE}, so Arbor will skip them and \
                 warn. Run `arbor init` to try again."
            ),
        }
    }
}

#[derive(Debug)]
pub enum ProposalError {
    /// The Scaffolder couldn't be set up, e.g. there is no API key.
    Unavailable(String),
    Prompt(PromptError),
    UnsupportedModel(UnsupportedModel),
    Turn(TurnError),
    /// The Scaffolder failed to submit valid Checks.
    Escalated {
        failures: Vec<String>,
    },
}

impl fmt::Display for ProposalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(reason) => f.write_str(reason),
            Self::Prompt(err) => err.fmt(f),
            Self::UnsupportedModel(err) => err.fmt(f),
            Self::Turn(err) => err.fmt(f),
            Self::Escalated { failures } => write!(
                f,
                "the Scaffolder couldn't propose valid Checks: {}",
                failures.join("; ")
            ),
        }
    }
}

impl std::error::Error for ProposalError {}

#[derive(Debug)]
pub enum InitError {
    Config(ConfigError),
    Proposal(ProposalError),
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(err) => err.fmt(f),
            Self::Proposal(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for InitError {}

/// Writes `arbor.toml` from the detected Checks when they pass, otherwise from a proposal the
/// Human Operator accepts. Re-running replaces only the `[checks]` table.
pub async fn init(
    worktree: &Path,
    proposer: &mut dyn ProposeChecks,
    operator: &mut dyn ConfirmChecks,
) -> Result<InitOutcome, InitError> {
    let (detection, check_runs) = match detect_checks(worktree).await {
        AutoDetection::Validated(project_type) => {
            let path =
                save_project_checks(worktree, &project_type.checks()).map_err(InitError::Config)?;
            return Ok(InitOutcome::Detected { project_type, path });
        }
        AutoDetection::Failed {
            detection,
            check_runs,
        } => (detection, check_runs),
    };
    let failed_runs: Vec<CheckRun> = check_runs.into_iter().filter(|run| !run.passed).collect();
    let proposal = proposer
        .propose(&detection, &failed_runs)
        .await
        .map_err(InitError::Proposal)?;
    match operator.confirm(&proposal).await {
        Confirmation::Accept => {
            let path =
                save_project_checks(worktree, &proposal.checks()).map_err(InitError::Config)?;
            Ok(InitOutcome::Accepted { path })
        }
        Confirmation::Skip => Ok(InitOutcome::Skipped),
    }
}

/// Proposes Checks with a read-only Scaffolder run.
pub struct ScaffolderProposer<'a> {
    pub client: &'a dyn LlmClient,
    pub prompts: &'a Prompts,
    pub conventions: &'a Conventions,
    pub model: ModelInfo,
    pub temperature: f64,
    pub worktree: &'a Path,
    /// What the run's model calls used, for the caller to record.
    pub usage: Usage,
}

impl ScaffolderProposer<'_> {
    async fn run(
        &mut self,
        detection: &Detection,
        failed_runs: &[CheckRun],
    ) -> Result<ProposedChecks, ProposalError> {
        let tried_checks: Vec<TriedCheck> = failed_runs.iter().map(tried_check).collect();
        let detection = detection.to_string();
        let inputs = CheckProposalInputs {
            detection: &detection,
            tried_checks: &tried_checks,
        };
        let prompt = self
            .prompts
            .check_proposal(self.conventions, &inputs)
            .map_err(ProposalError::Prompt)?;
        let run = CheckProposalRun {
            prompt: &prompt,
            model: self.model.clone(),
            temperature: self.temperature,
            worktree: self.worktree,
        };
        let mut agent = run.start().map_err(ProposalError::UnsupportedModel)?;
        loop {
            let turn = agent.turn(self.client).await.map_err(ProposalError::Turn)?;
            self.usage += turn.usage;
            match turn.outcome {
                None => {}
                Some(Outcome::Submitted(proposal)) => return Ok(proposal),
                Some(Outcome::Escalated { failures }) => {
                    return Err(ProposalError::Escalated { failures });
                }
                Some(Outcome::Questions(_)) => {
                    unreachable!("the check proposal runs in Assume mode, so it never asks")
                }
            }
        }
    }
}

impl ProposeChecks for ScaffolderProposer<'_> {
    fn propose<'a>(
        &'a mut self,
        detection: &'a Detection,
        failed_runs: &'a [CheckRun],
    ) -> BoxFuture<'a, Result<ProposedChecks, ProposalError>> {
        Box::pin(self.run(detection, failed_runs))
    }
}

fn tried_check(check_run: &CheckRun) -> TriedCheck {
    TriedCheck {
        check: check_run.check.name().to_owned(),
        command: check_run.command.clone(),
        output: check_run.output.clone(),
    }
}
