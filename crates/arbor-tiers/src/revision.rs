//! A Scaffolder Revision: one agentic run that diagnoses failed work and rewrites the Work Units
//! it affects (grilling log, Round 2 decision 4).

use std::path::Path;
use std::sync::Arc;

use arbor_config::{Checks, ClarificationMode};
use arbor_core::{Revision, Tier, WorkUnitId};
use arbor_llm::{ContentPart, Message, ModelInfo};
use arbor_prompts::Prompt;
use arbor_tools::{Tools, Toolset};

use crate::agent::{Agent, AgentSettings, Submission, UnsupportedModel};
use crate::scaffold_check::ScaffoldCheck;

pub const SUBMIT_REVISION: &str = "submit_revision";

pub struct RevisionRun<'a> {
    /// The Scaffolder's prompt for the Task, rendered with its `revision` inputs and the same
    /// `clarification_mode` and `cap_tokens`.
    pub prompt: &'a Prompt,
    pub model: ModelInfo,
    pub temperature: f64,
    pub clarification_mode: ClarificationMode,
    pub worktree: &'a Path,
    pub checks: Checks,
    pub cap_tokens: u32,
    /// The Task's Work Units, whose IDs a revised Work Unit keeps.
    pub revisable: Vec<WorkUnitId>,
    /// `Session::next_work_unit_id`: new Work Units, e.g. from a split, are numbered from here.
    pub first_new_id: WorkUnitId,
    /// `Config::revision_tool_call_budget`.
    pub tool_call_budget: u32,
}

impl RevisionRun<'_> {
    /// # Panics
    ///
    /// If the prompt isn't the Scaffolder's.
    pub fn start(self) -> Result<Agent<Revision>, UnsupportedModel> {
        assert_eq!(self.prompt.template, arbor_prompts::Template::Scaffolder);
        let mut messages = self.prompt.messages();
        let Some(Message::User { content }) = messages.last_mut() else {
            unreachable!("a prompt ends with its inputs");
        };
        content.push(ContentPart::text(format!(
            "A revised Work Unit keeps its ID. Number any new Work Unit from {} upwards.",
            self.first_new_id
        )));
        let settings = AgentSettings {
            tier: Tier::Scaffolder,
            model: self.model,
            temperature: self.temperature,
            clarification_mode: self.clarification_mode,
        };
        let tools = Tools::new(self.worktree, Toolset::Scaffolder, self.checks.clone());
        let check = ScaffoldCheck::new(
            self.worktree,
            self.checks,
            self.cap_tokens,
            self.first_new_id,
        )
        .revising(self.revisable);
        let budget = usize::try_from(self.tool_call_budget).unwrap_or(usize::MAX);
        Agent::new(settings, submission(Arc::new(check)), tools, messages)
            .map(|agent| agent.with_tool_call_budget(budget))
    }
}

fn submission(check: Arc<ScaffoldCheck>) -> Submission<Revision> {
    Submission::with_async(
        SUBMIT_REVISION,
        "Submit the Revision: your diagnosis of why the work failed, and the Work Units to write \
         again with their Regions marked again. Arbor checks them as it checks \
         `submit_work_units`; any problem comes back to you to fix before submitting again.",
        move |revision: &Revision| {
            let check = Arc::clone(&check);
            Box::pin(async move {
                if revision.diagnosis.trim().is_empty() {
                    return Err("the Revision has no diagnosis; say why the work failed".into());
                }
                check.run(&revision.work_units).await
            })
        },
    )
}
