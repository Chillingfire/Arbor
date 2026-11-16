use arbor_config::ClarificationMode;
use arbor_core::{Event, Plan, Request, Task, Tier, Usage, Usd, WorkUnit, WorkUnitId};
use arbor_tui::{
    Confirmation, Focus, KeyOutcome, LogEntry, ReplyOutcome, SessionView, Status, StatusInfo, Tone,
    TreeNode, UiState, WriterReplyRecord, describe, draw, tree_nodes,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
        Event::WorkUnitPassed {
            id: WorkUnitId(1),
            model: None,
        },
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
        Event::WorkUnitPassed {
            id: WorkUnitId(2),
            model: None,
        },
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
        tree_nodes(&view, &Default::default()),
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
fn the_log_describes_each_event_in_one_line_with_its_tier_and_tone() {
    let view = SessionView::from_events(&events()).unwrap();

    assert_eq!(view.log().len(), events().len());
    assert!(view.log().contains(&LogEntry {
        text: "WU-1 committed 0123456".to_owned(),
        tier: None,
        tone: Tone::Success,
    }));
    assert!(
        view.log().contains(&LogEntry {
            text: "Writer call: 1.5k in / 300 out tokens, $0.0020".to_owned(),
            tier: Some(Tier::Writer),
            tone: Tone::ModelCall,
        }),
        "{:?}",
        view.log()
    );
}

#[test]
fn a_broken_region_is_described_by_its_problem_not_as_failed_attempts() {
    let broken = Event::WorkUnitFailed {
        failure: arbor_core::WorkUnitFailure {
            id: WorkUnitId(4),
            attempts: Vec::new(),
            missing_response: None,
            broken_region: Some(arbor_core::BrokenRegion {
                problem: "WU-4: ARBOR:END on line 5 has no ARBOR:BEGIN".to_owned(),
                file_text: String::new(),
            }),
        },
    };

    assert_eq!(
        describe(&broken),
        "WU-4's Region is broken: WU-4: ARBOR:END on line 5 has no ARBOR:BEGIN"
    );
}

fn rendered(view: &SessionView, selected: TreeNode) -> String {
    let status = StatusInfo {
        models: ["opus".to_owned(), "sonnet".to_owned(), "haiku".to_owned()],
        clarification_mode: ClarificationMode::Assume,
        branch: Some("arbor/two-helpers".to_owned()),
        activity: None,
    };
    let ui_state = UiState {
        selected,
        ..UiState::default()
    };
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal
        .draw(|frame| draw(frame, view, &ui_state, &status))
        .unwrap();
    terminal.backend().to_string()
}

#[test]
fn the_layout_shows_tree_detail_log_and_status_bar() {
    let view = SessionView::from_events(&events()).unwrap();

    let screen = rendered(&view, TreeNode::WorkUnit { task: 1, unit: 1 });

    assert!(screen.contains("2. Second"), "{screen}");
    assert!(screen.contains("WU-3 Spec · Transcript · Diff"), "{screen}");
    assert!(screen.contains("Spec of unit 3"), "{screen}");
    assert!(screen.contains("Event log"), "{screen}");
    assert!(screen.contains(" Writer  $0.0020 1.5k↑ 300↓"), "{screen}");
    assert!(screen.contains("Committing · Task 2"), "{screen}");
    assert!(screen.contains("Clarification Assume"), "{screen}");
}

#[test]
fn before_a_plan_the_tree_says_so_and_the_detail_shows_the_request() {
    let view = SessionView::from_events(&events()[..1]).unwrap();

    let screen = rendered(&view, TreeNode::Plan);

    assert!(screen.contains("No Plan yet."), "{screen}");
    assert!(screen.contains("Add two helpers"), "{screen}");
}

#[test]
fn the_event_log_scrolls_back_and_says_how_much_is_below() {
    let view = SessionView::from_events(&events()).unwrap();
    let status = StatusInfo {
        models: ["opus".to_owned(), "sonnet".to_owned(), "haiku".to_owned()],
        clarification_mode: ClarificationMode::Ask,
        branch: None,
        activity: None,
    };
    let mut ui_state = UiState {
        focus: Focus::Log,
        ..UiState::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui_state.handle_key(key(KeyCode::Home), &view);
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal
        .draw(|frame| draw(frame, &view, &ui_state, &status))
        .unwrap();
    let screen = terminal.backend().to_string();

    assert!(
        screen.contains("   1 Session    · Session started"),
        "{screen}"
    );
    assert!(screen.contains("newer below"), "{screen}");

    ui_state.handle_key(key(KeyCode::End), &view);
    assert_eq!(ui_state.log_top, None);
}

#[test]
fn typing_in_the_prompt_submits_on_enter() {
    let view = SessionView::new();
    let mut ui_state = UiState::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

    ui_state.handle_key(key(KeyCode::Char('/')), &view);
    for typed in "Add x".chars() {
        ui_state.handle_key(key(KeyCode::Char(typed)), &view);
    }
    let outcome = ui_state.handle_key(key(KeyCode::Enter), &view);

    assert_eq!(outcome, KeyOutcome::Submitted("Add x".to_owned()));
    assert_eq!(ui_state.prompt, "");
}

#[test]
fn collapsing_a_task_hides_its_units_and_selects_the_task() {
    let view = SessionView::from_events(&events()).unwrap();
    let mut ui_state = UiState {
        selected: TreeNode::WorkUnit { task: 1, unit: 0 },
        ..UiState::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

    ui_state.handle_key(key(KeyCode::Left), &view);

    assert_eq!(ui_state.selected, TreeNode::Task(1));
    assert_eq!(
        tree_nodes(&view, &ui_state.collapsed),
        [
            TreeNode::Plan,
            TreeNode::Task(0),
            TreeNode::WorkUnit { task: 0, unit: 0 },
            TreeNode::Task(1),
        ]
    );
    assert!(rendered_with(&view, &ui_state).contains("2. Second  ▸ 2 hidden"));

    ui_state.handle_key(key(KeyCode::Right), &view);
    assert_eq!(tree_nodes(&view, &ui_state.collapsed).len(), 6);
}

#[test]
fn the_transcript_tab_shows_failed_replies_from_the_log_and_the_diff_tab_the_commit() {
    let mut failing = events();
    failing.truncate(failing.len() - 1);
    failing.push(Event::WorkUnitFailed {
        failure: arbor_core::WorkUnitFailure {
            id: WorkUnitId(2),
            attempts: vec![arbor_core::FailedAttempt {
                reply: "```rust\nreturn 1;\n```".to_owned(),
                error: "expected 2".to_owned(),
            }],
            missing_response: None,
            broken_region: None,
        },
    });
    let view = SessionView::from_events(&failing).unwrap();
    let mut ui_state = UiState {
        selected: TreeNode::WorkUnit { task: 1, unit: 0 },
        focus: Focus::Detail,
        ..UiState::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

    ui_state.handle_key(key(KeyCode::Right), &view);
    let transcript = rendered_with(&view, &ui_state);
    assert!(transcript.contains("✘ Attempt 1 failed"), "{transcript}");
    assert!(transcript.contains("│ return 1;"), "{transcript}");
    assert!(!transcript.contains("```"), "{transcript}");
    assert!(transcript.contains("expected 2"), "{transcript}");

    let mut committed = SessionView::from_events(&events()).unwrap();
    committed.set_diff(WorkUnitId(1), "+    added line".to_owned());
    ui_state.selected = TreeNode::WorkUnit { task: 0, unit: 0 };
    ui_state.handle_key(key(KeyCode::Right), &committed);
    let diff = rendered_with(&committed, &ui_state);
    assert!(diff.contains("Commit 0123456"), "{diff}");
    assert!(diff.contains("+    added line"), "{diff}");
}

fn rendered_with(view: &SessionView, ui_state: &UiState) -> String {
    let status = StatusInfo {
        models: ["opus".to_owned(), "sonnet".to_owned(), "haiku".to_owned()],
        clarification_mode: ClarificationMode::Ask,
        branch: None,
        activity: None,
    };
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal
        .draw(|frame| draw(frame, view, ui_state, &status))
        .unwrap();
    terminal.backend().to_string()
}

#[test]
fn the_selection_stays_on_its_task_when_work_units_arrive_above_it() {
    let mut view = SessionView::from_events(&events()[..3]).unwrap();
    let mut ui_state = UiState {
        selected: TreeNode::Task(1),
        ..UiState::default()
    };

    view.apply(&events()[3]).unwrap();
    assert_eq!(ui_state.selected, TreeNode::Task(1));
    let screen = rendered_with(&view, &ui_state);
    assert!(screen.contains("Description of Second"), "{screen}");

    ui_state.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &view);
    assert_eq!(ui_state.selected, TreeNode::WorkUnit { task: 0, unit: 0 });
}

#[test]
fn a_failed_writer_attempt_goes_in_the_log_with_its_number() {
    let mut view = SessionView::from_events(&events()).unwrap();

    view.record_writer_reply(
        WorkUnitId(3),
        WriterReplyRecord {
            reply: "fn x() {}".to_owned(),
            outcome: ReplyOutcome::Failed("the Checks failed:\nerror[E0308]".to_owned()),
        },
    );

    assert_eq!(
        view.log().last(),
        Some(&LogEntry {
            text: "WU-3 attempt 1 failed: the Checks failed".to_owned(),
            tier: Some(Tier::Writer),
            tone: Tone::Failure,
        })
    );
}

#[test]
fn a_long_prompt_shows_its_end() {
    let view = SessionView::new();
    let ui_state = UiState {
        focus: Focus::Prompt,
        prompt: format!("start{} end", " middle".repeat(30)),
        ..UiState::default()
    };

    let screen = rendered_with(&view, &ui_state);

    assert!(screen.contains("❯ …"), "{screen}");
    assert!(screen.contains("middle end  │"), "{screen}");
}

#[test]
fn a_scrolled_back_log_holds_still_as_events_arrive() {
    let events = events();
    let mut view = SessionView::from_events(&events[..events.len() - 1]).unwrap();
    let mut ui_state = UiState {
        focus: Focus::Log,
        ..UiState::default()
    };
    rendered_with(&view, &ui_state);
    ui_state.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE), &view);
    let top_line = |screen: &str| {
        screen
            .lines()
            .find(|line| line.contains("Session    · Session started"))
            .map(str::to_owned)
    };
    let before = top_line(&rendered_with(&view, &ui_state));

    view.apply(events.last().unwrap()).unwrap();
    view.push_note("A note", Tone::Progress);

    assert!(before.is_some());
    assert_eq!(top_line(&rendered_with(&view, &ui_state)), before);
}

#[test]
fn a_paste_lands_in_the_prompt_whole_and_alt_enter_adds_a_line() {
    let view = SessionView::from_events(&events()).unwrap();
    let mut ui_state = UiState::default();

    ui_state.paste("Add x\r\nand quit with q");
    ui_state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT), &view);
    ui_state.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE), &view);
    let outcome = ui_state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &view);

    assert_eq!(
        outcome,
        KeyOutcome::Submitted("Add x\nand quit with q\ny".to_owned())
    );
}

#[test]
fn only_the_confirming_key_confirms_and_any_other_dismisses() {
    let view = SessionView::from_events(&events()).unwrap();
    let mut ui_state = UiState {
        confirming: Some(Confirmation::Quit),
        ..UiState::default()
    };
    assert!(rendered_with(&view, &ui_state).contains("Leave arbor?"));

    let doubled = ui_state.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE), &view);
    assert_eq!(doubled, KeyOutcome::Handled);
    assert_eq!(ui_state.confirming, None);

    ui_state.confirming = Some(Confirmation::Quit);
    let confirmed = ui_state.handle_key(
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
        &view,
    );
    assert_eq!(confirmed, KeyOutcome::Confirmed(Confirmation::Quit));
}
