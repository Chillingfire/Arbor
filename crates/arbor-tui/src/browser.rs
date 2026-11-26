//! The Session browser: every stored Session with its status, age, cost, Markers and time until
//! pruned, where the Human Operator inspects, resumes, rewinds or deletes one.

use std::time::Duration;

use arbor_core::{Phase, RewindPoint};
use arbor_session::{SessionFolder, SessionStatus, SessionSummary, resume_resets_worktree};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};

use crate::ui::{ACCENT, BAR, GREEN, MUTED, RED, YELLOW, phase_name};

/// What the Human Operator asked of the browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserOutcome {
    Close,
    /// Show the Session's event log and tree without running it.
    Inspect(SessionFolder),
    Resume(SessionFolder),
    /// Delete the Session and its branch; already confirmed.
    Delete(SessionFolder),
    /// List the Checkpoints the Session can be rewound to, with [`SessionBrowser::choose_rewind`].
    ListRewindPoints(SessionFolder),
    /// Rewind the Session to the Checkpoint, then resume it; already confirmed.
    Rewind(SessionFolder, Box<RewindPoint>),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum Mode {
    #[default]
    List,
    ConfirmDelete,
    /// Resuming resets the worktree, which holds uncommitted changes.
    ConfirmResume,
    ChooseRewind {
        points: Vec<RewindPoint>,
        selected: usize,
    },
    ConfirmRewind(Box<RewindPoint>),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionBrowser {
    sessions: Vec<SessionSummary>,
    selected: usize,
    mode: Mode,
    /// The result of the last action, or why it couldn't be done.
    note: Option<String>,
}

impl SessionBrowser {
    pub fn new(sessions: Vec<SessionSummary>) -> Self {
        Self {
            sessions,
            ..Self::default()
        }
    }

    pub fn sessions(&self) -> &[SessionSummary] {
        &self.sessions
    }

    pub fn selected(&self) -> Option<&SessionSummary> {
        self.sessions.get(self.selected)
    }

    /// Shows a fresh listing, e.g. after a Delete, keeping the selection near where it was.
    pub fn refresh(&mut self, sessions: Vec<SessionSummary>) {
        self.selected = self.selected.min(sessions.len().saturating_sub(1));
        self.sessions = sessions;
        self.mode = Mode::List;
    }

    pub fn set_note(&mut self, note: impl Into<String>) {
        self.note = Some(note.into());
    }

    /// Lets the Human Operator pick one of the selected Session's Checkpoints, newest first.
    pub fn choose_rewind(&mut self, mut points: Vec<RewindPoint>) {
        if points.is_empty() {
            self.set_note("This Session has no Checkpoint to rewind to.");
            return;
        }
        points.reverse();
        self.mode = Mode::ChooseRewind {
            points,
            selected: 0,
        };
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<BrowserOutcome> {
        match std::mem::take(&mut self.mode) {
            Mode::List => self.handle_list_key(key),
            Mode::ConfirmDelete => {
                let folder = self.selected()?.folder.clone();
                is_ctrl(key, 'd').then_some(BrowserOutcome::Delete(folder))
            }
            Mode::ConfirmResume => {
                let folder = self.selected()?.folder.clone();
                is_ctrl(key, 'r').then_some(BrowserOutcome::Resume(folder))
            }
            Mode::ChooseRewind { points, selected } => {
                self.handle_rewind_key(key, points, selected)
            }
            Mode::ConfirmRewind(point) => {
                let folder = self.selected()?.folder.clone();
                is_ctrl(key, 'r').then_some(BrowserOutcome::Rewind(folder, point))
            }
        }
    }

    fn handle_list_key(&mut self, key: KeyEvent) -> Option<BrowserOutcome> {
        self.note = None;
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('b' | 'q') => return Some(BrowserOutcome::Close),
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(self.sessions.len().saturating_sub(1));
            }
            KeyCode::Enter => {
                return Some(BrowserOutcome::Inspect(self.selected()?.folder.clone()));
            }
            KeyCode::Char('r') => return self.resume(),
            KeyCode::Char('w') => {
                return self.runnable(true).map(BrowserOutcome::ListRewindPoints);
            }
            KeyCode::Char('d') => match self.selected()?.is_running() {
                true => self.set_note("A running Session can't be deleted."),
                false => self.mode = Mode::ConfirmDelete,
            },
            _ => {}
        }
        None
    }

    fn handle_rewind_key(
        &mut self,
        key: KeyEvent,
        points: Vec<RewindPoint>,
        selected: usize,
    ) -> Option<BrowserOutcome> {
        let selected = match key.code {
            KeyCode::Esc => return None,
            KeyCode::Enter => {
                self.mode = Mode::ConfirmRewind(Box::new(points.get(selected)?.clone()));
                return None;
            }
            KeyCode::Up | KeyCode::Char('k') => selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => (selected + 1).min(points.len() - 1),
            _ => selected,
        };
        self.mode = Mode::ChooseRewind { points, selected };
        None
    }

    /// Resumes the selected Session, asking first when resuming would discard uncommitted
    /// changes in its worktree.
    fn resume(&mut self) -> Option<BrowserOutcome> {
        let folder = self.runnable(false)?;
        let summary = self.selected()?;
        let discards_changes = summary.uncommitted_changes
            && summary.phase.as_ref().is_some_and(resume_resets_worktree);
        if discards_changes {
            self.mode = Mode::ConfirmResume;
            return None;
        }
        Some(BrowserOutcome::Resume(folder))
    }

    /// The selected Session's folder, if it can run again: not running and with a log that
    /// replays. Only a Rewind carries on a finished Session, or one stopped at the Budget Cap.
    fn runnable(&mut self, rewinding: bool) -> Option<SessionFolder> {
        let summary = self.selected()?;
        let refusal = match (&summary.status, &summary.phase) {
            (SessionStatus::Running, _) => Some("This Session is already running."),
            (_, None) => Some("This Session's event log can't be replayed."),
            _ if rewinding => None,
            (SessionStatus::Finished, _) => Some("This Session is finished; rewind it instead."),
            (SessionStatus::FinishedWithSkippedTasks(_), _) => {
                Some("This Session finished with skipped Tasks; rewind it instead.")
            }
            (_, Some(Phase::Stopped)) => {
                Some("This Session was stopped at the Budget Cap; rewind it instead.")
            }
            (_, Some(Phase::Aborted)) => Some("This Session was aborted; rewind it instead."),
            _ => None,
        };
        match refusal {
            Some(refusal) => {
                self.set_note(refusal);
                None
            }
            None => Some(summary.folder.clone()),
        }
    }
}

