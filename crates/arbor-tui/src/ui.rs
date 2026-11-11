//! Draws the provisional layout (grilling log Round 2 decision 10): the Plan tree on the left,
//! the detail pane on the right, the event log below them, and the status bar at the bottom.

use arbor_config::ClarificationMode;
use arbor_core::{Phase, Tier};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Paragraph, Wrap};

use crate::view::{SessionView, Status, phase_task, tokens};

/// A row of the Plan tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeNode {
    Plan,
    Task(usize),
    WorkUnit { task: usize, unit: usize },
}

/// The tree's rows, top to bottom.
pub fn tree_nodes(view: &SessionView) -> Vec<TreeNode> {
    if view.plan().is_none() {
        return Vec::new();
    }
    let mut nodes = vec![TreeNode::Plan];
    for (task, task_view) in view.tasks().iter().enumerate() {
        nodes.push(TreeNode::Task(task));
        nodes.extend((0..task_view.work_units.len()).map(|unit| TreeNode::WorkUnit { task, unit }));
    }
    nodes
}

/// What the status bar shows besides the Session's own numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusInfo {
    /// The model filling each Tier: Planner, Scaffolder, Writer.
    pub models: [String; 3],
    pub clarification_mode: ClarificationMode,
}

pub fn draw(frame: &mut Frame, view: &SessionView, selected: usize, status: &StatusInfo) {
    let [main_area, log_area, status_area] = Layout::vertical([
        Constraint::Min(8),
        Constraint::Length(8),
        Constraint::Length(2),
    ])
    .areas(frame.area());
    let [tree_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
            .areas(main_area);

    let nodes = tree_nodes(view);
    let selected_node = nodes.get(selected).copied();
    draw_tree(frame, tree_area, view, &nodes, selected);
    draw_detail(frame, detail_area, view, selected_node);
    draw_log(frame, log_area, view);
    draw_status(frame, status_area, view, status);
}

fn pane(title: &str) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::DarkGray))
        .title(Span::styled(format!(" {title} "), Style::new().bold()))
}

fn status_icon(status: Status) -> Span<'static> {
    match status {
        Status::Pending => Span::styled("○", Style::new().fg(Color::DarkGray)),
        Status::Active => Span::styled("●", Style::new().fg(Color::Yellow)),
        Status::Done => Span::styled("✓", Style::new().fg(Color::Green)),
        Status::Failed => Span::styled("✗", Style::new().fg(Color::Red)),
    }
}

fn draw_tree(
    frame: &mut Frame,
    area: Rect,
    view: &SessionView,
    nodes: &[TreeNode],
    selected: usize,
) {
    let block = pane("Plan");
    let Some(plan) = view.plan() else {
        let waiting = Paragraph::new("No Plan yet.".dark_gray()).block(block);
        frame.render_widget(waiting, area);
        return;
    };
    let items: Vec<ListItem> = nodes
        .iter()
        .map(|node| match *node {
            TreeNode::Plan => ListItem::new(Line::from(vec![
                Span::styled("▾ ", Style::new().fg(Color::DarkGray)),
                Span::raw(plan.goal.clone()).bold(),
            ])),
            TreeNode::Task(task) => {
                let task_view = &view.tasks()[task];
                ListItem::new(Line::from(vec![
                    Span::raw("  "),
                    status_icon(task_view.status),
                    Span::raw(format!(" {}. {}", task + 1, task_view.task.title)),
                ]))
            }
            TreeNode::WorkUnit { task, unit } => {
                let work_unit_view = &view.tasks()[task].work_units[unit];
                let work_unit = &work_unit_view.work_unit;
                ListItem::new(Line::from(vec![
                    Span::raw("      "),
                    status_icon(work_unit_view.status),
                    Span::raw(format!(" {} ", work_unit.id)),
                    Span::styled(work_unit.file.clone(), Style::new().fg(Color::DarkGray)),
                ]))
            }
        })
        .collect();
    let list = List::new(items).block(block).highlight_style(
        Style::new()
            .bg(Color::Rgb(40, 44, 52))
            .add_modifier(Modifier::BOLD),
    );
    let mut list_state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn heading(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_owned(),
        Style::new().fg(Color::Cyan).bold(),
    ))
}

