//! Startup pruning and what the Session browser's choices do: inspect, resume, rewind or delete a
//! stored Session.

use std::time::SystemTime;

use arbor_core::{Event, RewindPoint, rewind_points};
use arbor_git::{Repo, TreeSource};
use arbor_session::{
    SessionFolder, SessionSummary, delete_session, list_sessions, prune_sessions, rewind_session,
};
use arbor_tiers::{MarkerLine, ScanError, scan_markers};
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
        BrowserOutcome::Resume(folder) => check_changes_were_listed(app, project, &folder)
            .and_then(|()| claim(app, folder, None))
            .map(Some),
        BrowserOutcome::Rewind(folder, point) => check_changes_were_listed(app, project, &folder)
            .and_then(|()| claim(app, folder, Some(&point)))
            .map(Some),
    };
    outcome_result.unwrap_or_else(|message| {
        if let Some(browser) = &mut app.ui_state.browser {
            browser.set_note(message);
        }
        None
    })
}

/// Refuses when the worktree holds uncommitted changes the listing didn't show, since the Human
/// Operator wasn't asked about discarding them. The listing is refreshed so the next try asks.
fn check_changes_were_listed(
    app: &mut App,
    project: &Project,
    folder: &SessionFolder,
) -> Result<(), String> {
    let listed_with_changes = app
        .ui_state
        .browser
        .as_ref()
        .and_then(|browser| {
            browser
                .sessions()
                .iter()
                .find(|summary| summary.folder == *folder)
        })
        .is_some_and(|summary| summary.uncommitted_changes);
    if listed_with_changes || !folder.has_uncommitted_changes() {
        return Ok(());
    }
    let sessions = list(project)?;
    if let Some(browser) = &mut app.ui_state.browser {
        browser.refresh(sessions);
    }
    Err(format!(
        "Session {}'s worktree has gained uncommitted changes since it was listed. Choose again \
         to see whether they would be discarded.",
        folder.id()
    ))
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
    note_open_regions(app, &folder);
    Ok(Resumed {
        folder,
        lock,
        events,
    })
}

/// Runs the Marker check on the resumed Session's worktree, so the Human Operator sees which
/// Regions are still open.
fn note_open_regions(app: &mut App, folder: &SessionFolder) {
    let scanned = Repo::open(folder.worktree_path())
        .map_err(ScanError::Git)
        .and_then(|repo| {
            scan_markers(
                &repo,
                &TreeSource::WorkingTree,
                repo.head_commit().as_deref(),
            )
        });
    match scanned {
        Ok(marker_lines) => app
            .view
            .push_note(open_regions(&marker_lines), Tone::Progress),
        Err(err) => app.view.push_note(
            format!("Could not check the worktree for Markers: {err}"),
            Tone::Failure,
        ),
    }
}

/// One line naming each Region a BEGIN Marker still opens, by its Marker and place.
fn open_regions(marker_lines: &[MarkerLine]) -> String {
    let regions: Vec<String> = marker_lines
        .iter()
        .filter(|marker_line| marker_line.opens_region())
        .map(|marker_line| {
            let work_unit = marker_line.text.split_whitespace().last().unwrap_or("");
            format!("{work_unit} at {}:{}", marker_line.file, marker_line.line)
        })
        .collect();
    match regions.len() {
        0 if marker_lines.is_empty() => "No Markers left in the worktree.".to_owned(),
        0 => format!(
            "{} stray Marker line(s) in the worktree, but no open Region.",
            marker_lines.len()
        ),
        count => format!("{count} Region(s) still open: {}.", regions.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker_line(file: &str, line: usize, text: &str) -> MarkerLine {
        MarkerLine {
            file: file.to_owned(),
            line,
            text: text.to_owned(),
        }
    }

    #[test]
    fn the_note_names_each_open_region_by_work_unit_and_place() {
        let marker_lines = [
            marker_line("src/lib.rs", 3, "// ARBOR:BEGIN WU-2"),
            marker_line("src/lib.rs", 5, "// ARBOR:END WU-2"),
            marker_line("src/main.rs", 9, "# ARBOR:BEGIN WU-4"),
        ];
        assert_eq!(
            open_regions(&marker_lines),
            "2 Region(s) still open: WU-2 at src/lib.rs:3, WU-4 at src/main.rs:9."
        );
        assert_eq!(open_regions(&[]), "No Markers left in the worktree.");
    }
}
