//! Terminal UI: the Plan tree, detail pane, event log, prompt and status bar.

mod input;
mod ui;
mod view;

pub use input::KeyOutcome;
pub use ui::{DetailTab, Focus, StatusInfo, TreeNode, UiState, draw, tree_nodes};
pub use view::{
    LogEntry, SessionView, Status, TaskView, Tone, WorkUnitView, describe, event_tier, event_tone,
    phase_task, tokens,
};
