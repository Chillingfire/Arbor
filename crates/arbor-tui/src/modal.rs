//! The modals where the Session waits on the Human Operator: a Checkpoint to approve or reject,
//! an Escalation, leftover Markers or the Budget Cap to settle, and a Tier's clarifying questions
//! to answer (grilling log Round 1 decision 7, Round 3 decisions 2 and 9).

use arbor_core::{LeftoverMarker, Task, Tier, Usd, assumptions_in, is_assumption};
use arbor_session::{Checkpoint, Decision};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};
use std::cell::Cell;

use crate::escalation::{HintDraft, escalation_hints, escalation_lines};
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
    Checkpoint(Box<CheckpointModal>),
    Questions(QuestionsModal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointModal {
    pub checkpoint: Checkpoint,
    /// At a Plan or Replan Checkpoint, its Tasks in the order they would run, each marked kept or
    /// dropped. Empty at other Checkpoints.
    pub task_rows: Vec<TaskRow>,
    pub selected_row: usize,
    /// The rejection feedback being typed, or `None` while choosing.
    pub feedback: Option<String>,
    /// At the Budget Checkpoint, the new cap in dollars as typed. It starts at double the old cap.
    pub budget_cap_text: String,
    /// Whether the first digit or point replaces the suggested cap instead of adding to it.
    pub budget_cap_suggested: bool,
    /// At the Escalation Checkpoint, the hint being typed, or `None` while choosing.
    pub hint_draft: Option<HintDraft>,
    /// At the Escalation Checkpoint, a skip or abort waiting for the Human Operator to confirm it.
    pub confirming: Option<Decision>,
    pub scroll: ModalScroll,
}

/// How far a modal that doesn't fit the terminal is scrolled. By default it follows the selected
/// row or the typing cursor; PgUp and PgDn take over until the next other key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModalScroll {
    /// The first wrapped row shown, as of the last draw.
    top: Cell<usize>,
    following: bool,
}

impl Default for ModalScroll {
    fn default() -> Self {
        Self {
            top: Cell::new(0),
            following: true,
        }
    }
}

/// Wrapped rows that PgUp and PgDn move.
const SCROLL_PAGE: usize = 5;

impl ModalScroll {
    /// Whether the key was a scroll key. Any other key puts the view back on what it follows.
    fn handle_key(&mut self, key: KeyEvent) -> bool {
        let top = self.top.get();
        match key.code {
            KeyCode::PageUp => self.top.set(top.saturating_sub(SCROLL_PAGE)),
            KeyCode::PageDown => self.top.set(top + SCROLL_PAGE),
            _ => {
                self.following = true;
                return false;
            }
        }
        self.following = false;
        true
    }

    /// The first row to show of `total_rows`, with `focus` the rows from the first to the last
    /// that must stay in view while following.
    fn visible_top(
        &self,
        total_rows: usize,
        visible_rows: usize,
        focus: Option<(usize, usize)>,
    ) -> usize {
        let mut top = self.top.get();
        if let (true, Some((focus_start, focus_end))) = (self.following, focus) {
            if focus_start < top {
                top = focus_start;
            } else if focus_end > top + visible_rows {
                top = focus_end - visible_rows;
            }
        }
        let top = top.min(total_rows.saturating_sub(visible_rows));
        self.top.set(top);
        top
    }
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
    pub scroll: ModalScroll,
}

impl Modal {
    pub fn checkpoint(checkpoint: Checkpoint) -> Self {
        let task_count = proposed_tasks(&checkpoint).map_or(0, <[Task]>::len);
        let task_rows = (0..task_count)
            .map(|task| TaskRow { task, kept: true })
            .collect();
        let budget_cap_text = match &checkpoint {
            Checkpoint::Budget { cap, .. } => format!("{:.2}", cap.dollars() * 2.0),
            _ => String::new(),
        };
        Self::Checkpoint(Box::new(CheckpointModal {
            checkpoint,
            task_rows,
            selected_row: 0,
            feedback: None,
            budget_cap_text,
            budget_cap_suggested: true,
            hint_draft: None,
            confirming: None,
            scroll: ModalScroll::default(),
        }))
    }

