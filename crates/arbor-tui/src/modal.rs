//! The modals where the Session waits on the Human Operator: a Checkpoint to approve or reject,
//! and a Tier's clarifying questions to answer (grilling log Round 1 decision 7).

use arbor_core::{Tier, assumptions_in, is_assumption};
use arbor_session::{Checkpoint, Decision};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};

use crate::ui::{ACCENT, BAR, FAINT, MUTED, YELLOW};

/// What a key in a modal asks for, beyond changing the modal itself.
pub(crate) enum ModalOutcome {
    /// Esc: hide the modal to look around the Session; `c` shows it again.
    Hide,
    Decided(Decision),
    Answered(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    Checkpoint(CheckpointModal),
    Questions(QuestionsModal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointModal {
    pub checkpoint: Checkpoint,
    /// At a Plan Checkpoint, the Plan's Tasks in the order they would run, each marked kept or
    /// dropped. Empty at other Checkpoints.
    pub task_rows: Vec<TaskRow>,
    pub selected_row: usize,
    /// The rejection feedback being typed, or `None` while choosing.
    pub feedback: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskRow {
    /// The Task's index in the submitted Plan.
    pub task: usize,
    pub kept: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionsModal {
    pub tier: Tier,
    pub questions: String,
    pub answers: String,
}

impl Modal {
    pub fn checkpoint(checkpoint: Checkpoint) -> Self {
        let task_rows = match &checkpoint {
            Checkpoint::Plan { plan } => (0..plan.tasks.len())
                .map(|task| TaskRow { task, kept: true })
                .collect(),
            _ => Vec::new(),
        };
        Self::Checkpoint(CheckpointModal {
            checkpoint,
            task_rows,
            selected_row: 0,
            feedback: None,
        })
    }

    pub fn questions(tier: Tier, questions: String) -> Self {
        Self::Questions(QuestionsModal {
            tier,
            questions,
            answers: String::new(),
        })
    }

    /// `None` when the key only changed the modal. Esc hides the modal, which `c` brings back.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Option<ModalOutcome> {
        match self {
            Self::Checkpoint(checkpoint_modal) => checkpoint_modal.handle_key(key),
            Self::Questions(questions_modal) => questions_modal.handle_key(key),
        }
    }

    pub(crate) fn paste(&mut self, pasted: &str) {
        let typed = match self {
            Self::Checkpoint(CheckpointModal {
                feedback: Some(feedback),
                ..
            }) => feedback,
            Self::Questions(questions_modal) => &mut questions_modal.answers,
            Self::Checkpoint(_) => return,
        };
        typed.push_str(&pasted.replace("\r\n", "\n").replace('\r', "\n"));
    }
}

impl CheckpointModal {
    fn handle_key(&mut self, key: KeyEvent) -> Option<ModalOutcome> {
        if let Some(feedback) = &mut self.feedback {
            return match edit_text(feedback, key) {
                TextEdit::Send => Some(ModalOutcome::Decided(Decision::Reject(
                    self.feedback.take().unwrap_or_default(),
                ))),
                TextEdit::Leave => {
                    self.feedback = None;
                    None
                }
                TextEdit::Typed => None,
            };
        }
        let moving = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc => return Some(ModalOutcome::Hide),
            KeyCode::Char('a') | KeyCode::Enter => return self.approval(),
            KeyCode::Char('r') => self.feedback = Some(String::new()),
            KeyCode::Char('K') => self.move_selected_task(-1),
            KeyCode::Char('J') => self.move_selected_task(1),
            KeyCode::Up if moving => self.move_selected_task(-1),
            KeyCode::Down if moving => self.move_selected_task(1),
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected_row = self.selected_row.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected_row = (self.selected_row + 1).min(self.task_rows.len().max(1) - 1);
            }
            KeyCode::Char(' ') => {
                if let Some(row) = self.task_rows.get_mut(self.selected_row) {
                    row.kept = !row.kept;
                }
            }
            _ => {}
        }
        None
    }

    /// Approving a Plan with Tasks dropped or moved sends the Task order. With every Task dropped
    /// there is nothing to approve.
    fn approval(&self) -> Option<ModalOutcome> {
        if self.task_rows.is_empty() {
            return Some(ModalOutcome::Decided(Decision::Approve));
        }
        let task_order = self.task_order();
        if task_order.is_empty() {
            return None;
        }
        let unchanged = task_order.iter().copied().eq(0..self.task_rows.len());
        Some(ModalOutcome::Decided(if unchanged {
            Decision::Approve
        } else {
            Decision::ApproveTasks(task_order)
        }))
    }

    pub fn task_order(&self) -> Vec<usize> {
        self.task_rows
            .iter()
            .filter(|row| row.kept)
            .map(|row| row.task)
            .collect()
    }

    fn move_selected_task(&mut self, step: isize) {
        let Some(target) = self.selected_row.checked_add_signed(step) else {
            return;
        };
        if target < self.task_rows.len() {
            self.task_rows.swap(self.selected_row, target);
            self.selected_row = target;
        }
    }
}

impl QuestionsModal {
    fn handle_key(&mut self, key: KeyEvent) -> Option<ModalOutcome> {
        match edit_text(&mut self.answers, key) {
            TextEdit::Send => Some(ModalOutcome::Answered(std::mem::take(&mut self.answers))),
            TextEdit::Leave => Some(ModalOutcome::Hide),
            TextEdit::Typed => None,
        }
    }
}

enum TextEdit {
    Send,
    Leave,
    Typed,
}

/// Enter sends non-empty text; Alt+Enter or Shift+Enter starts a new line; Esc leaves.
fn edit_text(typed: &mut String, key: KeyEvent) -> TextEdit {
    match key.code {
        KeyCode::Esc => return TextEdit::Leave,
        KeyCode::Enter
            if key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
        {
            typed.push('\n');
        }
        KeyCode::Enter if !typed.trim().is_empty() => return TextEdit::Send,
        KeyCode::Backspace => {
            typed.pop();
        }
        KeyCode::Char(typed_char) => typed.push(typed_char),
        _ => {}
    }
    TextEdit::Typed
}

pub(crate) fn draw_modal(frame: &mut Frame, modal: &Modal) {
    let (title, lines, hint_pairs) = match modal {
        Modal::Checkpoint(checkpoint_modal) => (
            checkpoint_title(&checkpoint_modal.checkpoint),
            checkpoint_lines(checkpoint_modal),
            checkpoint_hints(checkpoint_modal),
        ),
        Modal::Questions(questions_modal) => (
            format!("The {:?} asks", questions_modal.tier),
            question_lines(questions_modal),
            TYPING_HINTS,
        ),
    };
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(100);
    let wanted_height = u16::try_from(lines.len() + 4).unwrap_or(u16::MAX);
    let height = wanted_height.min(area.height.saturating_sub(2)).max(3);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    let mut hint_spans = vec![Span::raw(" ")];
    for (keys, action) in hint_pairs {
        hint_spans.push(Span::styled(format!("{keys} "), Style::new().fg(ACCENT)));
        hint_spans.push(Span::styled(format!("{action}  "), Style::new().fg(MUTED)));
    }
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(YELLOW))
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(YELLOW).bold(),
        ))
        .title_bottom(Line::from(hint_spans).alignment(Alignment::Right))
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(BAR));
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        popup,
    );
}

