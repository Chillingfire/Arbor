//! Draws the provisional layout (grilling log Round 2 decision 10): a header, the Plan tree on
//! the left, the detail pane on the right, the event log and the prompt below them, and the status
//! bar at the bottom.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::time::Duration;

use arbor_config::ClarificationMode;
use arbor_core::{ASSUMPTION_PREFIX, Phase, Tier};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::symbols::scrollbar;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, List, ListItem, ListState, Padding, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};

use crate::view::{
    LogEntry, ReplyOutcome, SessionView, Status, Tone, WorkUnitView, first_line, phase_task, short,
    tokens,
};

const ACCENT: Color = Color::Rgb(122, 162, 247);
const MUTED: Color = Color::Rgb(110, 118, 135);
const FAINT: Color = Color::Rgb(70, 76, 90);
const SELECTION: Color = Color::Rgb(40, 46, 62);
const BAR: Color = Color::Rgb(28, 31, 40);
const GREEN: Color = Color::Rgb(158, 206, 106);
const YELLOW: Color = Color::Rgb(224, 175, 104);
const RED: Color = Color::Rgb(247, 118, 142);

const LOG_ROWS: u16 = 10;

/// A row of the Plan tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TreeNode {
    #[default]
    Plan,
    Task(usize),
    WorkUnit {
        task: usize,
        unit: usize,
    },
}

/// The tree's rows, top to bottom, leaving out the Work Units of `collapsed` Tasks.
pub fn tree_nodes(view: &SessionView, collapsed: &BTreeSet<usize>) -> Vec<TreeNode> {
    if view.plan().is_none() {
        return Vec::new();
    }
    let mut nodes = vec![TreeNode::Plan];
    for (task, task_view) in view.tasks().iter().enumerate() {
        nodes.push(TreeNode::Task(task));
        if collapsed.contains(&task) {
            continue;
        }
        nodes.extend((0..task_view.work_units.len()).map(|unit| TreeNode::WorkUnit { task, unit }));
    }
    nodes
}

/// The row of `selected` among `nodes`, or the top row when it isn't shown.
pub(crate) fn selected_row(nodes: &[TreeNode], selected: TreeNode) -> usize {
    nodes.iter().position(|node| *node == selected).unwrap_or(0)
}

/// The pane that takes the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Tree,
    Detail,
    Log,
    Prompt,
}

/// What the detail pane shows for a Work Unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailTab {
    #[default]
    Spec,
    Transcript,
    Diff,
}

/// A key that would stop the running Session, waiting for the Human Operator to confirm it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    Quit,
    Cancel,
}

/// The TUI's own state, apart from the Session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UiState {
    /// Kept as the node, not its row, so it stays put when Work Units arrive above it.
    pub selected: TreeNode,
    pub focus: Focus,
    pub detail_tab: DetailTab,
    /// The first entry the event log shows once scrolled back, or `None` to follow new events.
    /// Kept as an entry, not a distance from the newest, so the view holds still as events arrive.
    pub log_top: Option<usize>,
    /// How many log entries fit, as last drawn.
    pub log_rows: Cell<usize>,
    /// What the Human Operator has typed into the prompt.
    pub prompt: String,
    /// Tasks whose Work Units the tree hides.
    pub collapsed: BTreeSet<usize>,
    /// Lines the detail pane is scrolled down.
    pub detail_scroll: u16,
    /// How far the detail pane can scroll, as last drawn.
    pub detail_scroll_limit: Cell<u16>,
    pub confirming: Option<Confirmation>,
}

/// What the screen shows besides the Session's own numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusInfo {
    /// The model filling each Tier: Planner, Scaffolder, Writer.
    pub models: [String; 3],
    pub clarification_mode: ClarificationMode,
    /// The Session branch, once it exists.
    pub branch: Option<String>,
    /// The model call or Check running now.
    pub activity: Option<RunningActivity>,
}

