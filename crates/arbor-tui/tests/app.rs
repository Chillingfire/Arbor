use arbor_config::ClarificationMode;
use arbor_core::{Event, Plan, Request, Task, Tier, Usage, WorkUnit, WorkUnitId};
use arbor_session::{Activity, AutopilotUpdate};
use arbor_tiers::{Attempt, AttemptOutcome, Failure, FailureReason};
use arbor_tui::{App, ReplyOutcome, StatusInfo};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn app() -> App {
    App::new(StatusInfo {
        models: [
            "anthropic/claude-opus-5.5".to_owned(),
            "anthropic/claude-sonnet-5.5".to_owned(),
            "anthropic/claude-haiku-5.5".to_owned(),
        ],
        clarification_mode: ClarificationMode::Assume,
        branch: Some("arbor/add-x".to_owned()),
        activity: None,
    })
}

fn screen(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    terminal.backend().to_string()
}

fn started_session(app: &mut App) {
    for event in [
        Event::SessionStarted {
            request: Request {
                text: "Add x".to_owned(),
                issue: None,
            },
        },
        Event::PlanSubmitted {
            plan: Plan {
                goal: "Add x".to_owned(),
                constraints: Vec::new(),
                tasks: vec![Task {
                    title: "Add x".to_owned(),
                    description: "Add x".to_owned(),
                }],
            },
        },
        Event::PlanApproved {
            task_order: vec![0],
        },
        Event::WorkUnitsSubmitted {
            work_units: vec![WorkUnit {
                id: WorkUnitId(1),
                file: "src/lib.rs".to_owned(),
                spec: "Return x".to_owned(),
                context: Vec::new(),
                acceptance_criteria: Vec::new(),
                test_filter: "x".to_owned(),
            }],
        },
        Event::ScaffoldChecksPassed,
        Event::ScaffoldApproved,
        Event::SkeletonCommitted { commit: None },
    ] {
        app.apply_update(AutopilotUpdate::Event(event));
    }
}

#[test]
fn a_running_model_call_shows_a_spinner_with_tier_model_and_label() {
    let mut app = app();
    started_session(&mut app);

    app.apply_update(AutopilotUpdate::Working(Activity {
        tier: Some(Tier::Writer),
        model: Some("anthropic/claude-haiku-5.5".to_owned()),
        label: "Writing WU-1, attempt 1".to_owned(),
    }));

    assert!(app.is_working());
    let shown = screen(&app);
    assert!(
        shown.contains("Writer haiku-5.5 · Writing WU-1, attempt 1 0s"),
        "{shown}"
    );
    assert!(shown.contains("arbor/add-x"), "{shown}");

    app.apply_update(AutopilotUpdate::Event(Event::WorkUnitPassed {
        id: WorkUnitId(1),
    }));
    assert!(!app.is_working());
}

#[test]
fn a_long_goal_stops_short_of_the_spinner() {
    let mut app = app();
    app.apply_update(AutopilotUpdate::Event(Event::SessionStarted {
        request: Request {
            text: ["word"; 40].join(" "),
            issue: None,
        },
    }));

    app.apply_update(AutopilotUpdate::Working(Activity {
        tier: Some(Tier::Planner),
        model: None,
        label: "Planning, turn 1".to_owned(),
    }));

    let header = screen(&app).lines().next().unwrap().to_owned();
    assert!(header.contains("  ⠋ Planner"), "{header}");
}

#[test]
fn writer_attempts_and_diffs_fill_the_work_units_tabs() {
    let mut app = app();
    started_session(&mut app);
    let attempt = |outcome| Attempt {
        model: "anthropic/claude-haiku-5.5".to_owned(),
        usage: Usage::default(),
        outcome,
    };

    app.apply_update(AutopilotUpdate::WriterAttempt {
        work_unit: WorkUnitId(1),
        attempt: attempt(AttemptOutcome::Failed(Failure {
            reply: "```rust\n0\n```".to_owned(),
            reason: FailureReason::ChecksFailed {
                output: "expected 1".to_owned(),
            },
        })),
    });
    app.apply_update(AutopilotUpdate::Committed {
        work_unit: WorkUnitId(1),
        diff: "+1".to_owned(),
    });

    let work_unit_view = &app.view.tasks()[0].work_units[0];
    assert_eq!(work_unit_view.transcript[0].reply, "```rust\n0\n```");
    assert!(matches!(
        &work_unit_view.transcript[0].outcome,
        ReplyOutcome::Failed(reason) if reason.contains("expected 1")
    ));
    assert_eq!(work_unit_view.diff.as_deref(), Some("+1"));
}
