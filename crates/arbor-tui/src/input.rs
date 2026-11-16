//! What each key does to the [`UiState`].

use arbor_session::Decision;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::modal::ModalOutcome;
use crate::ui::{
    Confirmation, DetailTab, Focus, TreeNode, UiState, confirmation_key, selected_row, tree_nodes,
};
use crate::view::SessionView;

/// What a key asks of the program running the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    /// The key only changed what the screen shows.
    Handled,
    /// `q` or Ctrl+C: leave, after a [`Confirmation`] if a Session is running.
    Quit,
    /// The Human Operator pressed Enter in the prompt with this text.
    Submitted(String),
    /// Esc outside the prompt: stop what is running, after a [`Confirmation`].
    Cancel,
    /// The Human Operator pressed the confirming key.
    Confirmed(Confirmation),
    /// The Human Operator decided at the open Checkpoint.
    Decided(Decision),
    /// The Human Operator answered a Tier's questions.
    Answered(String),
    /// `m` outside the prompt: switch the Clarification Mode between Ask and Assume.
    SwitchClarificationMode,
}

const LOG_PAGE: usize = 10;

impl UiState {
    pub fn handle_key(&mut self, key: KeyEvent, view: &SessionView) -> KeyOutcome {
        if let Some(confirmation) = self.confirming.take() {
            return confirmed(confirmation, key);
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return KeyOutcome::Quit;
        }
        if self.shown_modal().is_some() {
            return self.handle_modal_key(key);
        }
        if self.modal.is_some() && self.focus != Focus::Prompt && key.code == KeyCode::Char('c') {
            self.modal_hidden = false;
            return KeyOutcome::Handled;
        }
        match key.code {
            KeyCode::Tab => {
                self.focus = next_focus(self.focus);
                return KeyOutcome::Handled;
            }
            KeyCode::BackTab => {
                self.focus = previous_focus(self.focus);
                return KeyOutcome::Handled;
            }
            _ => {}
        }
        if self.focus == Focus::Prompt {
            return self.handle_prompt_key(key);
        }
        match key.code {
            KeyCode::Char('q') => return KeyOutcome::Quit,
            KeyCode::Char('m') => return KeyOutcome::SwitchClarificationMode,
            KeyCode::Esc => return KeyOutcome::Cancel,
            KeyCode::Char('/' | 'i') => self.focus = Focus::Prompt,
            KeyCode::Left if self.focus == Focus::Tree => self.collapse_selected(),
            KeyCode::Right if self.focus == Focus::Tree => self.expand_selected(),
            KeyCode::Enter | KeyCode::Char(' ') if self.focus == Focus::Tree => {
                self.toggle_selected();
            }
            KeyCode::Left => self.switch_tab(previous_tab(self.detail_tab)),
            KeyCode::Right => self.switch_tab(next_tab(self.detail_tab)),
            KeyCode::Up | KeyCode::Char('k') => self.move_up(view, 1),
            KeyCode::Down | KeyCode::Char('j') => self.move_down(view, 1),
            KeyCode::PageUp => self.move_up(view, LOG_PAGE),
            KeyCode::PageDown => self.move_down(view, LOG_PAGE),
            KeyCode::Home | KeyCode::Char('g') => self.move_up(view, usize::MAX),
            KeyCode::End | KeyCode::Char('G') => self.move_down(view, usize::MAX),
            _ => {}
        }
        KeyOutcome::Handled
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => self.focus = Focus::Tree,
            KeyCode::Backspace => {
                self.prompt.pop();
            }
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.prompt.push('\n');
            }
            KeyCode::Enter if !self.prompt.trim().is_empty() => {
                return KeyOutcome::Submitted(std::mem::take(&mut self.prompt));
            }
            KeyCode::Char(typed) => self.prompt.push(typed),
            _ => {}
        }
        KeyOutcome::Handled
    }

    fn handle_modal_key(&mut self, key: KeyEvent) -> KeyOutcome {
        let Some(modal) = &mut self.modal else {
            return KeyOutcome::Handled;
        };
        match modal.handle_key(key) {
            None => KeyOutcome::Handled,
            Some(ModalOutcome::Hide) => {
                self.modal_hidden = true;
                KeyOutcome::Handled
            }
            Some(ModalOutcome::Decided(decision)) => {
                self.modal = None;
                KeyOutcome::Decided(decision)
            }
            Some(ModalOutcome::Answered(answers)) => {
                self.modal = None;
                KeyOutcome::Answered(answers)
            }
        }
    }

    /// Pasted text goes whole into the shown modal's text, or else the prompt, newlines included,
    /// so none of it is read as keys.
    pub fn paste(&mut self, pasted: &str) {
        self.confirming = None;
        if self.shown_modal().is_some()
            && let Some(modal) = &mut self.modal
        {
            modal.paste(pasted);
            return;
        }
        self.focus = Focus::Prompt;
        self.prompt
            .push_str(&pasted.replace("\r\n", "\n").replace('\r', "\n"));
    }

    fn select(&mut self, node: TreeNode) {
        if node != self.selected {
            self.selected = node;
            self.detail_scroll = 0;
        }
    }

    /// Selects the row `offset` rows away from the selected one, stopping at either end.
    fn select_by_offset(&mut self, view: &SessionView, offset: isize) {
        let nodes = tree_nodes(view, &self.collapsed);
        let Some(last) = nodes.len().checked_sub(1) else {
            return;
        };
        let row = selected_row(&nodes, self.selected)
            .saturating_add_signed(offset)
            .min(last);
        self.select(nodes[row]);
    }

    fn switch_tab(&mut self, tab: DetailTab) {
        self.detail_tab = tab;
        self.detail_scroll = 0;
    }

    /// On a Work Unit, collapses its Task and selects the Task.
    fn collapse_selected(&mut self) {
        let task = match self.selected {
            TreeNode::Task(task) | TreeNode::WorkUnit { task, .. } => task,
            TreeNode::Plan => return,
        };
        self.collapsed.insert(task);
        self.select(TreeNode::Task(task));
    }

    fn expand_selected(&mut self) {
        if let TreeNode::Task(task) = self.selected {
            self.collapsed.remove(&task);
        }
    }

    fn toggle_selected(&mut self) {
        if let TreeNode::Task(task) = self.selected
            && !self.collapsed.remove(&task)
        {
            self.collapsed.insert(task);
        }
    }

    fn move_up(&mut self, view: &SessionView, rows: usize) {
        match self.focus {
            Focus::Tree => {
                self.select_by_offset(view, -isize::try_from(rows).unwrap_or(isize::MAX))
            }
            Focus::Detail => {
                let rows = u16::try_from(rows).unwrap_or(u16::MAX);
                self.detail_scroll = self.detail_scroll.saturating_sub(rows);
            }
            Focus::Log => {
                let top = self.log_top.unwrap_or_else(|| self.newest_log_top(view));
                self.log_top = Some(top.saturating_sub(rows));
            }
            Focus::Prompt => {}
        }
    }

    fn move_down(&mut self, view: &SessionView, rows: usize) {
        match self.focus {
            Focus::Tree => self.select_by_offset(view, isize::try_from(rows).unwrap_or(isize::MAX)),
            Focus::Detail => {
                let rows = u16::try_from(rows).unwrap_or(u16::MAX);
                self.detail_scroll = self
                    .detail_scroll
                    .saturating_add(rows)
                    .min(self.detail_scroll_limit.get());
            }
            Focus::Log => {
                let newest = self.newest_log_top(view);
                self.log_top = self
                    .log_top
                    .map(|top| top.saturating_add(rows))
                    .filter(|&top| top < newest);
            }
            Focus::Prompt => {}
        }
    }

    /// The first entry shown when the log follows new events.
    fn newest_log_top(&self, view: &SessionView) -> usize {
        view.log().len().saturating_sub(self.log_rows.get().max(1))
    }
}

