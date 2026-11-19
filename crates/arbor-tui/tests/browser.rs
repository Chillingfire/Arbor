use std::time::Duration;

use arbor_config::ClarificationMode;
use arbor_core::{Phase, RewindPoint, Usd};
use arbor_session::{SessionFolder, SessionStatus, SessionSummary};
use arbor_tui::{
    BrowserOutcome, KeyOutcome, SessionBrowser, SessionView, StatusInfo, UiState, draw,
    duration_label,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const HOUR: Duration = Duration::from_secs(60 * 60);

fn summary(id: &str, status: SessionStatus, phase: Phase) -> SessionSummary {
    SessionSummary {
        folder: SessionFolder::new(format!(".arbor/sessions/{id}")),
        branch: format!("arbor/{}", id.split_once('-').unwrap().1),
        request: format!("Request for {id}"),
        status,
        phase: Some(phase),
        cost: Usd::from_dollars(0.125),
        idle: 50 * HOUR,
        pruned_in: Some(26 * 24 * HOUR),
        markers_at_tip: Some(3),
    }
}

fn browser() -> SessionBrowser {
    SessionBrowser::new(vec![
        summary(
            "2-cancelled",
            SessionStatus::Cancelled,
            Phase::PlanCheckpoint,
        ),
        summary("1-finished", SessionStatus::Finished, Phase::Done),
    ])
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(letter: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(letter), KeyModifiers::CONTROL)
}

fn folder(id: &str) -> SessionFolder {
    SessionFolder::new(format!(".arbor/sessions/{id}"))
}

fn screen(ui_state: &UiState) -> String {
    let status = StatusInfo {
        models: ["planner".into(), "scaffolder".into(), "writer".into()],
        clarification_mode: ClarificationMode::Ask,
        branch: None,
        activity: None,
    };
    let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
    terminal
        .draw(|frame| draw(frame, &SessionView::new(), ui_state, &status))
        .unwrap();
    terminal.backend().to_string()
}

#[test]
fn each_session_shows_status_age_time_until_pruned_cost_markers_and_branch() {
    let ui_state = UiState {
        browser: Some(browser()),
        ..UiState::default()
    };
    let shown = screen(&ui_state);
    for expected in [
        "Sessions",
        "cancelled",
        "finished",
        "2d ago",
        "prune   26d",
        "$ 0.1250",
        " 3 Markers",
        "arbor/cancelled",
        "Request for 2-cancelled",
        "at Plan Checkpoint",
    ] {
        assert!(shown.contains(expected), "missing {expected:?}:\n{shown}");
    }
}

#[test]
fn a_session_finished_with_skipped_tasks_shows_how_many_and_only_rewinds() {
    let mut browser = SessionBrowser::new(vec![summary(
        "1-skipped",
        SessionStatus::FinishedWithSkippedTasks(2),
        Phase::Done,
    )]);
    let ui_state = UiState {
        browser: Some(browser.clone()),
        ..UiState::default()
    };
    let shown = screen(&ui_state);
    assert!(shown.contains("2 skipped"), "{shown}");

    assert_eq!(browser.handle_key(key(KeyCode::Char('r'))), None);
    assert_eq!(
        browser.handle_key(key(KeyCode::Char('w'))),
        Some(BrowserOutcome::ListRewindPoints(folder("1-skipped")))
    );
}

#[test]
fn b_opens_the_browser_and_esc_closes_it() {
    let mut ui_state = UiState::default();
    let view = SessionView::new();
    assert_eq!(
        ui_state.handle_key(key(KeyCode::Char('b')), &view),
        KeyOutcome::OpenBrowser
    );
    ui_state.browser = Some(browser());
    assert_eq!(
        ui_state.handle_key(key(KeyCode::Esc), &view),
        KeyOutcome::Handled
    );
    assert_eq!(ui_state.browser, None);
}

#[test]
fn delete_asks_first_naming_the_branch() {
    let mut browser = browser();
    assert_eq!(browser.handle_key(key(KeyCode::Char('d'))), None);
    let mut ui_state = UiState {
        browser: Some(browser.clone()),
        ..UiState::default()
    };
    let shown = screen(&ui_state);
    assert!(shown.contains("its branch arbor/cancelled?"), "{shown}");
    assert_eq!(
        ui_state.handle_key(key(KeyCode::Char('d')), &SessionView::new()),
        KeyOutcome::Handled
    );

    assert_eq!(
        browser.handle_key(ctrl('d')),
        Some(BrowserOutcome::Delete(folder("2-cancelled")))
    );
}

#[test]
fn only_an_unfinished_session_resumes_but_a_finished_one_can_be_rewound() {
    let mut browser = browser();
    assert_eq!(
        browser.handle_key(key(KeyCode::Char('r'))),
        Some(BrowserOutcome::Resume(folder("2-cancelled")))
    );
    browser.handle_key(key(KeyCode::Down));
    assert_eq!(browser.handle_key(key(KeyCode::Char('r'))), None);
    assert_eq!(
        browser.handle_key(key(KeyCode::Char('w'))),
        Some(BrowserOutcome::ListRewindPoints(folder("1-finished")))
    );
    assert_eq!(
        browser.handle_key(key(KeyCode::Enter)),
        Some(BrowserOutcome::Inspect(folder("1-finished")))
    );
}

#[test]
fn rewinding_picks_a_checkpoint_newest_first_and_asks_first() {
    let mut browser = browser();
    assert_eq!(
        browser.handle_key(key(KeyCode::Char('w'))),
        Some(BrowserOutcome::ListRewindPoints(folder("2-cancelled")))
    );
    let point = |event_count| RewindPoint {
        event_count,
        phase: Phase::PlanCheckpoint,
        head: None,
        reviewed_edits: None,
    };
    browser.choose_rewind(vec![point(2), point(4)]);
    browser.handle_key(key(KeyCode::Down));
    browser.handle_key(key(KeyCode::Enter));
    assert_eq!(browser.handle_key(key(KeyCode::Char('r'))), None);

    browser.choose_rewind(vec![point(2), point(4)]);
    browser.handle_key(key(KeyCode::Enter));
    assert_eq!(
        browser.handle_key(ctrl('r')),
        Some(BrowserOutcome::Rewind(
            folder("2-cancelled"),
            Box::new(point(4))
        ))
    );
}

#[test]
fn durations_read_in_the_largest_whole_unit() {
    assert_eq!(duration_label(Duration::from_secs(59)), "0m");
    assert_eq!(duration_label(3 * HOUR), "3h");
    assert_eq!(duration_label(49 * HOUR), "2d");
}
