//! Startup pruning and what the Session browser's choices do: inspect, resume, rewind or delete a
//! stored Session.

use std::time::SystemTime;

use arbor_core::{Event, RewindPoint, rewind_points};
use arbor_session::{
    SessionFolder, SessionSummary, delete_session, list_sessions, prune_sessions, rewind_session,
};
use arbor_tui::{App, BrowserOutcome, SessionBrowser, SessionView, Tone};

use super::{Project, Resumed};

/// Prunes the Sessions past retention, then notes what was pruned and how many Sessions are
/// unfinished.
pub(super) fn prune_at_startup(project: &Project, app: &mut App) {
    let Ok(repo) = project.repo() else {
        return;
    };
    let retention_days = project.config.retention_days;
    let report = prune_sessions(&repo, retention_days, SystemTime::now());
    if let Some(notice) = report.notice() {
        app.view.push_note(notice, Tone::Progress);
    }
    for warning in report.warnings() {
        app.view.push_note(warning, Tone::Failure);
    }
    let unfinished = list(project).map_or(0, |sessions| {
        sessions
            .iter()
            .filter(|summary| !summary.is_finished())
            .count()
    });
    if unfinished > 0 {
        app.view.push_note(
            format!(
                "{unfinished} unfinished Session(s): press b to resume, rewind or delete them."
            ),
            Tone::Progress,
        );
    }
}

fn list(project: &Project) -> Result<Vec<SessionSummary>, String> {
    let repo = project.repo()?;
    list_sessions(&repo, project.config.retention_days, SystemTime::now())
        .map_err(|err| err.to_string())
}

pub(super) fn open(app: &mut App, project: &Project) {
    match list(project) {
        Ok(sessions) => app.ui_state.browser = Some(SessionBrowser::new(sessions)),
        Err(message) => app.view.push_note(
            format!("Could not list the Sessions: {message}"),
            Tone::Failure,
        ),
    }
}

/// Carries out the browser's choice. Returns the Session to resume, already shown on the screen.
pub(super) fn act(
    app: &mut App,
    project: &Project,
    outcome: BrowserOutcome,
    running_here: bool,
) -> Option<Resumed> {
    let outcome_result = match outcome {
        BrowserOutcome::Close => Ok(None),
        BrowserOutcome::ListRewindPoints(folder) => list_rewind_points(app, &folder).map(|()| None),
        BrowserOutcome::Delete(folder) => delete(app, project, &folder).map(|()| None),
        _ if running_here => Err("Stop the running Session first.".to_owned()),
        BrowserOutcome::Inspect(folder) => inspect(app, &folder).map(|()| None),
        BrowserOutcome::Resume(folder) => claim(app, folder, None).map(Some),
        BrowserOutcome::Rewind(folder, point) => claim(app, folder, Some(&point)).map(Some),
    };
    outcome_result.unwrap_or_else(|message| {
        if let Some(browser) = &mut app.ui_state.browser {
            browser.set_note(message);
        }
        None
    })
}

fn read_events(folder: &SessionFolder) -> Result<Vec<Event>, String> {
    folder.read_events().map_err(|err| err.to_string())
}

fn list_rewind_points(app: &mut App, folder: &SessionFolder) -> Result<(), String> {
    let events = read_events(folder)?;
    let points = rewind_points(&events).map_err(|err| err.to_string())?;
    if let Some(browser) = &mut app.ui_state.browser {
        browser.choose_rewind(points);
    }
    Ok(())
}

fn delete(app: &mut App, project: &Project, folder: &SessionFolder) -> Result<(), String> {
    let branch = folder.branch();
    let repo = project.repo()?;
    delete_session(&repo, folder).map_err(|err| err.to_string())?;
    let sessions = list(project)?;
    if let Some(browser) = &mut app.ui_state.browser {
        browser.refresh(sessions);
        browser.set_note(format!(
            "Deleted Session {} and its branch {branch}.",
            folder.id()
        ));
    }
    Ok(())
}

/// Shows the Session's tree and event log as its log has them, without running it.
fn show(app: &mut App, folder: &SessionFolder, events: &[Event]) -> Result<(), String> {
    let view = SessionView::from_events(events).map_err(|err| err.to_string())?;
    app.start_over();
    app.view = view;
    app.status.branch = Some(folder.branch());
    Ok(())
}

fn inspect(app: &mut App, folder: &SessionFolder) -> Result<(), String> {
    show(app, folder, &read_events(folder)?)?;
    app.view.push_note(
        format!(
            "Inspecting Session {}. Press b to resume, rewind or delete it.",
            folder.id()
        ),
        Tone::Progress,
    );
    Ok(())
}

/// Claims the Session for a run, rewinding it first when asked, and shows it.
fn claim(
    app: &mut App,
    folder: SessionFolder,
    rewind_to: Option<&RewindPoint>,
) -> Result<Resumed, String> {
    let lock = folder.lock_run().map_err(|err| err.to_string())?;
    let mut events = read_events(&folder)?;
    if let Some(point) = rewind_to {
        rewind_session(&folder, &events, point).map_err(|err| err.to_string())?;
        events = read_events(&folder)?;
    }
    show(app, &folder, &events)?;
    app.view
        .push_note(format!("Resuming Session {}.", folder.id()), Tone::Operator);
    Ok(Resumed {
        folder,
        lock,
        events,
    })
}