/// What the spinner in the header shows.
#[derive(Debug, Clone, PartialEq)]
pub struct RunningActivity {
    pub tier: Option<Tier>,
    pub model: Option<String>,
    pub label: String,
    pub elapsed: Duration,
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn draw(frame: &mut Frame, view: &SessionView, ui_state: &UiState, status: &StatusInfo) {
    frame.render_widget(
        Block::new().style(Style::new().fg(Color::Rgb(192, 202, 245))),
        frame.area(),
    );
    let [header_area, main_area, log_area, prompt_area, status_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(8),
        Constraint::Length(LOG_ROWS + 2),
        Constraint::Length(3),
        Constraint::Length(2),
    ])
    .areas(frame.area());
    let [tree_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
            .areas(main_area);

    let nodes = tree_nodes(view, &ui_state.collapsed);
    draw_header(frame, header_area, view, status);
    draw_tree(frame, tree_area, view, &nodes, ui_state);
    draw_detail(
        frame,
        detail_area,
        view,
        nodes.get(selected_row(&nodes, ui_state.selected)).copied(),
        ui_state,
    );
    draw_log(frame, log_area, view, ui_state);
    draw_prompt(frame, prompt_area, view, ui_state);
    draw_status(frame, status_area, view, status);
    if let Some(confirmation) = ui_state.confirming {
        draw_confirmation(frame, confirmation, status);
    }
}

fn pane(title: Line<'_>, focused: bool) -> Block<'_> {
    let border_color = if focused { ACCENT } else { FAINT };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border_color))
        .title(title)
        .padding(Padding::horizontal(1))
}

fn pane_title(text: &str, focused: bool) -> Line<'static> {
    let style = if focused {
        Style::new().fg(ACCENT).bold()
    } else {
        Style::new().fg(MUTED).bold()
    };
    Line::from(Span::styled(format!(" {text} "), style))
}

fn tier_color(tier: Tier) -> Color {
    match tier {
        Tier::Planner => Color::Rgb(187, 154, 247),
        Tier::Scaffolder => Color::Rgb(125, 207, 255),
        Tier::Writer => GREEN,
    }
}

fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Planner => "Planner",
        Tier::Scaffolder => "Scaffolder",
        Tier::Writer => "Writer",
    }
}

fn badge(text: &str, background: Color) -> Span<'static> {
    Span::styled(
        format!(" {text} "),
        Style::new()
            .fg(Color::Rgb(26, 27, 38))
            .bg(background)
            .bold(),
    )
}

fn status_icon(status: Status) -> Span<'static> {
    match status {
        Status::Pending => Span::styled("○", Style::new().fg(MUTED)),
        Status::Active => Span::styled("◉", Style::new().fg(YELLOW).bold()),
        Status::Done => Span::styled("✔", Style::new().fg(GREEN)),
        Status::Failed => Span::styled("✘", Style::new().fg(RED).bold()),
    }
}

fn draw_header(frame: &mut Frame, area: Rect, view: &SessionView, status: &StatusInfo) {
    let work_unit_statuses: Vec<Status> = view
        .tasks()
        .iter()
        .flat_map(|task_view| &task_view.work_units)
        .map(|work_unit_view| work_unit_view.status)
        .collect();
    let done = work_unit_statuses
        .iter()
        .filter(|&&work_unit_status| work_unit_status == Status::Done)
        .count();
    let mut left = vec![badge("🌳 arbor", GREEN), Span::raw(" ")];
    match (view.plan(), view.session().request()) {
        (Some(plan), _) => left.push(Span::raw(plan.goal.clone()).bold()),
        (None, Some(request)) => left.push(Span::raw(first_line(&request.text).to_owned()).bold()),
        (None, None) => left.push(Span::styled("No Session yet", Style::new().fg(MUTED))),
    }
    if let Some(branch) = &status.branch {
        left.push(Span::styled(
            format!("  ⎇ {branch}"),
            Style::new().fg(MUTED),
        ));
    }

    let mut right = Vec::new();
    if let Some(activity) = &status.activity {
        let frame_index = (activity.elapsed.as_millis() / 100) as usize % SPINNER.len();
        let color = activity.tier.map_or(YELLOW, tier_color);
        right.push(Span::styled(
            format!("{} ", SPINNER[frame_index]),
            Style::new().fg(color).bold(),
        ));
        if let Some(tier) = activity.tier {
            right.push(Span::styled(
                format!("{} ", tier_name(tier)),
                Style::new().fg(color).bold(),
            ));
        }
        if let Some(model) = &activity.model {
            right.push(Span::styled(
                format!("{} · ", short_model(model)),
                Style::new().fg(MUTED),
            ));
        }
        right.push(Span::raw(format!("{} ", activity.label)));
        right.push(Span::styled(
            format!("{}s   ", activity.elapsed.as_secs()),
            Style::new().fg(MUTED),
        ));
    }
    if !work_unit_statuses.is_empty() {
        let bar_width = 12;
        let filled = done * bar_width / work_unit_statuses.len();
        right.push(Span::styled("▰".repeat(filled), Style::new().fg(GREEN)));
        right.push(Span::styled(
            "▱".repeat(bar_width - filled),
            Style::new().fg(FAINT),
        ));
        right.push(Span::styled(
            format!(" {done}/{} Work Units ", work_unit_statuses.len()),
            Style::new().fg(MUTED),
        ));
    }
    let right = Line::from(right);
    let right_width = u16::try_from(right.width()).unwrap_or(u16::MAX);
    let [left_area, _gap, right_area] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(2),
        Constraint::Length(right_width),
    ])
    .areas(area);
    let header = Block::new().style(Style::new().bg(BAR));
    frame.render_widget(header, area);
    frame.render_widget(Paragraph::new(Line::from(left)), left_area);
    frame.render_widget(Paragraph::new(right), right_area);
}