    pub fn questions(tier: Tier, questions: String) -> Self {
        Self::Questions(QuestionsModal {
            tier,
            questions,
            answers: String::new(),
            scroll: ModalScroll::default(),
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
            Self::Checkpoint(checkpoint_modal) => {
                match (
                    &mut checkpoint_modal.feedback,
                    &mut checkpoint_modal.hint_draft,
                ) {
                    (Some(feedback), _) => feedback,
                    (None, Some(draft)) => &mut draft.hint,
                    (None, None) => return,
                }
            }
            Self::Questions(questions_modal) => &mut questions_modal.answers,
        };
        typed.push_str(&pasted.replace("\r\n", "\n").replace('\r', "\n"));
    }
}

/// The Tasks a Plan or Replan Checkpoint would run.
fn proposed_tasks(checkpoint: &Checkpoint) -> Option<&[Task]> {
    match checkpoint {
        Checkpoint::Plan { plan } => Some(&plan.tasks),
        Checkpoint::Replan { replan, .. } => Some(&replan.tasks),
        _ => None,
    }
}

impl CheckpointModal {
    fn handle_key(&mut self, key: KeyEvent) -> Option<ModalOutcome> {
        if self.scroll.handle_key(key) {
            return None;
        }
        match self.checkpoint {
            Checkpoint::Escalation { .. } => return self.escalation_key(key),
            Checkpoint::Markers { .. } => return markers_key(key),
            Checkpoint::Budget { spent, .. } => return self.budget_key(key, spent),
            _ => {}
        }
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

impl CheckpointModal {
    /// Digits and a point edit the new cap, the first replacing the suggested one; Enter sends it
    /// once it is above what was spent.
    fn budget_key(&mut self, key: KeyEvent, spent: Usd) -> Option<ModalOutcome> {
        let decision = match key.code {
            KeyCode::Esc => return Some(ModalOutcome::Hide),
            KeyCode::Char(typed_char) if typed_char.is_ascii_digit() || typed_char == '.' => {
                if std::mem::take(&mut self.budget_cap_suggested) {
                    self.budget_cap_text.clear();
                }
                self.budget_cap_text.push(typed_char);
                return None;
            }
            KeyCode::Backspace => {
                self.budget_cap_suggested = false;
                self.budget_cap_text.pop();
                return None;
            }
            KeyCode::Enter => {
                Decision::SetBudgetCap(Some(typed_budget_cap(&self.budget_cap_text, spent)?))
            }
            KeyCode::Char('u') => Decision::SetBudgetCap(None),
            KeyCode::Char('s') => Decision::Stop,
            _ => return None,
        };
        Some(ModalOutcome::Decided(decision))
    }
}

/// The typed cap, when it is an amount above what the Session has spent.
fn typed_budget_cap(budget_cap_text: &str, spent: Usd) -> Option<Usd> {
    let dollars: f64 = budget_cap_text.parse().ok()?;
    let cap = Usd::from_dollars(dollars);
    (dollars.is_finite() && cap > spent).then_some(cap)
}

/// Markers left in the branch are approved as they are, removed by hand, or the run stops.
fn markers_key(key: KeyEvent) -> Option<ModalOutcome> {
    let decision = match key.code {
        KeyCode::Esc => return Some(ModalOutcome::Hide),
        KeyCode::Char('a') => Decision::Approve,
        KeyCode::Char('f') => Decision::Fixed,
        KeyCode::Char('s') => Decision::Stop,
        _ => return None,
    };
    Some(ModalOutcome::Decided(decision))
}

impl QuestionsModal {
    fn handle_key(&mut self, key: KeyEvent) -> Option<ModalOutcome> {
        if self.scroll.handle_key(key) {
            return None;
        }
        match edit_text(&mut self.answers, key) {
            TextEdit::Send => Some(ModalOutcome::Answered(std::mem::take(&mut self.answers))),
            TextEdit::Leave => Some(ModalOutcome::Hide),
            TextEdit::Typed => None,
        }
    }
}

pub(crate) enum TextEdit {
    Send,
    Leave,
    Typed,
}

/// Enter sends non-empty text; Alt+Enter or Shift+Enter starts a new line; Esc leaves.
pub(crate) fn edit_text(typed: &mut String, key: KeyEvent) -> TextEdit {
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
    let (title, content, hint_pairs, scroll) = match modal {
        Modal::Checkpoint(checkpoint_modal) => (
            checkpoint_title(&checkpoint_modal.checkpoint),
            checkpoint_content(checkpoint_modal),
            checkpoint_hints(checkpoint_modal),
            &checkpoint_modal.scroll,
        ),
        Modal::Questions(questions_modal) => (
            format!("The {:?} asks", questions_modal.tier),
            question_content(questions_modal),
            TYPING_HINTS.to_vec(),
            &questions_modal.scroll,
        ),
    };
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(100);
    let text_width = width.saturating_sub(BORDER_AND_PADDING_COLUMNS);
    let wrapped_rows = wrapped_row_count(&content.lines, text_width);
    let wanted_height = u16::try_from(wrapped_rows + 2).unwrap_or(u16::MAX);
    let height = wanted_height.min(area.height.saturating_sub(2)).max(3);
    let visible_rows = usize::from(height - 2);
    let focus = content.focus.map(|focus_line| {
        let rows_before = wrapped_row_count(&content.lines[..focus_line], text_width);
        let rows_through = wrapped_row_count(&content.lines[..=focus_line], text_width);
        (rows_before, rows_through)
    });
    let top = scroll.visible_top(wrapped_rows, visible_rows, focus);
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
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(YELLOW))
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(YELLOW).bold(),
        ))
        .title_bottom(Line::from(hint_spans).alignment(Alignment::Right))
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(BAR));
    let (more_above, more_below) = (top > 0, top + visible_rows < wrapped_rows);
    if more_above || more_below {
        let arrows = format!(
            " {}PgUp/PgDn{} ",
            if more_above { "▲ " } else { "" },
            if more_below { " ▼" } else { "" }
        );
        block =
            block.title(Line::styled(arrows, Style::new().fg(ACCENT)).alignment(Alignment::Right));
    }
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(content.lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((u16::try_from(top).unwrap_or(u16::MAX), 0)),
        popup,
    );
}

