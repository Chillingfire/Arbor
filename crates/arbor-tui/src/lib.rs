//! Terminal UI: the Plan tree, detail pane, event log, prompt and status bar.

mod app;
mod escalation;
mod input;
mod modal;
mod ui;
mod view;

pub use app::App;
pub use escalation::HintDraft;
pub use input::KeyOutcome;
pub use modal::{CheckpointModal, Modal, QuestionsModal, TaskRow};
pub use ui::{
    Confirmation, DetailTab, Focus, RunningActivity, StatusInfo, TreeNode, UiState, draw,
    tree_nodes,
};
pub use view::{
    LogEntry, ReplyOutcome, SessionView, Status, TaskView, Tone, WorkUnitView, WriterReplyRecord,
    describe, event_tier, event_tone, phase_task, tokens,
};