fn draw_tree(
    frame: &mut Frame,
    area: Rect,
    view: &SessionView,
    nodes: &[TreeNode],
    ui_state: &UiState,
) {
    let focused = ui_state.focus == Focus::Tree;
    let block = pane(pane_title("Plan", focused), focused);
    let Some(plan) = view.plan() else {
        let waiting =
            Paragraph::new(Line::styled("No Plan yet.", Style::new().fg(MUTED))).block(block);
        frame.render_widget(waiting, area);
        return;
    };
    let guide = |text: &str| Span::styled(text.to_owned(), Style::new().fg(FAINT));
    let task_count = view.tasks().len();
    let items: Vec<ListItem> = nodes
        .iter()
        .map(|node| match *node {
            TreeNode::Plan => ListItem::new(Line::from(vec![
                Span::styled("◆ ", Style::new().fg(ACCENT)),
                Span::raw(plan.goal.clone()).bold(),
            ])),
            TreeNode::Task(task) => {
                let task_view = &view.tasks()[task];
                let branch = if task + 1 == task_count {
                    "└─ "
                } else {
                    "├─ "
                };
                let title_style = match task_view.status {
                    Status::Active => Style::new().bold(),
                    Status::Pending => Style::new().fg(MUTED),
                    _ => Style::new(),
                };
                let mut spans = vec![
                    guide(branch),
                    status_icon(task_view.status),
                    Span::styled(
                        format!(" {}. {}", task + 1, task_view.task.title),
                        title_style,
                    ),
                ];
                if ui_state.collapsed.contains(&task) && !task_view.work_units.is_empty() {
                    spans.push(Span::styled(
                        format!("  ▸ {} hidden", task_view.work_units.len()),
                        Style::new().fg(FAINT),
                    ));
                }
                ListItem::new(Line::from(spans))
            }
            TreeNode::WorkUnit { task, unit } => {
                let task_view = &view.tasks()[task];
                let work_unit_view = &task_view.work_units[unit];
                let work_unit = &work_unit_view.work_unit;
                let trunk = if task + 1 == task_count {
                    "   "
                } else {
                    "│  "
                };
                let branch = if unit + 1 == task_view.work_units.len() {
                    "└─ "
                } else {
                    "├─ "
                };
                ListItem::new(Line::from(vec![
                    guide(trunk),
                    guide(branch),
                    status_icon(work_unit_view.status),
                    Span::styled(format!(" {}", work_unit.id), Style::new().bold()),
                    Span::styled(format!("  {}", work_unit.file), Style::new().fg(MUTED)),
                ]))
            }
        })
        .collect();
    let highlight = if focused {
        Style::new().bg(SELECTION).add_modifier(Modifier::BOLD)
    } else {
        Style::new().bg(BAR)
    };
    let list = List::new(items)
        .block(block)
        .highlight_style(highlight)
        .highlight_symbol(Span::styled("▎", Style::new().fg(ACCENT)));
    let mut list_state =
        ListState::default().with_selected(Some(selected_row(nodes, ui_state.selected)));
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn heading(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_owned(),
        Style::new().fg(ACCENT).bold(),
    ))
}

