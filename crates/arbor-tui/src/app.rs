//! The live screen: the Session as the Autopilot runs it, the spinner, and the TUI's own state.

use std::time::Instant;

use arbor_session::{Activity, AutopilotUpdate};
use arbor_tiers::AttemptOutcome;
use ratatui::Frame;

use crate::ui::{RunningActivity, StatusInfo, UiState, draw};
use crate::view::{ReplyOutcome, SessionView, Tone, WriterReplyRecord};

pub struct App {
    pub view: SessionView,
    pub ui_state: UiState,
    pub status: StatusInfo,
    activity: Option<(Activity, Instant)>,
}

impl App {
    pub fn new(status: StatusInfo) -> Self {
        Self {
            view: SessionView::new(),
            ui_state: UiState::default(),
            status,
            activity: None,
        }
    }

    pub fn is_working(&self) -> bool {
        self.activity.is_some()
    }

    pub fn apply_update(&mut self, update: AutopilotUpdate) {
        match update {
            AutopilotUpdate::Event(event) => {
                self.activity = None;
                if let Err(err) = self.view.apply(&event) {
                    self.view.push_note(
                        format!("The view couldn't follow an event: {err}"),
                        Tone::Failure,
                    );
                }
            }
            AutopilotUpdate::Working(activity) => self.activity = Some((activity, Instant::now())),
            AutopilotUpdate::WriterAttempt { work_unit, attempt } => {
                let record = match attempt.outcome {
                    AttemptOutcome::Passed { reply, .. } => WriterReplyRecord {
                        reply,
                        outcome: ReplyOutcome::Passed,
                    },
                    AttemptOutcome::Failed(failure) => WriterReplyRecord {
                        outcome: ReplyOutcome::Failed(failure.reason.to_string()),
                        reply: failure.reply,
                    },
                    AttemptOutcome::Missing { items } => WriterReplyRecord {
                        reply: String::new(),
                        outcome: ReplyOutcome::Missing(items),
                    },
                };
                self.view.record_writer_reply(work_unit, record);
            }
            AutopilotUpdate::Committed { work_unit, diff } => self.view.set_diff(work_unit, diff),
        }
    }

    /// The run ended: nothing is running any more.
    pub fn stop_working(&mut self) {
        self.activity = None;
    }

    pub fn draw(&self, frame: &mut Frame) {
        let mut status = self.status.clone();
        status.activity = self
            .activity
            .as_ref()
            .map(|(activity, started)| RunningActivity {
                tier: activity.tier,
                model: activity.model.clone(),
                label: activity.label.clone(),
                elapsed: started.elapsed(),
            });
        draw(frame, &self.view, &self.ui_state, &status);
    }
}