const TYPING_HINTS: &[(&str, &str)] = &[
    ("Enter", "send"),
    ("Alt+Enter", "new line"),
    ("Esc", "back"),
];

fn checkpoint_title(checkpoint: &Checkpoint) -> String {
    match checkpoint {
        Checkpoint::Plan { .. } => "Plan Checkpoint".to_owned(),
        Checkpoint::Scaffold { task, .. } => format!("Scaffold Checkpoint · Task {}", task + 1),
        Checkpoint::Revision { task, .. } => format!("Revision Checkpoint · Task {}", task + 1),
    }
}

fn checkpoint_hints(checkpoint_modal: &CheckpointModal) -> &'static [(&'static str, &'static str)] {
    if checkpoint_modal.feedback.is_some() {
        TYPING_HINTS
    } else if checkpoint_modal.task_rows.is_empty() {
        &[("a", "approve"), ("r", "reject"), ("Esc", "look around")]
    } else {
        &[
            ("a", "approve"),
            ("r", "reject"),
            ("↑↓", "select"),
            ("Space", "keep/drop"),
            ("Shift+↑↓", "move"),
            ("Esc", "look around"),
        ]
    }
}

fn checkpoint_lines(checkpoint_modal: &CheckpointModal) -> Vec<Line<'static>> {
    let mut lines = match &checkpoint_modal.checkpoint {
        Checkpoint::Plan { plan } => {
            let mut lines = vec![labelled("Goal", &plan.goal)];
            for constraint in &plan.constraints {
                lines.push(constraint_line(constraint));
            }
            lines.push(Line::default());
            lines.push(Line::styled("Tasks", Style::new().fg(MUTED)));
            for (row_index, row) in checkpoint_modal.task_rows.iter().enumerate() {
                let title = &plan.tasks[row.task].title;
                lines.push(task_row_line(
                    row,
                    title,
                    row_index == checkpoint_modal.selected_row,
                ));
            }
            if checkpoint_modal.task_order().is_empty() {
                lines.push(Line::styled(
                    "Keep at least one Task to approve the Plan.",
                    Style::new().fg(YELLOW),
                ));
            }
            lines
        }
        Checkpoint::Scaffold { work_units, .. } => {
            let mut lines = vec![Line::styled(
                "The Skeleton and tests are in the worktree; Esc to look at each Work Unit first.",
                Style::new().fg(MUTED),
            )];
            for work_unit in work_units {
                lines.push(Line::default());
                lines.push(Line::from(vec![
                    Span::styled(format!("{} ", work_unit.id), Style::new().fg(ACCENT).bold()),
                    Span::styled(work_unit.file.clone(), Style::new().fg(MUTED)),
                ]));
                let spec_line = work_unit.spec.lines().next().unwrap_or_default();
                lines.push(Line::raw(spec_line.to_owned()));
                lines.extend(
                    assumptions_in(&work_unit.spec)
                        .into_iter()
                        .map(assumption_line),
                );
            }
            lines
        }
        Checkpoint::Revision { revision, .. } => {
            let mut lines = vec![labelled("Diagnosis", &revision.diagnosis)];
            for work_unit in &revision.work_units {
                lines.push(Line::from(vec![
                    Span::styled(format!("{} ", work_unit.id), Style::new().fg(ACCENT).bold()),
                    Span::raw(work_unit.spec.lines().next().unwrap_or_default().to_owned()),
                ]));
                lines.extend(
                    assumptions_in(&work_unit.spec)
                        .into_iter()
                        .map(assumption_line),
                );
            }
            lines
        }
    };
    if let Some(feedback) = &checkpoint_modal.feedback {
        lines.push(Line::default());
        lines.push(Line::styled(
            "Why reject it? The same Tier runs again with your feedback.",
            Style::new().fg(MUTED),
        ));
        lines.extend(typed_lines(feedback));
    }
    lines
}

