//! The three Tiers: the agentic Planner and Scaffolder, and the non-agentic Writer.

mod agent;
mod check_proposal;
pub mod markers;
mod planner;
pub mod reply;
mod scaffold_check;
mod scaffolder;
mod writer;

pub use agent::{
    Agent, AgentSettings, MAX_SUBMIT_FAILURES, Outcome, SUBMIT_NOW_PERCENT, Submission, Turn,
    TurnError, UnsupportedModel, Validate, core_usage, require_tool_calling,
};
pub use check_proposal::{
    CheckProposalRun, ProposedChecks, SUBMIT_CHECKS, validate_proposed_checks,
};
pub use planner::{PlannerRun, Rejection, SUBMIT_PLAN, validate_plan};
pub use scaffold_check::{
    NO_SCAFFOLD_CHECK, ScaffoldCheck, WRITER_CAP_MARGIN_PERCENT, field_problems, marker_problems,
    writer_cap_report, writer_input_tokens,
};
pub use scaffolder::{SUBMIT_WORK_UNITS, ScaffolderRun, WorkUnits};
pub use writer::{
    Attempt, AttemptError, AttemptOutcome, Failure, FailureReason, MAX_WRITER_ATTEMPTS,
    RETRY_OUTPUT_CHARS, WRITER_SEED, Writer, WriterError, WriterRun, commit_subject,
};
