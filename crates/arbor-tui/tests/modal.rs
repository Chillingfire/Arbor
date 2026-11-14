use arbor_config::ClarificationMode;
use arbor_core::{Plan, Task, Tier, WorkUnit, WorkUnitId};
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
    let status = StatusInfo {
        models: ["opus".to_owned(), "sonnet".to_owned(), "haiku".to_owned()],
        clarification_mode: ClarificationMode::Ask,
        branch: None,
        activity: None,
    };
    let mut terminal = Terminal::new(TestBackend::new(110, 34)).unwrap();
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
