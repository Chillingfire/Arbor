use arbor_config::ClarificationMode;
use arbor_core::{
    FailedAttempt, LeftoverMarker, Plan, Replan, RevisionCause, Task, Tier, Usd, WorkUnit,
    WorkUnitFailure, WorkUnitId,
};
use arbor_session::{Checkpoint, Decision};
use arbor_tui::{KeyOutcome, Modal, SessionView, StatusInfo, UiState, draw};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn plan() -> Plan {
    let task = |title: &str| Task {
        title: title.to_owned(),
        description: format!("{title}."),
    };
    Plan {
        goal: "Add word statistics".to_owned(),
        constraints: vec![
            "Keep the API small".to_owned(),
            "Assumption: words split on whitespace".to_owned(),
        ],
        tasks: vec![task("Add longest_word"), task("Add unique_words")],
    }
}

fn opened(modal: Modal) -> UiState {
    let mut ui_state = UiState::default();
    ui_state.open_modal(modal);
    ui_state
}

fn press(ui_state: &mut UiState, code: KeyCode) -> KeyOutcome {
    press_with(ui_state, code, KeyModifiers::NONE)
}

fn press_with(ui_state: &mut UiState, code: KeyCode, modifiers: KeyModifiers) -> KeyOutcome {
    ui_state.handle_key(KeyEvent::new(code, modifiers), &SessionView::new())
}

fn type_text(ui_state: &mut UiState, typed: &str) {
    for typed_char in typed.chars() {
        press(ui_state, KeyCode::Char(typed_char));
    }
}

fn screen(ui_state: &UiState) -> String {
    screen_with_rows(ui_state, 34)
}

fn screen_with_rows(ui_state: &UiState, rows: u16) -> String {
    let status = StatusInfo {
        models: ["opus".to_owned(), "sonnet".to_owned(), "haiku".to_owned()],
        clarification_mode: ClarificationMode::Ask,
        branch: None,
        activity: None,
    };
    let mut terminal = Terminal::new(TestBackend::new(110, rows)).unwrap();
    terminal
        .draw(|frame| draw(frame, &SessionView::new(), ui_state, &status))
        .unwrap();
    terminal.backend().to_string()
}

#[test]
fn approving_a_plan_as_it_is_or_with_tasks_moved_and_dropped() {
    let mut as_it_is = opened(Modal::checkpoint(Checkpoint::Plan { plan: plan() }));
    assert_eq!(
        press(&mut as_it_is, KeyCode::Char('a')),
        KeyOutcome::Decided(Decision::Approve)
    );
    assert_eq!(as_it_is.modal, None);

    let mut reordered = opened(Modal::checkpoint(Checkpoint::Plan { plan: plan() }));
    press_with(&mut reordered, KeyCode::Down, KeyModifiers::SHIFT);
    press(&mut reordered, KeyCode::Char(' '));
    assert_eq!(
        press(&mut reordered, KeyCode::Enter),
        KeyOutcome::Decided(Decision::ApproveTasks(vec![1]))
    );
}

#[test]
fn a_plan_with_every_task_dropped_cant_be_approved() {
    let mut ui_state = opened(Modal::checkpoint(Checkpoint::Plan { plan: plan() }));
    press(&mut ui_state, KeyCode::Char(' '));
    press(&mut ui_state, KeyCode::Char('j'));
    press(&mut ui_state, KeyCode::Char(' '));

    assert_eq!(
        press(&mut ui_state, KeyCode::Char('a')),
        KeyOutcome::Handled
    );
    assert!(ui_state.modal.is_some());
    assert!(screen(&ui_state).contains("Keep at least one Task to approve the Plan."));
}

#[test]
fn rejecting_takes_typed_feedback_and_esc_goes_back_to_choosing() {
    let mut ui_state = opened(Modal::checkpoint(Checkpoint::Revision {
        task: 0,
        revision: arbor_core::Revision {
            diagnosis: "The spec left out ties".to_owned(),
            work_units: Vec::new(),
            shared_change: None,
        },
    }));
    press(&mut ui_state, KeyCode::Char('r'));
    assert_eq!(press(&mut ui_state, KeyCode::Enter), KeyOutcome::Handled);
    press(&mut ui_state, KeyCode::Esc);
    assert!(ui_state.shown_modal().is_some(), "Esc leaves the feedback");

    press(&mut ui_state, KeyCode::Char('r'));
    type_text(&mut ui_state, "Keep the first word");
    ui_state.paste("\r\non ties");
    assert_eq!(
        press(&mut ui_state, KeyCode::Enter),
        KeyOutcome::Decided(Decision::Reject("Keep the first word\non ties".to_owned()))
    );
}

#[test]
fn esc_hides_the_modal_to_look_around_and_c_brings_it_back() {
    let mut ui_state = opened(Modal::checkpoint(Checkpoint::Plan { plan: plan() }));
    press(&mut ui_state, KeyCode::Esc);
    assert_eq!(ui_state.shown_modal(), None);
    assert!(screen(&ui_state).contains("c back to what the Session waits on"));
    assert_eq!(
        press(&mut ui_state, KeyCode::Char('a')),
        KeyOutcome::Handled,
        "keys go to the panes while the modal is hidden"
    );

    press(&mut ui_state, KeyCode::Char('c'));
    assert!(ui_state.shown_modal().is_some());
    assert_eq!(
        press_with(&mut ui_state, KeyCode::Char('c'), KeyModifiers::CONTROL),
        KeyOutcome::Quit
    );
}