/// The border takes a column on each side, and so does the padding.
const BORDER_AND_PADDING_COLUMNS: u16 = 4;

fn wrapped_row_count(lines: &[Line<'static>], text_width: u16) -> usize {
    Paragraph::new(lines.to_vec())
        .wrap(Wrap { trim: false })
        .line_count(text_width)
}

/// What a modal shows, and the line that must stay in view as the Operator selects or types.
struct ModalContent {
    lines: Vec<Line<'static>>,
    focus: Option<usize>,
}

pub(crate) const TYPING_HINTS: &[(&str, &str)] = &[
    ("Enter", "send"),
    ("Alt+Enter", "new line"),
    ("Esc", "back"),
];

fn checkpoint_title(checkpoint: &Checkpoint) -> String {
    match checkpoint {
        Checkpoint::Plan { .. } => "Plan Checkpoint".to_owned(),
        Checkpoint::Scaffold { task, .. } => format!("Scaffold Checkpoint · Task {}", task + 1),
        Checkpoint::Revision { task, .. } => format!("Revision Checkpoint · Task {}", task + 1),
        Checkpoint::Escalation { task, .. } => format!("Escalation · Task {}", task + 1),
        Checkpoint::Replan { task, .. } => format!("Replan Checkpoint · Task {}", task + 1),
        Checkpoint::Markers { .. } => "Marker Checkpoint".to_owned(),
        Checkpoint::Budget { .. } => "Budget Checkpoint".to_owned(),
    }
}

fn checkpoint_hints(checkpoint_modal: &CheckpointModal) -> Vec<(&'static str, &'static str)> {
    if let Checkpoint::Escalation { .. } = checkpoint_modal.checkpoint {
        return escalation_hints(checkpoint_modal);
    }
    let hints: &[(&str, &str)] = if let Checkpoint::Markers { .. } = checkpoint_modal.checkpoint {
        &[
            ("a", "approve as they are"),
            ("f", "I removed them"),
            ("s", "stop"),
            ("Esc", "look around"),
        ]
    } else if let Checkpoint::Budget { .. } = checkpoint_modal.checkpoint {
        &[
            ("Enter", "raise the cap"),
            ("u", "no cap"),
            ("s", "stop"),
            ("Esc", "look around"),
        ]
    } else if checkpoint_modal.feedback.is_some() {
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
    };
    hints.to_vec()
}

fn checkpoint_content(checkpoint_modal: &CheckpointModal) -> ModalContent {
    let mut focus = None;
    let mut lines = match &checkpoint_modal.checkpoint {
        Checkpoint::Plan { plan } => {
            let mut lines = vec![labelled("Goal", &plan.goal)];
            for constraint in &plan.constraints {
                lines.push(constraint_line(constraint));
            }
            lines.push(Line::default());
            focus = Some(lines.len() + 1 + checkpoint_modal.selected_row);
            lines.extend(task_lines(checkpoint_modal, &plan.tasks, "Plan"));
            lines
        }
        Checkpoint::Replan { replan, .. } => {
            let mut lines = vec![labelled("Diagnosis", &replan.diagnosis), Line::default()];
            lines.push(Line::styled(
                "These Tasks replace the failed one, which starts again from its first commit.",
                Style::new().fg(MUTED),
            ));
            focus = Some(lines.len() + 1 + checkpoint_modal.selected_row);
            lines.extend(task_lines(checkpoint_modal, &replan.tasks, "Replan"));
            lines
        }
        Checkpoint::Escalation { .. } => {
            let (lines, typing_focus) = escalation_lines(checkpoint_modal);
            focus = typing_focus;
            lines
        }
        Checkpoint::Markers { remaining } => marker_lines(remaining),
        Checkpoint::Budget { cap, spent } => {
            let lines = budget_lines(*cap, *spent, &checkpoint_modal.budget_cap_text);
            focus = Some(BUDGET_CAP_LINE);
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
        focus = Some(lines.len() - 1);
    }
    ModalContent { lines, focus }
}

fn task_lines(
    checkpoint_modal: &CheckpointModal,
    tasks: &[Task],
    proposal: &str,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled("Tasks", Style::new().fg(MUTED))];
    for (row_index, row) in checkpoint_modal.task_rows.iter().enumerate() {
        lines.push(task_row_line(
            row,
            &tasks[row.task].title,
            row_index == checkpoint_modal.selected_row,
        ));
    }
    if checkpoint_modal.task_order().is_empty() {
        lines.push(Line::styled(
            format!("Keep at least one Task to approve the {proposal}."),
            Style::new().fg(YELLOW),
        ));
    }
    lines
}

/// Check output is long; its last lines usually say what failed.
const CHECK_OUTPUT_LINES: usize = 8;

/// More Markers than this are counted, not listed.
const LISTED_MARKERS: usize = 12;

fn marker_lines(remaining: &[LeftoverMarker]) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::raw("The branch's commits still hold Markers, so the Session can't finish yet."),
        Line::default(),
    ];
    for marker in remaining.iter().take(LISTED_MARKERS) {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}:{}  ", marker.file, marker.line),
                Style::new().fg(ACCENT),
            ),
            Span::styled(marker.text.clone(), Style::new().fg(FAINT)),
        ]));
    }
    if remaining.len() > LISTED_MARKERS {
        lines.push(Line::styled(
            format!("…and {} more", remaining.len() - LISTED_MARKERS),
            Style::new().fg(MUTED),
        ));
    }
    lines.push(Line::default());
    for option in [
        "a  Approve them as they are, e.g. fixtures or docs that only look like Markers.",
        "f  Remove them yourself in the worktree; Arbor commits that and checks again.",
        "s  Stop here, keeping the branch and worktree.",
    ] {
        lines.push(Line::styled(option, Style::new().fg(MUTED)));
    }
    lines
}