fn label(text: &str) -> Span<'static> {
    Span::styled(format!("{text:<7}"), Style::new().fg(MUTED))
}

fn tabs_title(title: &str, active: DetailTab, focused: bool) -> Line<'static> {
    let mut spans = pane_title(title, focused).spans;
    for (tab, name) in [
        (DetailTab::Spec, "Spec"),
        (DetailTab::Transcript, "Transcript"),
        (DetailTab::Diff, "Diff"),
    ] {
        let style = if tab == active {
            Style::new().fg(ACCENT).bold().underlined()
        } else {
            Style::new().fg(MUTED)
        };
        spans.push(Span::styled(name, style));
        spans.push(Span::styled(" · ", Style::new().fg(FAINT)));
    }
    spans.pop();
    spans.push(Span::raw(" "));
    Line::from(spans)
}

fn draw_detail(
    frame: &mut Frame,
    area: Rect,
    view: &SessionView,
    node: Option<TreeNode>,
    ui_state: &UiState,
) {
    let focused = ui_state.focus == Focus::Detail;
    let mut lines: Vec<Line> = Vec::new();
    let title = match node {
        None => {
            match view.session().request() {
                Some(request) => {
                    lines.push(heading("Request"));
                    lines.extend(request.text.lines().map(|line| Line::raw(line.to_owned())));
                }
                None => {
                    lines.push(Line::styled(
                        "Type a Request in the prompt below to start a Session.",
                        Style::new().fg(MUTED),
                    ));
                }
            }
            pane_title("Detail", focused)
        }
        Some(TreeNode::Plan) => {
            let plan = view
                .plan()
                .expect("the tree has a Plan row only with a Plan");
            lines.push(heading("Goal"));
            lines.push(Line::raw(plan.goal.clone()));
            if !plan.constraints.is_empty() {
                lines.push(Line::default());
                lines.push(heading("Constraints"));
                lines.extend(plan.constraints.iter().map(|constraint| {
                    match constraint.strip_prefix(ASSUMPTION_PREFIX) {
                        Some(assumption) => Line::from(vec![
                            Span::styled("▲ ", Style::new().fg(YELLOW)),
                            Span::styled("Assumption", Style::new().fg(YELLOW).bold()),
                            Span::raw(assumption.to_owned()),
                        ]),
                        None => Line::from(vec![
                            Span::styled("• ", Style::new().fg(MUTED)),
                            Span::raw(constraint.clone()),
                        ]),
                    }
                }));
            }
            pane_title("Plan", focused)
        }
        Some(TreeNode::Task(task)) => {
            let task_view = &view.tasks()[task];
            lines.push(heading("Description"));
            lines.extend(
                task_view
                    .task
                    .description
                    .lines()
                    .map(|line| Line::raw(line.to_owned())),
            );
            lines.push(Line::default());
            lines.push(Line::from(vec![
                label("Units"),
                Span::raw(task_view.work_units.len().to_string()),
            ]));
            pane_title(
                &format!("Task {}: {}", task + 1, task_view.task.title),
                focused,
            )
        }
        Some(TreeNode::WorkUnit { task, unit }) => {
            let work_unit_view = &view.tasks()[task].work_units[unit];
            let work_unit = &work_unit_view.work_unit;
            match ui_state.detail_tab {
                DetailTab::Spec => {
                    lines.push(Line::from(vec![
                        label("File"),
                        Span::raw(work_unit.file.clone()),
                    ]));
                    lines.push(Line::from(vec![
                        label("Tests"),
                        Span::raw(work_unit.test_filter.clone()),
                    ]));
                    lines.push(Line::default());
                    lines.push(heading("Spec"));
                    lines.extend(
                        work_unit
                            .spec
                            .lines()
                            .map(|line| Line::raw(line.to_owned())),
                    );
                    if !work_unit.acceptance_criteria.is_empty() {
                        lines.push(Line::default());
                        lines.push(heading("Acceptance criteria"));
                        lines.extend(work_unit.acceptance_criteria.iter().map(|criterion| {
                            Line::from(vec![
                                Span::styled("☐ ", Style::new().fg(MUTED)),
                                Span::raw(criterion.clone()),
                            ])
                        }));
                    }
                    lines.push(Line::default());
                    lines.push(heading("Context Pack"));
                    lines.extend(work_unit.context.iter().map(|snippet| {
                        Line::from(vec![
                            Span::styled("• ", Style::new().fg(MUTED)),
                            Span::raw(snippet.path.clone()),
                            Span::styled(
                                format!(
                                    "  {:?} · {} lines",
                                    snippet.priority,
                                    snippet.text.lines().count()
                                ),
                                Style::new().fg(MUTED),
                            ),
                        ])
                    }));
                }
                DetailTab::Transcript => transcript_lines(work_unit_view, &mut lines),
                DetailTab::Diff => diff_lines(work_unit_view, &mut lines),
            }
            tabs_title(&work_unit.id.to_string(), ui_state.detail_tab, focused)
        }
    };
    let line_count = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let visible_rows = area.height.saturating_sub(2);
    let scroll_limit = line_count.saturating_sub(visible_rows);
    ui_state.detail_scroll_limit.set(scroll_limit);
    let scroll = ui_state.detail_scroll.min(scroll_limit);
    let detail = Paragraph::new(lines)
        .block(pane(title, focused))
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(detail, area);
    if line_count > visible_rows {
        let mut scrollbar_state = ScrollbarState::new(usize::from(line_count - visible_rows))
            .position(usize::from(scroll));
        frame.render_stateful_widget(
            pane_scrollbar(focused),
            area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

fn pane_scrollbar(focused: bool) -> Scrollbar<'static> {
    Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .symbols(scrollbar::VERTICAL)
        .begin_symbol(None)
        .end_symbol(None)
        .thumb_style(Style::new().fg(if focused { ACCENT } else { MUTED }))
        .track_style(Style::new().fg(FAINT))
}

fn code_line(text: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("│ ", Style::new().fg(FAINT)),
        Span::styled(text.to_owned(), Style::new().fg(Color::Rgb(169, 177, 214))),
    ])
}

