//! The three Tiers: the agentic Planner and Scaffolder, and the non-agentic Writer.

mod agent;
mod check_proposal;
mod checkpoint_commits;
mod init;
mod marker_allowlist;
mod marker_scan;
pub mod markers;
mod planner;
mod replan;
pub mod reply;
mod revision;
mod revision_review;
mod scaffold_check;
mod scaffolder;
mod task_completion;
mod unified_diff;
mod writer;
mod writer_cap;

pub use agent::{
    Agent, AgentSettings, MAX_SUBMIT_FAILURES, Outcome, SUBMIT_NOW_PERCENT, Submission, Turn,
    TurnError, UnsupportedModel, Validate, core_usage, require_tool_calling,
};
pub use check_proposal::{
    CheckProposalRun, ProposedChecks, SUBMIT_CHECKS, validate_proposed_checks,
};
pub use checkpoint_commits::{
    commit_revision, commit_skeleton, discard_revision, revision_subject, skeleton_subject,
};
pub use init::{
    AutoDetection, CheckRun, ConfirmChecks, Confirmation, InitError, InitOutcome, ProposalError,
    ProposeChecks, ScaffolderProposer, detect_checks, init, validate_checks,
};
pub use marker_allowlist::{AllowlistError, MARKER_ALLOWLIST_FILE, MarkerAllowlist};
pub use marker_scan::{MarkerLine, ScanError, scan_markers};
pub use planner::{PlannerRun, Rejection, SUBMIT_PLAN, validate_plan};
pub use replan::{ReplanRun, SUBMIT_REPLAN, validate_replan};
pub use revision::{RevisionRun, SUBMIT_REVISION};
pub use revision_review::{
    REWORDING_ONLY, changes_outside_regions, changes_work_units, review_revision,
};
pub use scaffold_check::{
    NO_SCAFFOLD_CHECK, ScaffoldCheck, comment_less_marker_problems, field_problems,
    marker_problems, revision_field_problems,
};
pub use scaffolder::{SUBMIT_WORK_UNITS, ScaffolderRun, WorkUnits};
pub use task_completion::{
    MARKER_FIX_SUBJECT, NO_UNIT_CHECK, TASK_FIX_SUBJECT, check_markers, check_operator_fix,
    commit_operator_fix, leftover_markers, reset_task, run_task_check, strip_work_unit_markers,
    task_check_warning, task_diff,
};
pub use writer::{
    Attempt, AttemptError, AttemptOutcome, Failure, FailureReason, MAX_WRITER_ATTEMPTS,
    RETRY_OUTPUT_CHARS, WRITER_SEED, Writer, WriterError, WriterRun, commit_subject,
};
pub use writer_cap::{WRITER_CAP_MARGIN_PERCENT, writer_cap_report, writer_input_tokens};