fn confirmed(confirmation: Confirmation, key: KeyEvent) -> KeyOutcome {
    let (confirming_char, _) = confirmation_key(confirmation);
    let is_confirming_key =
        key.code == KeyCode::Char(confirming_char) && key.modifiers.contains(KeyModifiers::CONTROL);
    if is_confirming_key {
        KeyOutcome::Confirmed(confirmation)
    } else {
        KeyOutcome::Handled
    }
}

fn next_focus(focus: Focus) -> Focus {
    match focus {
        Focus::Tree => Focus::Detail,
        Focus::Detail => Focus::Log,
        Focus::Log => Focus::Prompt,
        Focus::Prompt => Focus::Tree,
    }
}

fn previous_focus(focus: Focus) -> Focus {
    match focus {
        Focus::Tree => Focus::Prompt,
        Focus::Detail => Focus::Tree,
        Focus::Log => Focus::Detail,
        Focus::Prompt => Focus::Log,
    }
}

fn next_tab(tab: DetailTab) -> DetailTab {
    match tab {
        DetailTab::Spec => DetailTab::Transcript,
        DetailTab::Transcript => DetailTab::Diff,
        DetailTab::Diff => DetailTab::Spec,
    }
}

fn previous_tab(tab: DetailTab) -> DetailTab {
    match tab {
        DetailTab::Spec => DetailTab::Diff,
        DetailTab::Transcript => DetailTab::Spec,
        DetailTab::Diff => DetailTab::Transcript,
    }
}