fn transcript_lines(work_unit_view: &WorkUnitView, lines: &mut Vec<Line<'static>>) {
    if work_unit_view.transcript.is_empty() {
        lines.push(Line::styled(
            "The Writer hasn't replied yet.",
            Style::new().fg(MUTED),
        ));
        return;
    }
    for (attempt_index, record) in work_unit_view.transcript.iter().enumerate() {
        if attempt_index > 0 {
            lines.push(Line::default());
        }
        let (marker, verdict) = match &record.outcome {
            ReplyOutcome::Passed => (Span::styled("✔ ", Style::new().fg(GREEN)), "passed"),
            ReplyOutcome::Failed(_) => (Span::styled("✘ ", Style::new().fg(RED)), "failed"),
            ReplyOutcome::Missing(_) => (
                Span::styled("? ", Style::new().fg(YELLOW)),
                "sent a Missing Response",
            ),
        };
        lines.push(Line::from(vec![
            marker,
            Span::styled(
                format!("Attempt {}", attempt_index + 1),
                Style::new().fg(ACCENT).bold(),
            ),
            Span::styled(format!(" {verdict}"), Style::new().fg(MUTED)),
        ]));
        lines.extend(
            record
                .reply
                .lines()
                .filter(|reply_line| !reply_line.trim_start().starts_with("```"))
                .map(code_line),
        );
        match &record.outcome {
            ReplyOutcome::Passed => {}
            ReplyOutcome::Failed(error) => {
                lines.extend(
                    error.lines().map(|error_line| {
                        Line::styled(error_line.to_owned(), Style::new().fg(RED))
                    }),
                );
            }
            ReplyOutcome::Missing(missing_items) => {
                lines.extend(missing_items.iter().map(|missing_item| {
                    Line::styled(format!("needs: {missing_item}"), Style::new().fg(YELLOW))
                }));
            }
        }
    }
}

