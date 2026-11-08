//! Per-Tier prompt templates and Conventions loading.
//!
//! [`Prompts`] renders a [`Prompt`] per call, whose parts come in caching order: the template's
//! instructions, then the [`Conventions`] (Planner and Scaffolder only), then the Session inputs.
//! The caller appends the transcript.

mod conventions;
mod prompt;
mod templates;

pub use conventions::{CONVENTIONS_CAP_TOKENS, Conventions, ConventionsError, estimate_tokens};
pub use prompt::Prompt;
pub use templates::{
    PROMPTS_DIR, PlannerInputs, PromptError, Prompts, RevisionInputs, ScaffolderInputs, Tier,
    WriterInputs,
};
