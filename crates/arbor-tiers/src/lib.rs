//! The three Tiers: the agentic Planner and Scaffolder, and the non-agentic Writer.
//!
//! [`PlannerRun`] starts the Planner, which turns a Request into a Plan via `submit_plan`.
//! [`ScaffolderRun`] starts the Scaffolder, which turns a Task into a Skeleton, tests and Work
//! Units via `submit_work_units`, checked by [`ScaffoldCheck`]. [`markers`] finds the Regions.
//! [`WriterRun`] starts a [`Writer`], which fills one Region per Work Unit with plain model calls
//! whose replies [`reply`] parses.
//!
//! An [`Agent`] runs one model call per [`Agent::turn`], runs the tools it asks for, and ends when
//! the model calls its submit tool with arguments that parse and validate, or escalates after
//! [`MAX_SUBMIT_FAILURES`] failed submissions.

mod agent;
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
pub use planner::{PlannerRun, Rejection, SUBMIT_PLAN, validate_plan};
pub use scaffold_check::{
    NO_SCAFFOLD_CHECK, ScaffoldCheck, WRITER_CAP_MARGIN_PERCENT, field_problems, marker_problems,
    writer_cap_report, writer_input_tokens,
};
pub use scaffolder::{SUBMIT_WORK_UNITS, ScaffolderRun, WorkUnits};
pub use writer::{
    Attempt, AttemptOutcome, Failure, FailureReason, MAX_WRITER_ATTEMPTS, RETRY_OUTPUT_CHARS,
    WRITER_SEED, Writer, WriterError, WriterRun, commit_subject,
};