fn diff_lines(work_unit_view: &WorkUnitView, lines: &mut Vec<Line<'static>>) {
    let Some(commit) = &work_unit_view.commit else {
        lines.push(Line::styled(
            "Nothing committed for this Work Unit yet.",
            Style::new().fg(MUTED),
        ));
        return;
    };
    lines.push(Line::from(vec![
        label("Commit"),
        Span::styled(short(commit).to_owned(), Style::new().fg(YELLOW)),
    ]));
    lines.push(Line::default());
    let Some(diff) = &work_unit_view.diff else {
        lines.push(Line::styled(
            "The diff hasn't been read yet.",
            Style::new().fg(MUTED),
        ));
        return;
    };
    lines.extend(diff.lines().map(|diff_line| {
        let style = if diff_line.starts_with("+++") || diff_line.starts_with("---") {
            Style::new().bold()
        } else if diff_line.starts_with('+') {
            Style::new().fg(GREEN)
        } else if diff_line.starts_with('-') {
            Style::new().fg(RED)
        } else if diff_line.starts_with("@@") {
            Style::new().fg(ACCENT)
        } else if diff_line.starts_with("diff ") || diff_line.starts_with("index ") {
            Style::new().fg(MUTED)
        } else {
            Style::new()
        };
        Line::styled(diff_line.to_owned(), style)
    }));
}

fn log_line(number: usize, entry: &LogEntry) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("{number:>4} "),
        Style::new().fg(FAINT),
    )];
    spans.push(match entry.tier {
        Some(tier) => Span::styled(
            format!("{:<10} ", tier_name(tier)),
            Style::new().fg(tier_color(tier)),
        ),
        None => Span::styled(format!("{:<10} ", "Session"), Style::new().fg(MUTED)),
    });
    let (marker, text_style) = match entry.tone {
        Tone::Success => ("✔ ", Style::new().fg(GREEN)),
        Tone::Failure => ("✘ ", Style::new().fg(RED)),
        Tone::Operator => ("➜ ", Style::new().fg(YELLOW)),
        Tone::ModelCall => ("· ", Style::new().fg(MUTED)),
        Tone::Progress => ("· ", Style::new()),
    };
    spans.push(Span::styled(marker, text_style));
    spans.push(Span::styled(entry.text.clone(), text_style));
    Line::from(spans)
}

