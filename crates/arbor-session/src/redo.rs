//! Readying a resumed Session's worktree for the step a killed run left unfinished.

use std::fs;

use arbor_core::{Effect, Event, Phase, WorkUnitId};
use arbor_git::{CommitId, CommitMessage};
use arbor_tiers::markers::marker_lines_of;
use arbor_tiers::{commit_subject, revision_subject, skeleton_subject};

use crate::{Autopilot, AutopilotError, failed};

/// Whether resuming at `phase` resets the worktree to the last commit, which discards any
/// uncommitted changes in it.
pub fn resume_resets_worktree(phase: &Phase) -> bool {
    matches!(
        phase,
        Phase::Writing { .. } | Phase::Revising { .. } | Phase::Scaffolding { feedback: None, .. }
    )
}

/// A step that commits before the log records it.
enum CommittingStep {
    Skeleton,
    Revision,
    WorkUnit(WorkUnitId),
}

impl CommittingStep {
    fn logged_as(&self, commit: &CommitId) -> Event {
        let hash = commit.as_str().to_owned();
        match *self {
            Self::Skeleton => Event::SkeletonCommitted { commit: Some(hash) },
            Self::Revision => Event::RevisionCommitted { commit: Some(hash) },
            Self::WorkUnit(id) => Event::WorkUnitPassed { id },
        }
    }
}

impl Autopilot<'_> {
    /// Puts the worktree where the in-flight step expects it. A Writer, a first Scaffolder run, or
    /// a Revision starts from the last commit, so edits a killed run left are undone; a Scaffolder
    /// rerun with feedback revises the rejected Skeleton in place, so it is kept. A Work Unit that
    /// passed was committed by its Writer, so that commit is `HEAD`.
    pub(crate) fn prepare_to_redo(&mut self) -> Result<(), AutopilotError> {
        if let Some((step, head)) = self.unlogged_commit()? {
            self.apply(step.logged_as(&head))?;
            if let CommittingStep::WorkUnit(_) = step {
                self.passed_commit = Some(head);
            }
            return Ok(());
        }
        let phase = self.session.phase().clone();
        if resume_resets_worktree(&phase) {
            let head = self.worktree.head().map_err(failed("reading HEAD"))?;
            self.worktree
                .reset_to(&head)
                .map_err(failed("undoing the unfinished step's edits"))?;
        } else if let Phase::Committing { .. } = phase {
            let head = self.worktree.head().map_err(failed("reading HEAD"))?;
            self.passed_commit = Some(head);
        }
        Ok(())
    }

    /// A run killed after the Writer, the Skeleton or a Revision committed, but before the log
    /// said so, left `HEAD` on that commit with nothing uncommitted. Redoing the step on top of
    /// it would find no Region to fill, or nothing to commit and lose the hash.
    fn unlogged_commit(&self) -> Result<Option<(CommittingStep, CommitId)>, AutopilotError> {
        let (step, subject) = match *self.session.phase() {
            Phase::CommittingSkeleton { task } => {
                (CommittingStep::Skeleton, skeleton_subject(self.task(task)?))
            }
            Phase::CommittingRevision { task } => {
                (CommittingStep::Revision, revision_subject(self.task(task)?))
            }
            Phase::Writing { .. } => {
                let [Effect::RunWriter { id }] = self.session.pending()[..] else {
                    return Ok(None);
                };
                let work_unit = self.work_unit(id)?;
                if self.region_still_open(&work_unit.file, id) {
                    return Ok(None);
                }
                (
                    CommittingStep::WorkUnit(id),
                    commit_subject(&work_unit.spec, &work_unit.file),
                )
            }
            _ => return Ok(None),
        };
        let worktree = self.worktree;
        let head = worktree.head().map_err(failed("reading HEAD"))?;
        let expected =
            CommitMessage::new(&subject).map_err(failed("reading the commit subject"))?;
        let head_subject = worktree
            .head_subject()
            .map_err(failed("reading HEAD's subject"))?;
        let has_uncommitted_changes = worktree
            .has_uncommitted_changes()
            .map_err(failed("checking the worktree for uncommitted changes"))?;
        if has_uncommitted_changes
            || head_subject != expected.subject()
            || logs_commit(&self.events, head.as_str())
        {
            return Ok(None);
        }
        Ok(Some((step, head)))
    }

    /// An unreadable file counts as open, so the Writer is run again.
    fn region_still_open(&self, file: &str, id: WorkUnitId) -> bool {
        fs::read_to_string(self.worktree.path().join(file)).map_or(true, |file_text| {
            !marker_lines_of(&file_text, id).is_empty()
        })
    }
}

fn logs_commit(events: &[Event], hash: &str) -> bool {
    events.iter().any(|event| match event {
        Event::SkeletonCommitted {
            commit: Some(logged),
        }
        | Event::RevisionCommitted {
            commit: Some(logged),
        }
        | Event::OperatorFixed {
            commit: Some(logged),
        }
        | Event::WorkUnitCommitted { commit: logged, .. } => logged == hash,
        _ => false,
    })
}