#[test]
fn answers_are_typed_over_several_lines_and_sent_with_enter() {
    let mut ui_state = opened(Modal::questions(
        Tier::Planner,
        "Should ties keep the first word?".to_owned(),
    ));
    assert!(screen(&ui_state).contains("The Planner asks"));
    type_text(&mut ui_state, "Yes");
    press_with(&mut ui_state, KeyCode::Enter, KeyModifiers::ALT);
    type_text(&mut ui_state, "and ignore case");

    assert_eq!(
        press(&mut ui_state, KeyCode::Enter),
        KeyOutcome::Answered("Yes\nand ignore case".to_owned())
    );
    assert_eq!(ui_state.modal, None);
}

#[test]
fn the_checkpoints_show_their_assumptions() {
    let plan_screen = screen(&opened(Modal::checkpoint(Checkpoint::Plan {
        plan: plan(),
    })));
    assert!(plan_screen.contains("Plan Checkpoint"), "{plan_screen}");
    assert!(plan_screen.contains("▲ Assumption: words split on whitespace"));
    assert!(plan_screen.contains("❯ [x] 1. Add longest_word"));

    let work_unit = WorkUnit {
        id: WorkUnitId(3),
        file: "src/lib.rs".to_owned(),
        spec: "Return the longest word. Assumption: ties keep the first.".to_owned(),
        context: Vec::new(),
        acceptance_criteria: Vec::new(),
        test_filter: "longest".to_owned(),
    };
    let scaffold_screen = screen(&opened(Modal::checkpoint(Checkpoint::Scaffold {
        task: 1,
        work_units: vec![work_unit],
    })));
    assert!(scaffold_screen.contains("Scaffold Checkpoint · Task 2"));
    assert!(scaffold_screen.contains("▲ Assumption: ties keep the first."));
}

fn escalation(may_replan: bool, failed_fix: Option<&str>) -> Modal {
    Modal::checkpoint(Checkpoint::Escalation {
        task: 0,
        cause: RevisionCause::WorkUnitFailed(WorkUnitFailure {
            id: WorkUnitId(3),
            attempts: vec![FailedAttempt {
                reply: "None".to_owned(),
                error: "test longest_word_tests::ties ... FAILED".to_owned(),
            }],
            missing_response: None,
        }),
        may_replan,
        failed_fix: failed_fix.map(str::to_owned),
    })
}

#[test]
fn an_escalation_is_fixed_by_hand_replanned_or_stopped_but_never_approved() {
    for (key, decision) in [
        ('f', Decision::Fixed),
        ('p', Decision::Replan),
        ('s', Decision::Stop),
    ] {
        let mut ui_state = opened(escalation(true, None));
        assert_eq!(
            press(&mut ui_state, KeyCode::Char(key)),
            KeyOutcome::Decided(decision)
        );
    }
    let mut ui_state = opened(escalation(true, None));
    for key in [KeyCode::Enter, KeyCode::Char('a'), KeyCode::Char('r')] {
        assert_eq!(press(&mut ui_state, key), KeyOutcome::Handled);
    }
    let escalation_screen = screen(&ui_state);
    assert!(escalation_screen.contains("Escalation · Task 1"));
    assert!(escalation_screen.contains("WU-3"));
    assert!(escalation_screen.contains("longest_word_tests::ties ... FAILED"));
}

#[test]
fn a_task_replanned_once_cant_go_to_the_planner_again() {
    let mut ui_state = opened(escalation(false, Some("error[E0308]: mismatched types")));

    assert_eq!(
        press(&mut ui_state, KeyCode::Char('p')),
        KeyOutcome::Handled
    );
    let escalation_screen = screen(&ui_state);
    assert!(escalation_screen.contains("can't go to the Planner again"));
    assert!(escalation_screen.contains("Your fix didn't pass the Checks:"));
    assert!(escalation_screen.contains("error[E0308]: mismatched types"));
}

#[test]
fn a_replan_is_approved_like_a_plan_with_tasks_dropped() {
    let replan = Replan {
        diagnosis: "The Task needed a tokenizer first".to_owned(),
        tasks: plan().tasks,
    };
    let mut ui_state = opened(Modal::checkpoint(Checkpoint::Replan { task: 1, replan }));
    press(&mut ui_state, KeyCode::Char(' '));

    let replan_screen = screen(&ui_state);
    assert!(replan_screen.contains("Replan Checkpoint · Task 2"));
    assert!(replan_screen.contains("The Task needed a tokenizer first"));
    assert_eq!(
        press(&mut ui_state, KeyCode::Char('a')),
        KeyOutcome::Decided(Decision::ApproveTasks(vec![1]))
    );
}