fn draw_log(frame: &mut Frame, area: Rect, view: &SessionView, ui_state: &UiState) {
    let focused = ui_state.focus == Focus::Log;
    let log = view.log();
    let visible_rows = usize::from(area.height.saturating_sub(2));
    ui_state.log_rows.set(visible_rows);
    let newest_start = log.len().saturating_sub(visible_rows);
    let start = ui_state
        .log_top
        .map_or(newest_start, |top| top.min(newest_start));
    let lines: Vec<Line> = log
        .iter()
        .enumerate()
        .skip(start)
        .take(visible_rows)
        .map(|(index, entry)| log_line(index + 1, entry))
        .collect();
    let following = if start == newest_start {
        Span::styled(" following ", Style::new().fg(MUTED))
    } else {
        Span::styled(
            format!(" {} newer below ", newest_start - start),
            Style::new().fg(YELLOW),
        )
    };
    let block = pane(
        pane_title(&format!("Event log · {}", log.len()), focused),
        focused,
    )
    .title_top(Line::from(following).alignment(Alignment::Right));
    frame.render_widget(Paragraph::new(lines).block(block), area);

    if log.len() > visible_rows {
        let mut scrollbar_state = ScrollbarState::new(newest_start).position(start);
        frame.render_stateful_widget(
            pane_scrollbar(focused),
            area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

fn draw_prompt(frame: &mut Frame, area: Rect, view: &SessionView, ui_state: &UiState) {
    let focused = ui_state.focus == Focus::Prompt;
    let title = match view.session().phase() {
        Phase::NotStarted => "Request",
        _ => "Prompt",
    };
    let hint_pairs: &[(&str, &str)] = match ui_state.focus {
        Focus::Tree => &[
            ("Tab", "focus"),
            ("↑↓", "move"),
            ("←→", "fold"),
            ("/", "prompt"),
            ("q", "quit"),
        ],
        Focus::Detail => &[
            ("Tab", "focus"),
            ("↑↓", "scroll"),
            ("←→", "tabs"),
            ("q", "quit"),
        ],
        Focus::Log => &[
            ("Tab", "focus"),
            ("↑↓ PgUp PgDn", "scroll"),
            ("End", "follow"),
            ("q", "quit"),
        ],
        Focus::Prompt => &[
            ("Enter", "send"),
            ("Alt+Enter", "new line"),
            ("Esc", "leave"),
            ("Tab", "focus"),
        ],
    };
    let mut hint_spans = vec![Span::raw(" ")];
    for (keys, action) in hint_pairs {
        hint_spans.push(Span::styled(format!("{keys} "), Style::new().fg(ACCENT)));
        hint_spans.push(Span::styled(format!("{action}  "), Style::new().fg(MUTED)));
    }
    let hints = Line::from(hint_spans).alignment(Alignment::Right);
    let block = pane(pane_title(title, focused), focused).title_bottom(hints);
    // Border, padding and "❯ " on the left; padding, border and the cursor's cell on the right.
    let text_start = 4;
    let room = usize::from(area.width.saturating_sub(text_start + 3));
    let shown = typed_tail(&ui_state.prompt.replace('\n', "↵"), room);
    let shown_width = u16::try_from(shown.chars().count()).unwrap_or(u16::MAX);
    let content = if ui_state.prompt.is_empty() {
        let placeholder = match view.session().phase() {
            Phase::NotStarted => "Describe the change you want, then press Enter",
            _ => "Answers and hints go here",
        };
        Line::from(vec![
            Span::styled("❯ ", Style::new().fg(ACCENT)),
            Span::styled(placeholder, Style::new().fg(FAINT).italic()),
        ])
    } else {
        Line::from(vec![
            Span::styled("❯ ", Style::new().fg(ACCENT)),
            Span::raw(shown),
        ])
    };
    frame.render_widget(Paragraph::new(content).block(block), area);
    if focused {
        frame.set_cursor_position((area.x + text_start + shown_width, area.y + 1));
    }
}

/// The end of what was typed, so the cursor stays in view: `room` characters at most, starting
/// with `…` when the start is cut off.
fn typed_tail(typed: &str, room: usize) -> String {
    let typed_length = typed.chars().count();
    if typed_length <= room {
        return typed.to_owned();
    }
    let kept: String = typed.chars().skip(typed_length + 1 - room.max(1)).collect();
    format!("…{kept}")
}

fn phase_label(view: &SessionView) -> String {
    let phase = view.session().phase();
    let task = phase_task(phase)
        .map(|task| format!(" · Task {}", task + 1))
        .unwrap_or_default();
    let name = match phase {
        Phase::NotStarted => "Not started",
        Phase::Planning { .. } => "Planning",
        Phase::PlanCheckpoint => "Plan Checkpoint",
        Phase::Scaffolding { .. } => "Scaffolding",
        Phase::ScaffoldChecking { .. } => "Scaffold Check",
        Phase::ScaffoldCheckpoint { .. } => "Scaffold Checkpoint",
        Phase::CommittingSkeleton { .. } => "Committing Skeleton",
        Phase::Writing { .. } => "Writing",
        Phase::Committing { .. } => "Committing",
        Phase::TaskChecking { .. } => "Task's Unit Check",
        Phase::Revising { .. } => "Revising",
        Phase::RevisionCheckpoint { .. } => "Revision Checkpoint",
        Phase::CommittingRevision { .. } => "Committing Revision",
        Phase::DiscardingRevision { .. } => "Discarding Revision",
        Phase::EscalationCheckpoint { .. } => "Escalation Checkpoint",
        Phase::Replanning { .. } => "Replanning",
        Phase::ReplanCheckpoint { .. } => "Replan Checkpoint",
        Phase::ResettingTask { .. } => "Resetting Task",
        Phase::CheckingMarkers => "Marker check",
        Phase::MarkerCheckpoint { .. } => "Marker Checkpoint",
        Phase::Done => "Done",
        Phase::Stopped => "Stopped",
    };
    format!("{name}{task}")
}

fn draw_status(frame: &mut Frame, area: Rect, view: &SessionView, status: &StatusInfo) {
    let usage = view.session().usage();
    let gap = || Span::raw("  ");
    let mut cost_spans = Vec::new();
    for tier in [Tier::Planner, Tier::Scaffolder, Tier::Writer] {
        let tier_usage = usage.tier(tier);
        cost_spans.push(badge(tier_name(tier), tier_color(tier)));
        cost_spans.push(Span::styled(
            format!(" ${:.4} ", tier_usage.cost.dollars()),
            Style::new().bold(),
        ));
        cost_spans.push(Span::styled(
            format!(
                "{}↑ {}↓ {}↺ ",
                tokens(tier_usage.input_tokens),
                tokens(tier_usage.output_tokens),
                tokens(tier_usage.cache_read_tokens)
            ),
            Style::new().fg(MUTED),
        ));
        cost_spans.push(gap());
    }

    let total = usage.total().cost.dollars();
    let budget = match view.session().budget_cap() {
        Some(cap) => format!("${total:.4} of ${:.2} cap", cap.dollars()),
        None => format!("${total:.4} · no cap"),
    };
    let mode = match status.clarification_mode {
        ClarificationMode::Ask => "Ask",
        ClarificationMode::Assume => "Assume",
    };
    let info_spans = vec![
        badge(&phase_label(view), YELLOW),
        gap(),
        Span::styled("Total ", Style::new().fg(MUTED)),
        Span::styled(budget, Style::new().bold()),
        gap(),
        Span::styled("Clarification ", Style::new().fg(MUTED)),
        Span::styled(mode, Style::new().bold()),
        gap(),
        Span::styled("Models ", Style::new().fg(MUTED)),
        Span::raw(
            status
                .models
                .iter()
                .map(|model| short_model(model))
                .collect::<Vec<_>>()
                .join(" · "),
        ),
        Span::styled("  ↺ = cache hits", Style::new().fg(FAINT)),
    ];
    let status_bar = Paragraph::new(vec![Line::from(cost_spans), Line::from(info_spans)])
        .style(Style::new().bg(BAR));
    frame.render_widget(status_bar, area);
}

/// `anthropic/claude-opus-5.5` as `opus-5.5`, so the status bar fits.
fn short_model(model: &str) -> &str {
    let name = model.rsplit('/').next().unwrap_or(model);
    name.strip_prefix("claude-").unwrap_or(name)
}

/// The key that confirms each [`Confirmation`]. It differs from the key that asked, so a stray or
/// doubled key press can't stop a Session.
pub fn confirmation_key(confirmation: Confirmation) -> (char, &'static str) {
    match confirmation {
        Confirmation::Quit => ('q', "Ctrl+Q"),
        Confirmation::Cancel => ('x', "Ctrl+X"),
    }
}

fn draw_confirmation(frame: &mut Frame, confirmation: Confirmation, status: &StatusInfo) {
    let (_, keys) = confirmation_key(confirmation);
    let (title, question, action) = match confirmation {
        Confirmation::Quit => (
            "Leave arbor?",
            "Leaving stops the running Session.",
            "leave",
        ),
        Confirmation::Cancel => (
            "Stop the Session?",
            "Stopping ends the running Session.",
            "stop",
        ),
    };
    let branch = status.branch.as_deref().unwrap_or("the Session branch");
    let lines = vec![
        Line::styled(question, Style::new().bold()),
        Line::default(),
        Line::raw(format!(
            "Work already committed stays on {branch}. The model call or Check running now is \
             lost, and the Work Unit being written goes back to its Placeholder."
        )),
        Line::default(),
        Line::raw(
            "Markers stay in the Session branch until the Session is resumed or deleted; \
             the mainline is not touched.",
        ),
        Line::default(),
        Line::from(vec![
            Span::styled(format!("{keys} "), Style::new().fg(RED).bold()),
            Span::styled(format!("{action}   "), Style::new().fg(MUTED)),
            Span::styled("any other key ", Style::new().fg(ACCENT)),
            Span::styled("keep working", Style::new().fg(MUTED)),
        ]),
    ];
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(64);
    let height = 11.min(area.height);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(RED))
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(RED).bold(),
        ))
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(BAR));
    frame.render_widget(ratatui::widgets::Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        popup,
    );
}
