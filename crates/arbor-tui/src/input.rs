//! What each key does to the [`UiState`].

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ui::{DetailTab, Focus, TreeNode, UiState, tree_nodes};
use crate::view::SessionView;

/// What a key asks of the program running the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    /// The key only changed what the screen shows.
    Handled,
    Quit,
    /// The Human Operator pressed Enter in the prompt with this text.
    Submitted(String),
}

const LOG_PAGE: usize = 10;

impl UiState {
    pub fn handle_key(&mut self, key: KeyEvent, view: &SessionView) -> KeyOutcome {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return KeyOutcome::Quit;
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
            KeyCode::Char('/' | 'i') => self.focus = Focus::Prompt,
            KeyCode::Left if self.focus == Focus::Tree => self.collapse_selected(view),
            KeyCode::Right if self.focus == Focus::Tree => self.expand_selected(view),
            KeyCode::Enter | KeyCode::Char(' ') if self.focus == Focus::Tree => {
                self.toggle_selected(view);
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
            KeyCode::Enter if !self.prompt.trim().is_empty() => {
                return KeyOutcome::Submitted(std::mem::take(&mut self.prompt));
            }
            KeyCode::Char(typed) => self.prompt.push(typed),
            _ => {}
        }
        KeyOutcome::Handled
    }

    fn select(&mut self, row: usize) {
        if row != self.selected {
            self.selected = row;
            self.detail_scroll = 0;
        }
    }

    fn switch_tab(&mut self, tab: DetailTab) {
        self.detail_tab = tab;
        self.detail_scroll = 0;
    }

    /// On a Work Unit, collapses its Task and selects the Task.
    fn collapse_selected(&mut self, view: &SessionView) {
        let task = match tree_nodes(view, &self.collapsed).get(self.selected) {
            Some(TreeNode::Task(task) | TreeNode::WorkUnit { task, .. }) => *task,
            _ => return,
        };
        self.collapsed.insert(task);
        self.select_task(view, task);
    }

    fn expand_selected(&mut self, view: &SessionView) {
        if let Some(TreeNode::Task(task)) = tree_nodes(view, &self.collapsed).get(self.selected) {
            self.collapsed.remove(task);
        }
    }

    fn toggle_selected(&mut self, view: &SessionView) {
        if let Some(TreeNode::Task(task)) = tree_nodes(view, &self.collapsed).get(self.selected)
            && !self.collapsed.remove(task)
        {
            self.collapsed.insert(*task);
        }
    }

    fn select_task(&mut self, view: &SessionView, task: usize) {
        let row = tree_nodes(view, &self.collapsed)
            .iter()
            .position(|node| *node == TreeNode::Task(task));
        if let Some(row) = row {
            self.select(row);
        }
    }

    fn move_up(&mut self, view: &SessionView, rows: usize) {
        match self.focus {
            Focus::Tree => self.select(self.selected.saturating_sub(rows)),
            Focus::Detail => {
                let rows = u16::try_from(rows).unwrap_or(u16::MAX);
                self.detail_scroll = self.detail_scroll.saturating_sub(rows);
            }
            Focus::Log => {
                let oldest = view.log().len().saturating_sub(1);
                self.log_scrolled_back = self.log_scrolled_back.saturating_add(rows).min(oldest);
            }
            Focus::Prompt => {}
        }
    }

    fn move_down(&mut self, view: &SessionView, rows: usize) {
        match self.focus {
            Focus::Tree => {
                let last = tree_nodes(view, &self.collapsed).len().saturating_sub(1);
                self.select(self.selected.saturating_add(rows).min(last));
            }
            Focus::Detail => {
                let rows = u16::try_from(rows).unwrap_or(u16::MAX);
                self.detail_scroll = self
                    .detail_scroll
                    .saturating_add(rows)
                    .min(self.detail_scroll_limit.get());
            }
            Focus::Log => self.log_scrolled_back = self.log_scrolled_back.saturating_sub(rows),
            Focus::Prompt => {}
        }
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
