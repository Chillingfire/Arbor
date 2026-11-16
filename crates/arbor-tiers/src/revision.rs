//! A Scaffolder Revision: one agentic run that diagnoses failed work and rewrites the Work Units
//! it affects (grilling log, Round 2 decision 4).

use std::path::Path;
use std::sync::Arc;

use arbor_config::{Checks, ClarificationMode};
use arbor_core::{Revision, RevisionCause, Tier, WorkUnit, WorkUnitId};
use arbor_llm::ModelInfo;
use arbor_prompts::Prompt;
use arbor_tools::{Tools, Toolset};

use crate::agent::{Agent, AgentSettings, Submission, UnsupportedModel, append_input};
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
    pub cause: &'a RevisionCause,
    /// `Session::work_units`. After a failed Task Check any of them may be revised; after a
    /// failed Work Unit only that one, in its own file.
    pub task_work_units: &'a [WorkUnit],
    /// `Session::next_work_unit_id`: new Work Units, e.g. from a split, are numbered from here.
    pub first_new_id: WorkUnitId,
    /// `Config::revision_tool_call_budget`.
    pub tool_call_budget: u32,
}

/// Which Work Units a Revision may touch.
struct Scope {
    revisable: Vec<WorkUnitId>,
    /// The failed Work Unit's file, when one Work Unit failed.
    file: Option<String>,
    kept_regions: Vec<WorkUnitId>,
}

impl RevisionRun<'_> {
    /// # Panics
    ///
    /// If the prompt isn't the Scaffolder's.
    pub fn start(self) -> Result<Agent<Revision>, UnsupportedModel> {
        assert_eq!(self.prompt.template, arbor_prompts::Template::Scaffolder);
        let mut messages = self.prompt.messages();
        append_input(
            &mut messages,
            format!(
                "A revised Work Unit keeps its ID. Number any new Work Unit from {} upwards.",
                self.first_new_id
            ),
        );
        let scope = self.scope();
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
        .revising(scope.revisable)
        .keeping_regions(scope.kept_regions);
        let budget = usize::try_from(self.tool_call_budget).unwrap_or(usize::MAX);
        Agent::new(
            settings,
            submission(Arc::new(check), scope.file),
            tools,
            messages,
        )
        .map(|agent| agent.with_tool_call_budget(budget))
    }

    fn scope(&self) -> Scope {
        let task_ids = self.task_work_units.iter().map(|unit| unit.id);
        match self.cause.work_unit() {
            None => Scope {
                revisable: task_ids.collect(),
                file: None,
                kept_regions: Vec::new(),
            },
            Some(failed) => Scope {
                revisable: vec![failed],
                file: self
                    .task_work_units
                    .iter()
                    .find(|unit| unit.id == failed)
                    .map(|unit| unit.file.clone()),
                kept_regions: task_ids.filter(|&id| id != failed).collect(),
            },
        }
    }
}

fn submission(check: Arc<ScaffoldCheck>, scope_file: Option<String>) -> Submission<Revision> {
    Submission::with_async(
        SUBMIT_REVISION,
        "Submit the Revision: your diagnosis of why the work failed, and the Work Units to write \
         again with their Regions marked again. Arbor checks them as it checks \
         `submit_work_units`; any problem comes back to you to fix before submitting again. If \
         the fix needs a signature or type other Work Units depend on, set `shared_change` \
         instead and leave `work_units` empty.",
        move |revision: &Revision| {
            let check = Arc::clone(&check);
            let scope_file = scope_file.clone();
            Box::pin(
                async move { validate_revision(&check, scope_file.as_deref(), revision).await },
            )
        },
    )
}

async fn validate_revision(
    check: &ScaffoldCheck,
    scope_file: Option<&str>,
    revision: &Revision,
) -> Result<(), String> {
    if revision.diagnosis.trim().is_empty() {
        return Err("the Revision has no diagnosis; say why the work failed".into());
    }
    if let Some(shared_change) = &revision.shared_change {
        return check_shared_change(shared_change, revision);
    }
    if let Some(file) = scope_file {
        check_file_scope(file, revision)?;
    }
    check.run(&revision.work_units).await
}

fn check_shared_change(shared_change: &str, revision: &Revision) -> Result<(), String> {
    if shared_change.trim().is_empty() {
        return Err("`shared_change` is empty; name the signature or type to change".into());
    }
    if !revision.work_units.is_empty() {
        return Err(
            "a Revision with a `shared_change` goes to the Planner, so leave \
                    `work_units` empty"
                .into(),
        );
    }
    Ok(())
}

/// A Revision of one failed Work Unit stays in that Work Unit's file.
fn check_file_scope(file: &str, revision: &Revision) -> Result<(), String> {
    let outside: Vec<String> = revision
        .work_units
        .iter()
        .filter(|unit| unit.file != file)
        .map(|unit| format!("{} is in {}", unit.id, unit.file))
        .collect();
    if outside.is_empty() {
        return Ok(());
    }
    Err(format!(
        "a Revision of one failed Work Unit stays in its file, {file}, but {}. If the fix needs \
         changes elsewhere that other Work Units depend on, set `shared_change` instead.",
        outside.join(" and ")
    ))
}
