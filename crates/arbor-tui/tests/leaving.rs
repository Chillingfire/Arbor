use std::path::{Path, PathBuf};

use arbor_config::ClarificationMode;
use arbor_tui::{Confirmation, SessionView, StatusInfo, UiState, UnfinishedSession, draw};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn unfinished(running: bool) -> UnfinishedSession {
    UnfinishedSession {
        branch: "arbor/add-login".to_owned(),
        worktree: PathBuf::from("/work/.arbor/sessions/1-add-login/worktree"),
        markers: 4,
        placeholders: 2,
        running,
    }
}

fn rendered(ui_state: &UiState) -> String {
    let status = StatusInfo {
        models: ["opus".to_owned(), "sonnet".to_owned(), "haiku".to_owned()],
        clarification_mode: ClarificationMode::Ask,
        branch: Some("arbor/add-login".to_owned()),
        activity: None,
    };
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal
        .draw(|frame| draw(frame, &SessionView::new(), ui_state, &status))
        .unwrap();
    terminal.backend().to_string()
}

#[test]
fn the_warning_counts_markers_and_placeholders_and_says_how_to_resume_or_delete() {
    let warning = unfinished(false).warning();

    assert_eq!(
        warning[0],
        "The Session is unfinished. The branch arbor/add-login and its worktree hold 4 Markers \
         and 2 Placeholders."
    );
    assert_eq!(
        warning[1],
        "Your own checkout and the mainline are untouched."
    );
    assert!(warning[2].starts_with("To resume, rewind or delete it,"));
    assert!(warning[2].contains("press b to open the Session browser"));
    let worktree: PathBuf = Path::new("/work/.arbor/sessions/1-add-login/worktree")
        .components()
        .collect();
    assert!(
        warning[2].contains(&worktree.display().to_string()),
        "{}",
        warning[2]
    );
    assert_eq!(warning.len(), 3);
}

#[test]
fn a_running_session_says_leaving_stops_it() {
    let warning = unfinished(true).warning();
    assert!(warning[0].starts_with("Leaving stops the running Session."));
}

#[test]
fn the_quit_confirmation_shows_the_whole_warning() {
    let ui_state = UiState {
        confirming: Some(Confirmation::Quit),
        leaving: Some(unfinished(false)),
        ..UiState::default()
    };

    let screen = rendered(&ui_state);

    assert!(screen.contains("Leave arbor?"), "{screen}");
    assert!(screen.contains("4 Markers"), "{screen}");
    assert!(screen.contains("Your own checkout and the mainline are untouched."));
    assert!(screen.contains("arbor/add-login"), "{screen}");
    assert!(screen.contains("Ctrl+Q"), "{screen}");
}

#[cfg(windows)]
#[test]
fn the_worktree_path_uses_one_kind_of_separator_on_windows() {
    let mut session = unfinished(false);
    session.worktree = PathBuf::from(r"C:\work").join(".arbor/sessions/1-add-login/worktree");

    let warning = session.warning().join("\n");

    assert!(
        warning.contains(r"C:\work\.arbor\sessions\1-add-login\worktree"),
        "{warning}"
    );
    assert!(!warning.contains(".arbor/"), "{warning}");
}