fn is_ctrl(key: KeyEvent, letter: char) -> bool {
    key.code == KeyCode::Char(letter) && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// `45m`, `5h` or `12d`.
pub fn duration_label(duration: Duration) -> String {
    let minutes = duration.as_secs() / 60;
    match minutes {
        0..60 => format!("{minutes}m"),
        60..1440 => format!("{}h", minutes / 60),
        _ => format!("{}d", minutes / 1440),
    }
}

fn status_span(status: &SessionStatus) -> Span<'static> {
    let (label, color) = match status {
        SessionStatus::Running => ("running".to_owned(), ACCENT),
        SessionStatus::Cancelled => ("cancelled".to_owned(), YELLOW),
        SessionStatus::Failed(_) => ("failed".to_owned(), RED),
        SessionStatus::Finished => ("finished".to_owned(), GREEN),
        SessionStatus::FinishedWithSkippedTasks(skipped) => (format!("{skipped} skipped"), YELLOW),
        SessionStatus::Aborted => ("aborted".to_owned(), MUTED),
    };
    Span::styled(format!("{label:<10}"), Style::new().fg(color).bold())
}

fn markers_label(markers_at_tip: Option<usize>) -> Span<'static> {
    match markers_at_tip {
        Some(0) => Span::styled("no Markers ", Style::new().fg(MUTED)),
        Some(count) => Span::styled(format!("{count:>2} Markers "), Style::new().fg(YELLOW)),
        None => Span::styled("Markers ?  ", Style::new().fg(MUTED)),
    }
}

fn session_row(summary: &SessionSummary, is_selected: bool) -> Line<'static> {
    let pruned_in = summary.pruned_in.map_or("never".to_owned(), duration_label);
    let mut row = Line::from(vec![
        Span::raw(if is_selected { "❯ " } else { "  " }),
        status_span(&summary.status),
        Span::raw(format!("{:>4} ago  ", duration_label(summary.idle))),
        Span::styled(format!("prune {pruned_in:>5}  "), Style::new().fg(MUTED)),
        Span::raw(format!("${:>7.4}  ", summary.cost.dollars())),
        markers_label(summary.markers_at_tip),
        Span::styled(format!("{}  ", summary.branch), Style::new().fg(ACCENT)),
        Span::raw(summary.request.clone()),
    ]);
    if is_selected {
        row = row.bold();
    }
    row
}

fn selected_details(summary: &SessionSummary) -> Line<'static> {
    let waiting = summary
        .phase
        .as_ref()
        .map_or("unknown".to_owned(), phase_name);
    let mut details = format!("  {} · at {waiting}", summary.folder.id());
    if let SessionStatus::Failed(reason) = &summary.status {
        details.push_str(&format!(" · {reason}"));
    }
    Line::styled(details, Style::new().fg(MUTED))
}

fn rewind_point_label(point: &RewindPoint) -> String {
    let commit = point
        .head
        .as_deref()
        .map_or("the branch start".to_owned(), |hash| {
            hash.get(..7).unwrap_or(hash).to_owned()
        });
    format!(
        "{} · after event {} · at {commit}",
        phase_name(&point.phase),
        point.event_count
    )
}

fn uncommitted_warning(summary: &SessionSummary) -> String {
    if !summary.uncommitted_changes {
        return String::new();
    }
    format!(
        " The uncommitted changes in {} are lost too.",
        summary.folder.worktree_path().display()
    )
}

