use arbor_config::ClarificationMode;
use arbor_core::{FailedAttempt, RevisionCause, Tier, Usd, WorkUnitFailure, WorkUnitId};
use arbor_session::{Checkpoint, Decision, StrongerWriterOffer};
use arbor_tui::{KeyOutcome, Modal, SessionView, StatusInfo, UiState, draw};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn press(ui_state: &mut UiState, code: KeyCode) -> KeyOutcome {
    ui_state.handle_key(KeyEvent::new(code, KeyModifiers::NONE), &SessionView::new())
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
    let mut terminal = Terminal::new(TestBackend::new(110, 40)).unwrap();
    terminal
        .draw(|frame| draw(frame, &SessionView::new(), ui_state, &status))
        .unwrap();
    terminal.backend().to_string()
}

fn work_unit_failed() -> RevisionCause {
    RevisionCause::WorkUnitFailed(WorkUnitFailure {
        id: WorkUnitId(3),
        attempts: vec![FailedAttempt {
            reply: "None".to_owned(),
            error: "test longest_word_tests::ties ... FAILED".to_owned(),
        }],
        missing_response: None,
        broken_region: None,
    })
}

fn offer() -> StrongerWriterOffer {
    StrongerWriterOffer {
        model: "anthropic/claude-sonnet-5.5".to_owned(),
        cost_per_attempt: Usd::from_dollars(0.012),
        max_attempts: 3,
    }
}

fn escalated(cause: RevisionCause, may_replan: bool) -> UiState {
    let stronger_writer = cause.work_unit().map(|_| offer());
    let mut ui_state = UiState::default();
    ui_state.open_modal(Modal::checkpoint(Checkpoint::Escalation {
        task: 0,
        cause,
        may_replan,
        failed_fix: None,
        later_tasks: vec!["Print the longest word".to_owned()],
        stronger_writer,
    }));
    ui_state
}

#[test]
fn a_hint_goes_to_the_tier_picked_with_tab() {
    let mut ui_state = escalated(work_unit_failed(), true);
    press(&mut ui_state, KeyCode::Char('h'));
    assert!(screen(&ui_state).contains("Hint for the Writer"));
    type_text(&mut ui_state, "Use fold");
    press(&mut ui_state, KeyCode::Tab);
    assert!(screen(&ui_state).contains("Hint for the Scaffolder"));

    assert_eq!(
        press(&mut ui_state, KeyCode::Enter),
        KeyOutcome::Decided(Decision::RetryWithHint {
            tier: Tier::Scaffolder,
            hint: "Use fold".to_owned(),
        })
    );
}

#[test]
fn after_a_failed_task_check_a_hint_cant_go_to_the_writer_nor_to_a_used_replan() {
    let mut ui_state = escalated(
        RevisionCause::TaskChecksFailed {
            output: "test failed".to_owned(),
        },
        false,
    );
    press(&mut ui_state, KeyCode::Char('h'));
    assert!(screen(&ui_state).contains("Hint for the Scaffolder"));
    press(&mut ui_state, KeyCode::Tab);
    assert!(screen(&ui_state).contains("Hint for the Scaffolder"));
    press(&mut ui_state, KeyCode::Esc);
    assert!(screen(&ui_state).contains("Skip the Task"));
}

#[test]
fn the_stronger_model_and_its_cost_show_before_it_is_chosen() {
    let mut ui_state = escalated(work_unit_failed(), true);
    let escalation_screen = screen(&ui_state);
    assert!(escalation_screen.contains("anthropic/claude-sonnet-5.5"));
    assert!(escalation_screen.contains("$0.0120"));
    assert!(escalation_screen.contains("$0.0360"));

    assert_eq!(
        press(&mut ui_state, KeyCode::Char('m')),
        KeyOutcome::Decided(Decision::RetryWithStrongerWriter)
    );
}

#[test]
fn a_failed_task_check_has_no_stronger_model_to_offer() {
    let mut ui_state = escalated(
        RevisionCause::TaskChecksFailed {
            output: "test failed".to_owned(),
        },
        true,
    );
    assert_eq!(
        press(&mut ui_state, KeyCode::Char('m')),
        KeyOutcome::Handled
    );
}

#[test]
fn skipping_warns_about_later_tasks_and_waits_for_confirmation() {
    let mut ui_state = escalated(work_unit_failed(), true);
    assert_eq!(
        press(&mut ui_state, KeyCode::Char('k')),
        KeyOutcome::Handled
    );
    let confirmation_screen = screen(&ui_state);
    assert!(confirmation_screen.contains("Skip Task 1?"));
    assert!(confirmation_screen.contains("2. Print the longest word"));

    assert_eq!(
        press(&mut ui_state, KeyCode::Enter),
        KeyOutcome::Decided(Decision::Skip)
    );
}

#[test]
fn an_abort_is_cancelled_by_any_other_key_and_sent_once_confirmed() {
    let mut ui_state = escalated(work_unit_failed(), true);
    press(&mut ui_state, KeyCode::Char('x'));
    assert!(screen(&ui_state).contains("Abort the Session?"));
    assert_eq!(
        press(&mut ui_state, KeyCode::Char('q')),
        KeyOutcome::Handled
    );
    assert!(!screen(&ui_state).contains("Abort the Session?"));

    press(&mut ui_state, KeyCode::Char('x'));
    assert_eq!(
        press(&mut ui_state, KeyCode::Enter),
        KeyOutcome::Decided(Decision::Abort)
    );
}
