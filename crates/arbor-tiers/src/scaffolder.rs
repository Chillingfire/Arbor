use std::path::Path;
use std::sync::Arc;

use arbor_config::{Checks, ClarificationMode};
use arbor_core::{Tier, WorkUnit, WorkUnitId};
use arbor_llm::{ContentPart, Message, ModelInfo};
use arbor_prompts::Prompt;
use arbor_tools::{Tools, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agent::{Agent, AgentSettings, Submission, UnsupportedModel};
use crate::scaffold_check::ScaffoldCheck;

/// The tool the Scaffolder ends its run with.
pub const SUBMIT_WORK_UNITS: &str = "submit_work_units";

/// What the Scaffolder submits for one Task. A tool's parameters must be an object, so the list
/// is wrapped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkUnits {
    /// One per Region, in the order the Writers should fill them.
    pub work_units: Vec<WorkUnit>,
}

/// Everything a Scaffolder run for one Task starts from.
pub struct ScaffolderRun<'a> {
    /// The Scaffolder's prompt, rendered with the same `clarification_mode` and `cap_tokens`.
    pub prompt: &'a Prompt,
    /// The Scaffolder model's OpenRouter metadata.
    pub model: ModelInfo,
    pub temperature: f64,
    pub clarification_mode: ClarificationMode,
    /// The Session worktree, which the Scaffolder reads, writes and checks.
    pub worktree: &'a Path,
    /// The Checks loaded at Session start.
    pub checks: Checks,
    /// The Writer Cap in tokens.
    pub cap_tokens: u32,
    /// The lowest Work Unit ID this Task may use, so IDs stay unique across the Session.
    pub first_id: WorkUnitId,
    /// Set when the Human Operator rejected the previous Skeleton and Work Units at the scaffold
    /// Checkpoint: their feedback.
    pub feedback: Option<&'a str>,
}

impl ScaffolderRun<'_> {
    /// [`crate::NO_SCAFFOLD_CHECK`] when `checks.scaffold` isn't configured. The Session shows it
    /// once; each submission then skips the Scaffold Check but still checks the Markers.
    pub fn warning(&self) -> Option<&'static str> {
        self.checks
            .scaffold
            .is_none()
            .then_some(crate::NO_SCAFFOLD_CHECK)
    }

    /// Starts the run. Its [`Agent::turn`]s end with the submitted [`WorkUnits`], the model's
    /// questions, or an Escalation once [`crate::MAX_SUBMIT_FAILURES`] submissions have failed,
    /// which goes to the scaffold Checkpoint.
    ///
    /// A submission is accepted only when [`ScaffoldCheck::run`] passes: every Work Unit fits the
    /// Writer Cap, its file holds exactly one Marker pair for it, and the Scaffold Check passes.
    /// Otherwise the problems go back to the model as the tool result.
    ///
    /// # Panics
    ///
    /// If the prompt isn't the Scaffolder's.
    pub fn start(self) -> Result<Agent<WorkUnits>, UnsupportedModel> {
        assert_eq!(self.prompt.tier, arbor_prompts::Tier::Scaffolder);
        let mut messages = self.prompt.messages();
        let Some(Message::User { content }) = messages.last_mut() else {
            unreachable!("a prompt ends with its inputs");
        };
        content.push(ContentPart::text(format!(
            "Number this Task's Work Units from {} upwards; lower IDs belong to earlier Tasks.",
            self.first_id
        )));
        if let Some(feedback) = self.feedback {
            content.push(ContentPart::text(rejection_text(feedback)));
        }
        let settings = AgentSettings {
            tier: Tier::Scaffolder,
            model: self.model,
            temperature: self.temperature,
            clarification_mode: self.clarification_mode,
        };
        let tools = Tools::new(self.worktree, Toolset::Scaffolder, self.checks.clone());
        let check = ScaffoldCheck::new(self.worktree, self.checks, self.cap_tokens, self.first_id);
        Agent::new(settings, submission(Arc::new(check)), tools, messages)
    }
}

fn rejection_text(feedback: &str) -> String {
    format!(
        "# Rejected Skeleton and Work Units\n\nThe Human Operator rejected your Skeleton and Work \
         Units for this Task at the Checkpoint. The previous Skeleton and tests are still in the \
         worktree.\n\nTheir feedback:\n\n{feedback}\n\nRevise the Skeleton, tests and Work Units \
         to address the feedback, then call `{SUBMIT_WORK_UNITS}` again."
    )
}

fn submission(check: Arc<ScaffoldCheck>) -> Submission<WorkUnits> {
    Submission::with_async(
        SUBMIT_WORK_UNITS,
        "Submit the Task's Work Units once the Skeleton, its Markers and the tests are written. \
         Arbor checks each Work Unit's size against the Writer Cap and its Markers, then runs \
         the Scaffold Check; any problem comes back to you to fix before submitting again.",
        move |units: &WorkUnits| {
            let check = Arc::clone(&check);
            Box::pin(async move { check.run(&units.work_units).await })
        },
    )
}
