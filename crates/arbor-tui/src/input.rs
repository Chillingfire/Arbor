//! What each key does to the [`UiState`].

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ui::{DetailTab, Focus, UiState, tree_nodes};
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
            KeyCode::Left => self.detail_tab = previous_tab(self.detail_tab),
            KeyCode::Right => self.detail_tab = next_tab(self.detail_tab),
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

    fn move_up(&mut self, view: &SessionView, rows: usize) {
        match self.focus {
            Focus::Tree => self.selected = self.selected.saturating_sub(rows),
            Focus::Log => {
                let oldest = view.log().len().saturating_sub(1);
                self.log_scrolled_back = self.log_scrolled_back.saturating_add(rows).min(oldest);
            }
            Focus::Detail | Focus::Prompt => {}
        }
    }

    fn move_down(&mut self, view: &SessionView, rows: usize) {
        match self.focus {
            Focus::Tree => {
                let last = tree_nodes(view).len().saturating_sub(1);
                self.selected = self.selected.saturating_add(rows).min(last);
            }
            Focus::Log => self.log_scrolled_back = self.log_scrolled_back.saturating_sub(rows),
            Focus::Detail | Focus::Prompt => {}
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