fn budget() -> Modal {
    Modal::checkpoint(Checkpoint::Budget {
        cap: Usd::from_dollars(1.0),
        spent: Usd::from_dollars(1.02),
    })
}

#[test]
fn the_budget_cap_is_raised_to_the_typed_amount_once_it_is_above_the_spend() {
    let mut ui_state = opened(budget());
    assert!(screen(&ui_state).contains("New cap  $2.00"));
    for _ in 0..4 {
        press(&mut ui_state, KeyCode::Backspace);
    }
    type_text(&mut ui_state, "1.01");
    assert_eq!(press(&mut ui_state, KeyCode::Enter), KeyOutcome::Handled);
    assert!(screen(&ui_state).contains("Type an amount above $1.0200."));

    press(&mut ui_state, KeyCode::Backspace);
    type_text(&mut ui_state, "5");
    assert_eq!(
        press(&mut ui_state, KeyCode::Enter),
        KeyOutcome::Decided(Decision::SetBudgetCap(Some(Usd::from_dollars(1.05))))
    );
}

#[test]
fn the_budget_checkpoint_also_lifts_the_cap_or_stops() {
    let mut uncapped = opened(budget());
    assert_eq!(
        press(&mut uncapped, KeyCode::Char('u')),
        KeyOutcome::Decided(Decision::SetBudgetCap(None))
    );
    let mut stopped = opened(budget());
    assert_eq!(
        press(&mut stopped, KeyCode::Char('s')),
        KeyOutcome::Decided(Decision::Stop)
    );
}

#[test]
fn leftover_markers_are_listed_and_approved_removed_or_stopped_at() {
    let markers = Modal::checkpoint(Checkpoint::Markers {
        remaining: vec![LeftoverMarker {
            file: "src/lib.rs".to_owned(),
            line: 12,
            text: "// ARBOR:BEGIN WU-4".to_owned(),
        }],
    });
    let marker_screen = screen(&opened(markers.clone()));
    assert!(marker_screen.contains("Marker Checkpoint"));
    assert!(marker_screen.contains("src/lib.rs:12  // ARBOR:BEGIN WU-4"));
    for (key, decision) in [
        ('a', Decision::Approve),
        ('f', Decision::Fixed),
        ('s', Decision::Stop),
    ] {
        assert_eq!(
            press(&mut opened(markers.clone()), KeyCode::Char(key)),
            KeyOutcome::Decided(decision)
        );
    }
}

#[test]
fn the_first_digit_of_a_new_cap_replaces_the_suggested_one() {
    let mut ui_state = opened(budget());
    type_text(&mut ui_state, "25");

    assert!(screen(&ui_state).contains("New cap  $25"));
    assert_eq!(
        press(&mut ui_state, KeyCode::Enter),
        KeyOutcome::Decided(Decision::SetBudgetCap(Some(Usd::from_dollars(25.0))))
    );
}

fn long_plan() -> Plan {
    let mut plan = plan();
    plan.constraints = (1..=6)
        .map(|number| {
            format!(
                "Constraint {number}: {}",
                "keeps the change small ".repeat(8)
            )
        })
        .collect();
    plan.tasks = (1..=12)
        .map(|number| Task {
            title: format!("Task number {number}"),
            description: String::new(),
        })
        .collect();
    plan
}

#[test]
fn a_plan_taller_than_the_terminal_keeps_the_selected_task_in_view() {
    let mut ui_state = opened(Modal::checkpoint(Checkpoint::Plan { plan: long_plan() }));
    assert!(screen(&ui_state).contains("Task number 1 "));
    assert!(!screen(&ui_state).contains("Task number 12"));

    for _ in 0..11 {
        press(&mut ui_state, KeyCode::Char('j'));
    }
    assert!(screen(&ui_state).contains("Task number 12"));

    for _ in 0..11 {
        press(&mut ui_state, KeyCode::Char('k'));
    }
    assert!(screen(&ui_state).contains("Task number 1 "));
}

#[test]
fn page_keys_scroll_an_escalation_whose_options_are_below_the_terminal() {
    let failed_fix = (1..=8)
        .map(|number| format!("error number {number}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut ui_state = opened(escalation(true, Some(&failed_fix)));
    assert!(!screen_with_rows(&ui_state, 16).contains("Stop here"));
    assert!(screen_with_rows(&ui_state, 16).contains("PgUp/PgDn"));

    for _ in 0..4 {
        press(&mut ui_state, KeyCode::PageDown);
    }
    assert!(screen_with_rows(&ui_state, 16).contains("Stop here"));

    for _ in 0..4 {
        press(&mut ui_state, KeyCode::PageUp);
    }
    assert!(!screen_with_rows(&ui_state, 16).contains("Stop here"));
}

#[test]
fn long_feedback_scrolls_so_what_is_typed_stays_visible() {
    let mut ui_state = opened(Modal::checkpoint(Checkpoint::Plan { plan: long_plan() }));
    press(&mut ui_state, KeyCode::Char('r'));
    type_text(&mut ui_state, "too many Tasks here");

    assert!(screen_with_rows(&ui_state, 16).contains("too many Tasks here"));
}
