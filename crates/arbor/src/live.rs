//! Runs the TUI and, once there is a Request, a live Session behind it. The Session runs on its own
//! thread with the Autopilot, which approves every Checkpoint as it is until the Checkpoint modals
//! exist (#15), and streams its updates to the screen.

use std::env;
use std::fs;
use std::io;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arbor_config::{
    API_KEY_ENV, ClarificationMode, Config, Platform, resolve_api_key, user_config_path,
};
use arbor_core::{EventLog, Request};
use arbor_git::{Repo, SessionBranch};
use arbor_llm::OpenRouterClient;
use arbor_session::{Autopilot, AutopilotUpdate, SessionEnd};
use arbor_tui::{App, Focus, KeyOutcome, StatusInfo, Tone};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event as TerminalEvent, KeyEvent, KeyEventKind};
use tokio::sync::oneshot;

/// How often the screen redraws while waiting, so the spinner moves.
const FRAME: Duration = Duration::from_millis(100);

/// Where Sessions live, relative to the project root.
const SESSIONS_DIR: &str = ".arbor/sessions";

/// What the Session thread sends to the screen.
enum DriverMessage {
    Started {
        branch: String,
        session_dir: PathBuf,
    },
    Update(AutopilotUpdate),
    Ended(String, Tone),
}

/// The Session running behind the screen.
struct RunningSession {
    messages: Receiver<DriverMessage>,
    cancel: Option<oneshot::Sender<()>>,
}

struct Project {
    root: PathBuf,
    config: Config,
    api_key: Option<String>,
}

pub fn run(initial_request: Option<Request>) -> ExitCode {
    let project = match load_project() {
        Ok(project) => project,
        Err(message) => {
            eprintln!("arbor: {message}");
            return ExitCode::FAILURE;
        }
    };
    let roles = &project.config.roles;
    let mut app = App::new(StatusInfo {
        models: [
            roles.planner.model.clone(),
            roles.scaffolder.model.clone(),
            roles.writer.model.clone(),
        ],
        // Nobody can answer the Planner's questions until the question modal exists (#15).
        clarification_mode: ClarificationMode::Assume,
        branch: None,
        activity: None,
    });
    let outcome =
        ratatui::run(|terminal| event_loop(terminal, &mut app, &project, initial_request));
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("arbor: the terminal failed: {err}");
            ExitCode::FAILURE
        }
    }
}

fn load_project() -> Result<Project, String> {
    let root =
        env::current_dir().map_err(|err| format!("could not read the current directory: {err}"))?;
    let repo =
        Repo::open(&root).map_err(|err| format!("{err}; run arbor inside a git repository"))?;
    let root = repo.root().to_owned();
    let user_path = user_config_path(Platform::current(), |name| env::var(name).ok());
    let config = Config::load(user_path.as_deref(), &root).map_err(|err| err.to_string())?;
    let user_text = user_path.and_then(|path| fs::read_to_string(path).ok());
    let api_key = resolve_api_key(env::var(API_KEY_ENV).ok().as_deref(), user_text.as_deref())
        .map_err(|err| err.to_string())?
        .map(|key| key.expose().to_owned());
    Ok(Project {
        root,
        config,
        api_key,
    })
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    project: &Project,
    initial_request: Option<Request>,
) -> io::Result<()> {
    let mut run = Run::default();
    match initial_request {
        Some(request) => run.start(app, project, request),
        None => app.ui_state.focus = Focus::Prompt,
    }
    loop {
        if let Some(session) = &mut run.session
            && !drain_messages(app, session)
        {
            run.session = None;
            app.stop_working();
        }
        terminal.draw(|frame| app.draw(frame))?;
        // Take every key already waiting before drawing again, so a paste doesn't trail behind.
        let mut key_waiting = event::poll(FRAME)?;
        while key_waiting {
            if let TerminalEvent::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
                && run.handle_key(app, project, key).is_break()
            {
                return Ok(());
            }
            key_waiting = event::poll(Duration::ZERO)?;
        }
    }
}

/// The Session behind the screen, once the Human Operator has sent a Request.
#[derive(Default)]
struct Run {
    session: Option<RunningSession>,
    started: bool,
}