fn draw_detail(frame: &mut Frame, area: Rect, view: &SessionView, node: Option<TreeNode>) {
    let mut lines: Vec<Line> = Vec::new();
    let title = match node {
        None => {
            match view.session().request() {
                Some(request) => {
                    lines.push(heading("Request"));
                    lines.extend(request.text.lines().map(|line| Line::raw(line.to_owned())));
                }
                None => lines.push(Line::raw("No Session yet.").dark_gray()),
            }
            "Detail".to_owned()
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
                    let marker = if constraint.starts_with(arbor_core::ASSUMPTION_PREFIX) {
                        Span::styled("• ", Style::new().fg(Color::Yellow))
                    } else {
                        Span::raw("• ")
                    };
                    Line::from(vec![marker, Span::raw(constraint.clone())])
                }));
            }
            "Plan".to_owned()
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
            format!("Task {}: {}", task + 1, task_view.task.title)
        }
        Some(TreeNode::WorkUnit { task, unit }) => {
            let work_unit = &view.tasks()[task].work_units[unit].work_unit;
            lines.push(Line::from(vec![
                Span::styled("File  ", Style::new().fg(Color::DarkGray)),
                Span::raw(work_unit.file.clone()),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Tests ", Style::new().fg(Color::DarkGray)),
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
                lines.extend(
                    work_unit
                        .acceptance_criteria
                        .iter()
                        .map(|criterion| Line::raw(format!("• {criterion}"))),
                );
            }
            lines.push(Line::default());
            lines.push(heading("Context Pack"));
            lines.extend(work_unit.context.iter().map(|snippet| {
                Line::raw(format!(
                    "• {} ({:?}, {} lines)",
                    snippet.path,
                    snippet.priority,
                    snippet.text.lines().count()
                ))
            }));
            format!("{}: spec", work_unit.id)
        }
    };
    let detail = Paragraph::new(lines)
        .block(pane(&title))
        .wrap(Wrap { trim: false });
    frame.render_widget(detail, area);
}

fn draw_log(frame: &mut Frame, area: Rect, view: &SessionView) {
    let visible_lines = usize::from(area.height.saturating_sub(2));
    let log = view.log();
    let newest: Vec<Line> = log[log.len().saturating_sub(visible_lines)..]
        .iter()
        .map(|entry| Line::raw(entry.clone()))
        .collect();
    frame.render_widget(Paragraph::new(newest).block(pane("Event log")), area);
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
    let separator = || Span::styled(" │ ", Style::new().fg(Color::DarkGray));
    let mut cost_spans = Vec::new();
    for (tier, name) in [
        (Tier::Planner, "Planner"),
        (Tier::Scaffolder, "Scaffolder"),
        (Tier::Writer, "Writer"),
    ] {
        let tier_usage = usage.tier(tier);
        cost_spans.push(Span::styled(format!("{name} "), Style::new().bold()));
        cost_spans.push(Span::raw(format!(
            "${:.4} {}↑ {}↓",
            tier_usage.cost.dollars(),
            tokens(tier_usage.input_tokens),
            tokens(tier_usage.output_tokens)
        )));
        cost_spans.push(Span::styled(
            format!(" {} cached", tokens(tier_usage.cache_read_tokens)),
            Style::new().fg(Color::DarkGray),
        ));
        cost_spans.push(separator());
    }
    cost_spans.pop();
    let total = usage.total().cost.dollars();
    let budget = match view.session().budget_cap() {
        Some(cap) => format!("Total ${total:.4} of ${:.2} cap", cap.dollars()),
        None => format!("Total ${total:.4}, no cap"),
    };

    let mode = match status.clarification_mode {
        ClarificationMode::Ask => "Ask",
        ClarificationMode::Assume => "Assume",
    };
    let [planner_model, scaffolder_model, writer_model] = &status.models;
    let info_spans = vec![
        Span::styled(phase_label(view), Style::new().fg(Color::Yellow).bold()),
        separator(),
        Span::styled(budget, Style::new().bold()),
        separator(),
        Span::styled("Models ", Style::new().fg(Color::DarkGray)),
        Span::raw(format!(
            "P {planner_model} · S {scaffolder_model} · W {writer_model}"
        )),
        separator(),
        Span::styled("Mode ", Style::new().fg(Color::DarkGray)),
        Span::raw(mode),
    ];
    let status_bar = Paragraph::new(vec![Line::from(cost_spans), Line::from(info_spans)])
        .style(Style::new().bg(Color::Rgb(30, 33, 40)));
    frame.render_widget(status_bar, area);
}
