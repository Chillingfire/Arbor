//! Terminal UI: the Plan tree, detail pane, event log, prompt and status bar.

mod app;
mod browser;
mod input;
mod leaving;
mod modal;
mod setup;
mod ui;
mod view;

pub use app::App;
pub use browser::{BrowserOutcome, SessionBrowser, duration_label};
pub use input::KeyOutcome;
pub use leaving::UnfinishedSession;
pub use modal::{CheckpointModal, Modal, QuestionsModal, TaskRow};
pub use setup::{
    ApiKeyDialog, KeyStorage, ManualRole, ManualRoles, ModelsDialog, SetupDialog, SetupOutcome,
    UncheckedKey, draw_setup,
};
pub use ui::{
    Confirmation, DetailTab, Focus, RunningActivity, StatusInfo, TreeNode, UiState, draw,
    tree_nodes,
};
pub use view::{
    LogEntry, ReplyOutcome, SessionView, Status, TaskView, Tone, WorkUnitView, WriterReplyRecord,
    describe, event_tier, event_tone, phase_task, tokens,
};