impl Run {
    fn start(&mut self, app: &mut App, project: &Project, request: Request) {
        self.session = start_session(app, project, request);
        self.started = true;
    }

    fn cancel(&mut self) {
        if let Some(session) = &mut self.session {
            cancel(session);
        }
    }

    fn handle_key(&mut self, app: &mut App, project: &Project, key: KeyEvent) -> ControlFlow<()> {
        match app.ui_state.handle_key(key, &app.view) {
            KeyOutcome::Handled => {}
            KeyOutcome::Quit => {
                self.cancel();
                return ControlFlow::Break(());
            }
            KeyOutcome::Cancel => self.cancel(),
            KeyOutcome::Submitted(text) if !self.started => {
                self.start(app, project, Request { text, issue: None });
                app.ui_state.focus = Focus::Tree;
            }
            KeyOutcome::Submitted(_) => app.view.push_note(
                "Answers and hints reach the Tiers once the Checkpoint modals exist (#15).",
                Tone::Progress,
            ),
        }
        ControlFlow::Continue(())
    }
}

fn cancel(session: &mut RunningSession) {
    if let Some(cancel) = session.cancel.take() {
        let _ = cancel.send(());
    }
}

/// Applies every message waiting from the Session thread. Returns `false` once the run has ended.
fn drain_messages(app: &mut App, session: &mut RunningSession) -> bool {
    loop {
        match session.messages.try_recv() {
            Ok(DriverMessage::Started {
                branch,
                session_dir,
            }) => {
                app.status.branch = Some(branch);
                app.view.push_note(
                    format!("Session folder {}", session_dir.display()),
                    Tone::Progress,
                );
            }
            Ok(DriverMessage::Update(update)) => app.apply_update(update),
            Ok(DriverMessage::Ended(message, tone)) => {
                app.view.push_note(message, tone);
                return false;
            }
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
}

fn start_session(app: &mut App, project: &Project, request: Request) -> Option<RunningSession> {
    let Some(api_key) = project.api_key.clone() else {
        app.view.push_note(
            format!("No OpenRouter API key: set {API_KEY_ENV} or api_key in the user config."),
            Tone::Failure,
        );
        return None;
    };
    app.view.push_note(
        "Checkpoints are approved automatically and Clarification Mode is Assume until the \
         Checkpoint modals exist (#15).",
        Tone::Operator,
    );
    let (sender, messages) = mpsc::channel();
    let (cancel, cancelled) = oneshot::channel();
    let root = project.root.clone();
    let mut config = project.config.clone();
    config.clarification_mode = ClarificationMode::Assume;
    thread::spawn(move || {
        let ending = drive(&root, config, &api_key, request, &sender, cancelled);
        let _ = sender.send(ending);
    });
    Some(RunningSession {
        messages,
        cancel: Some(cancel),
    })
}

/// Creates the Session branch and worktree and runs the Session. Returns the last message.
fn drive(
    root: &Path,
    config: Config,
    api_key: &str,
    request: Request,
    sender: &Sender<DriverMessage>,
    cancelled: oneshot::Receiver<()>,
) -> DriverMessage {
    let failed = |message: String| DriverMessage::Ended(message, Tone::Failure);
    let repo = match Repo::open(root) {
        Ok(repo) => repo,
        Err(err) => return failed(format!("Could not open the repository: {err}")),
    };
    let mainline = match repo.mainline(config.mainline.as_deref()) {
        Ok(mainline) => mainline,
        Err(err) => return failed(format!("Could not find the mainline: {err}")),
    };
    let branch = match free_branch(&repo, &request.text) {
        Ok(branch) => branch,
        Err(message) => return failed(message),
    };
    let session_dir = root.join(SESSIONS_DIR).join(session_id(&branch));
    if let Err(err) = exclude_sessions(root) {
        return failed(format!("Could not keep {SESSIONS_DIR} out of git: {err}"));
    }
    if let Err(err) = fs::create_dir_all(&session_dir) {
        return failed(format!("Could not create {}: {err}", session_dir.display()));
    }
    let worktree = match repo.create_worktree(&mainline, &branch, &session_dir.join("worktree")) {
        Ok(worktree) => worktree,
        Err(err) => return failed(format!("Could not create the Session worktree: {err}")),
    };
    let event_file = match fs::File::create(session_dir.join("events.jsonl")) {
        Ok(event_file) => event_file,
        Err(err) => return failed(format!("Could not create the event log: {err}")),
    };
    let _ = sender.send(DriverMessage::Started {
        branch: branch.as_str().to_owned(),
        session_dir: session_dir.clone(),
    });

    let mut event_log = EventLog::new(event_file);
    let client = OpenRouterClient::new(api_key);
    let observer_sender = sender.clone();
    let autopilot = Autopilot::new(&worktree, &mainline, &client, config);
    let mut autopilot = match autopilot {
        Ok(autopilot) => autopilot.on_update(move |update| {
            if let AutopilotUpdate::Event(event) = &update {
                let _ = event_log.append(event);
            }
            let _ = observer_sender.send(DriverMessage::Update(update));
        }),
        Err(err) => return failed(err.to_string()),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => return failed(format!("Could not start the async runtime: {err}")),
    };
    runtime.block_on(async {
        tokio::select! {
            session_end = autopilot.run(request) => ending(session_end),
            _ = cancelled => DriverMessage::Ended(
                "Cancelled. The branch and worktree are kept.".to_owned(),
                Tone::Operator,
            ),
        }
    })
}

fn ending(session_end: Result<SessionEnd, arbor_session::AutopilotError>) -> DriverMessage {
    match session_end {
        Ok(SessionEnd::Finished) => {
            DriverMessage::Ended("Session finished.".to_owned(), Tone::Success)
        }
        Ok(SessionEnd::NeedsOperator(effect)) => DriverMessage::Ended(
            format!("Stopped: {effect:?} needs the Checkpoint modals (#15)."),
            Tone::Failure,
        ),
        Ok(SessionEnd::TierStopped { tier, reason }) => {
            DriverMessage::Ended(format!("Stopped: the {tier:?} {reason}"), Tone::Failure)
        }
        Ok(SessionEnd::ScaffoldChecksFailed { check_output }) => DriverMessage::Ended(
            format!(
                "Stopped: the Scaffold Check failed: {}",
                check_output.lines().last().unwrap_or_default()
            ),
            Tone::Failure,
        ),
        Err(err) => DriverMessage::Ended(format!("Stopped: {err}"), Tone::Failure),
    }
}

/// A Session branch named after the Request's first words that doesn't exist yet.
fn free_branch(repo: &Repo, request_text: &str) -> Result<SessionBranch, String> {
    let feature: Vec<&str> = request_text.split_whitespace().take(6).collect();
    let feature = feature.join(" ");
    let existing = repo.branches().map_err(|err| err.to_string())?;
    for attempt in 1..100 {
        let candidate = if attempt == 1 {
            feature.clone()
        } else {
            format!("{feature} {attempt}")
        };
        let branch = SessionBranch::for_feature(&candidate).map_err(|err| err.to_string())?;
        if !existing.iter().any(|name| name == branch.as_str()) {
            return Ok(branch);
        }
    }
    Err("every Session branch name for this Request is taken".to_owned())
}

fn session_id(branch: &SessionBranch) -> String {
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs());
    let slug = branch.as_str().trim_start_matches("arbor/");
    format!("{started}-{slug}")
}

/// Adds the Sessions folder to `.git/info/exclude`, so the worktrees inside the project don't show
/// up as untracked files.
fn exclude_sessions(root: &Path) -> io::Result<()> {
    let info_dir = root.join(".git").join("info");
    if !info_dir.is_dir() {
        // A linked worktree or submodule keeps `.git` as a file; leave it alone.
        return Ok(());
    }
    let exclude_path = info_dir.join("exclude");
    let existing = fs::read_to_string(&exclude_path).unwrap_or_default();
    let pattern = format!("/{SESSIONS_DIR}/");
    if existing.lines().any(|line| line.trim() == pattern) {
        return Ok(());
    }
    let separator = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    fs::write(&exclude_path, format!("{existing}{separator}{pattern}\n"))
}