fn hints(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (keys, action) in pairs {
        spans.push(Span::styled(format!("{keys} "), Style::new().fg(ACCENT)));
        spans.push(Span::styled(format!("{action}  "), Style::new().fg(MUTED)));
    }
    Line::from(spans)
}

fn list_lines(browser: &SessionBrowser) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if browser.sessions.is_empty() {
        lines.push(Line::styled(
            "No Sessions are stored in this project.",
            Style::new().fg(MUTED).italic(),
        ));
    }
    for (index, summary) in browser.sessions.iter().enumerate() {
        let is_selected = index == browser.selected;
        lines.push(session_row(summary, is_selected));
        if is_selected {
            lines.push(selected_details(summary));
        }
    }
    lines
}

/// What the browser asks or offers under the list.
fn footer_lines(browser: &SessionBrowser) -> Vec<Line<'static>> {
    let mut lines = vec![Line::default()];
    match &browser.mode {
        Mode::List => lines.push(hints(&[
            ("↑↓", "move"),
            ("Enter", "inspect"),
            ("r", "resume"),
            ("w", "rewind"),
            ("d", "delete"),
            ("Esc", "close"),
        ])),
        Mode::ConfirmDelete => {
            if let Some(summary) = browser.selected() {
                lines.push(Line::styled(
                    format!(
                        "Delete Session {} and its branch {}? Its worktree and event log go too, \
                         and the branch can't be brought back.{}",
                        summary.folder.id(),
                        summary.branch,
                        uncommitted_warning(summary)
                    ),
                    Style::new().fg(RED).bold(),
                ));
            }
            lines.push(hints(&[("Ctrl+D", "delete"), ("any other key", "keep it")]));
        }
        Mode::ConfirmResume => {
            if let Some(summary) = browser.selected() {
                lines.push(Line::styled(
                    format!(
                        "Resume Session {}? It redoes its unfinished step from the last commit, \
                         so the uncommitted changes in {} are discarded. Commit or stash them \
                         first to keep them.",
                        summary.folder.id(),
                        summary.folder.worktree_path().display()
                    ),
                    Style::new().fg(RED).bold(),
                ));
            }
            lines.push(hints(&[
                ("Ctrl+R", "discard them and resume"),
                ("any other key", "back"),
            ]));
        }
        Mode::ChooseRewind { points, selected } => {
            lines.push(Line::styled(
                "Rewind to which Checkpoint?",
                Style::new().bold(),
            ));
            for (index, point) in points.iter().enumerate() {
                let marker = if index == *selected { "❯ " } else { "  " };
                lines.push(Line::raw(format!("{marker}{}", rewind_point_label(point))));
            }
            lines.push(hints(&[
                ("↑↓", "move"),
                ("Enter", "choose"),
                ("Esc", "back"),
            ]));
        }
        Mode::ConfirmRewind(point) => {
            lines.push(Line::styled(
                format!(
                    "Rewind to {}? The worktree is reset to that Checkpoint and the work done \
                     since is discarded; the event log keeps its transcript.{}",
                    rewind_point_label(point),
                    browser
                        .selected()
                        .map_or(String::new(), uncommitted_warning)
                ),
                Style::new().fg(RED).bold(),
            ));
            lines.push(hints(&[
                ("Ctrl+R", "rewind and resume"),
                ("any other key", "back"),
            ]));
        }
    }
    if let Some(note) = &browser.note {
        lines.push(Line::styled(note.clone(), Style::new().fg(YELLOW)));
    }
    lines
}

/// How many rows `lines` take wrapped to `width` columns.
fn wrapped_rows(lines: &[Line<'_>], width: u16) -> u16 {
    let width = usize::from(width.max(1));
    let rows: usize = lines
        .iter()
        .map(|line| line.width().div_ceil(width).max(1))
        .sum();
    u16::try_from(rows).unwrap_or(u16::MAX)
}

pub(crate) fn draw_browser(frame: &mut Frame, browser: &SessionBrowser) {
    let area = frame.area();
    let popup = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Span::styled(" Sessions ", Style::new().fg(ACCENT).bold()))
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(BAR));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    let footer = footer_lines(browser);
    let footer_height = wrapped_rows(&footer, inner.width).min(inner.height / 2);
    let list_height = inner.height - footer_height;
    // Rows don't wrap, and the selected Session's details line sits just below it.
    let selected_bottom = u16::try_from(browser.selected + 2).unwrap_or(u16::MAX);
    let list_top = selected_bottom.saturating_sub(list_height);
    frame.render_widget(
        Paragraph::new(list_lines(browser)).scroll((list_top, 0)),
        Rect {
            height: list_height,
            ..inner
        },
    );
    frame.render_widget(
        Paragraph::new(footer).wrap(Wrap { trim: false }),
        Rect {
            y: inner.y + list_height,
            height: footer_height,
            ..inner
        },
    );
}
