use arbor_config::ClarificationMode;
use arbor_core::{Event, Plan, Request, Task, Tier, Usage, Usd, WorkUnit, WorkUnitId};
use arbor_tui::{SessionView, Status, StatusInfo, TreeNode, draw, tree_nodes};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn work_unit(id: u32) -> WorkUnit {
    WorkUnit {
        id: WorkUnitId(id),
        file: "src/lib.rs".to_owned(),
        spec: format!("Spec of unit {id}"),
        context: Vec::new(),
        acceptance_criteria: vec![format!("criterion {id}")],
        test_filter: format!("tests_{id}"),
    }
}

fn task(title: &str) -> Task {
    Task {
        title: title.to_owned(),
        description: format!("Description of {title}"),
    }
}

/// Task 1 done with WU-1; in Task 2, WU-2 passed and waits for its commit.
fn events() -> Vec<Event> {
    vec![
        Event::SessionStarted {
            request: Request {
                text: "Add two helpers".to_owned(),
                issue: None,
            },
        },
        Event::PlanSubmitted {
            plan: Plan {
                goal: "Two helpers".to_owned(),
                constraints: Vec::new(),
                tasks: vec![task("First"), task("Second")],
            },
        },
        Event::PlanApproved {
            task_order: vec![0, 1],
        },
        Event::WorkUnitsSubmitted {
            work_units: vec![work_unit(1)],
        },
        Event::ScaffoldChecksPassed,
        Event::ScaffoldApproved,
        Event::SkeletonCommitted { commit: None },
        Event::WorkUnitPassed { id: WorkUnitId(1) },
        Event::UsageRecorded {
            tier: Tier::Writer,
            usage: Usage {
                input_tokens: 1_500,
                output_tokens: 300,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost: Usd::from_dollars(0.002),
            },
        },
        Event::WorkUnitCommitted {
            id: WorkUnitId(1),
            commit: "0123456789".to_owned(),
        },
        Event::TaskChecksPassed,
        Event::WorkUnitsSubmitted {
            work_units: vec![work_unit(2), work_unit(3)],
        },
        Event::ScaffoldChecksPassed,
        Event::ScaffoldApproved,
        Event::SkeletonCommitted { commit: None },
        Event::WorkUnitPassed { id: WorkUnitId(2) },
    ]
}

#[test]
fn the_tree_keeps_finished_tasks_and_marks_the_unit_being_written() {
    let view = SessionView::from_events(&events()).unwrap();

    let task_statuses: Vec<Status> = view
        .tasks()
        .iter()
        .map(|task_view| task_view.status)
        .collect();
    assert_eq!(task_statuses, [Status::Done, Status::Active]);
    let unit_statuses: Vec<(u32, Status)> = view
        .tasks()
        .iter()
        .flat_map(|task_view| &task_view.work_units)
        .map(|unit_view| (unit_view.work_unit.id.0, unit_view.status))
        .collect();
    assert_eq!(
        unit_statuses,
        [(1, Status::Done), (2, Status::Done), (3, Status::Pending)]
    );
    assert_eq!(
        tree_nodes(&view),
        [
            TreeNode::Plan,
            TreeNode::Task(0),
            TreeNode::WorkUnit { task: 0, unit: 0 },
            TreeNode::Task(1),
            TreeNode::WorkUnit { task: 1, unit: 0 },
            TreeNode::WorkUnit { task: 1, unit: 1 },
        ]
    );
}

#[test]
fn the_log_describes_each_event_in_one_line() {
    let view = SessionView::from_events(&events()).unwrap();

    assert_eq!(view.log().len(), events().len());
    assert!(view.log().contains(&"WU-1 committed 0123456".to_owned()));
    assert!(
        view.log()
            .contains(&"Writer call: 1.5k in / 300 out tokens, $0.0020".to_owned()),
        "{:?}",
        view.log()
    );
}

fn rendered(view: &SessionView, selected: usize) -> String {
    let status = StatusInfo {
        models: ["opus".to_owned(), "sonnet".to_owned(), "haiku".to_owned()],
        clarification_mode: ClarificationMode::Assume,
    };
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal
        .draw(|frame| draw(frame, view, selected, &status))
        .unwrap();
    terminal.backend().to_string()
}

#[test]
fn the_layout_shows_tree_detail_log_and_status_bar() {
    let view = SessionView::from_events(&events()).unwrap();

    let screen = rendered(&view, 5);

    assert!(screen.contains("2. Second"), "{screen}");
    assert!(screen.contains("WU-3: spec"), "{screen}");
    assert!(screen.contains("Spec of unit 3"), "{screen}");
    assert!(screen.contains("Event log"), "{screen}");
    assert!(screen.contains("Writer $0.0020 1.5k↑ 300↓"), "{screen}");
    assert!(screen.contains("Committing · Task 2"), "{screen}");
    assert!(screen.contains("Mode Assume"), "{screen}");
}

#[test]
fn before_a_plan_the_tree_says_so_and_the_detail_shows_the_request() {
    let view = SessionView::from_events(&events()[..1]).unwrap();

    let screen = rendered(&view, 0);

    assert!(screen.contains("No Plan yet."), "{screen}");
    assert!(screen.contains("Add two helpers"), "{screen}");
}