const BUDGET_CAP_LINE: usize = 2;

fn budget_lines(cap: Usd, spent: Usd, budget_cap_text: &str) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::raw(format!(
            "The Session has spent ${:.4} of its ${:.2} Budget Cap. No new model call starts \
             until you raise the cap or stop.",
            spent.dollars(),
            cap.dollars()
        )),
        Line::default(),
        Line::from(vec![
            Span::styled("New cap  $", Style::new().fg(MUTED)),
            Span::raw(budget_cap_text.to_owned()),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ]),
    ];
    if typed_budget_cap(budget_cap_text, spent).is_none() {
        lines.push(Line::styled(
            format!("Type an amount above ${:.4}.", spent.dollars()),
            Style::new().fg(YELLOW),
        ));
    }
    lines.push(Line::default());
    for option in [
        "u  Carry on with no cap.",
        "s  Stop the Session here, keeping the branch and worktree.",
    ] {
        lines.push(Line::styled(option, Style::new().fg(MUTED)));
    }
    lines
}

pub(crate) fn output_tail(check_output: &str) -> Vec<Line<'static>> {
    let output_lines: Vec<&str> = check_output.trim_end().lines().collect();
    let start = output_lines.len().saturating_sub(CHECK_OUTPUT_LINES);
    output_lines[start..]
        .iter()
        .map(|output_line| Line::styled(format!("  {output_line}"), Style::new().fg(FAINT)))
        .collect()
}

fn question_content(questions_modal: &QuestionsModal) -> ModalContent {
    let mut lines: Vec<Line<'static>> = questions_modal
        .questions
        .lines()
        .map(|question_line| Line::raw(question_line.to_owned()))
        .collect();
    lines.push(Line::default());
    lines.push(Line::styled("Your answers", Style::new().fg(MUTED)));
    lines.extend(typed_lines(&questions_modal.answers));
    let focus = Some(lines.len() - 1);
    ModalContent { lines, focus }
}

pub(crate) fn labelled(name: &str, labelled_text: &str) -> Line<'static> {
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
pub(crate) fn typed_lines(typed: &str) -> Vec<Line<'static>> {
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