fn question_lines(questions_modal: &QuestionsModal) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = questions_modal
        .questions
        .lines()
        .map(|question_line| Line::raw(question_line.to_owned()))
        .collect();
    lines.push(Line::default());
    lines.push(Line::styled("Your answers", Style::new().fg(MUTED)));
    lines.extend(typed_lines(&questions_modal.answers));
    lines
}

fn labelled(name: &str, labelled_text: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{name}  "), Style::new().fg(MUTED)),
        Span::raw(labelled_text.to_owned()),
    ])
}

fn constraint_line(constraint: &str) -> Line<'static> {
    if is_assumption(constraint) {
        assumption_line(constraint.trim())
    } else {
        Line::from(vec![
            Span::styled("• ", Style::new().fg(MUTED)),
            Span::raw(constraint.to_owned()),
        ])
    }
}

/// Assumption-mode guesses stand out, so the Human Operator checks them before approving.
fn assumption_line(assumption: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("▲ ", Style::new().fg(YELLOW)),
        Span::styled(assumption.to_owned(), Style::new().fg(YELLOW)),
    ])
}

fn task_row_line(row: &TaskRow, title: &str, selected: bool) -> Line<'static> {
    let pointer = if selected { "❯ " } else { "  " };
    let (check, title_style) = if row.kept {
        ("[x] ", Style::new())
    } else {
        (
            "[ ] ",
            Style::new().fg(FAINT).add_modifier(Modifier::CROSSED_OUT),
        )
    };
    let mut row_line = Line::from(vec![
        Span::styled(pointer, Style::new().fg(ACCENT)),
        Span::styled(check, Style::new().fg(MUTED)),
        Span::styled(format!("{}. {title}", row.task + 1), title_style),
    ]);
    if selected {
        row_line = row_line.bold();
    }
    row_line
}

/// What the Human Operator is typing, with a cursor after it.
fn typed_lines(typed: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = typed
        .split('\n')
        .map(|typed_line| {
            Line::from(vec![
                Span::styled("❯ ", Style::new().fg(ACCENT)),
                Span::raw(typed_line.to_owned()),
            ])
        })
        .collect();
    if let Some(last) = lines.last_mut() {
        last.push_span(Span::styled("▏", Style::new().fg(ACCENT)));
    }
    lines
}
