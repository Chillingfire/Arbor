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
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arbor_config::{
    API_KEY_ENV, ClarificationMode, Config, ConfigError, Platform, resolve_api_key,
    user_config_path,
};
use arbor_core::{Effect, Phase, Request};
use arbor_git::{Repo, SessionBranch, Worktree};
use arbor_llm::OpenRouterClient;
use arbor_session::{Autopilot, AutopilotUpdate, SessionEnd};
use arbor_tui::{App, Confirmation, Focus, KeyOutcome, StatusInfo, Tone};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event as TerminalEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::execute;
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
    driver: JoinHandle<()>,
}

struct Project {
    root: PathBuf,
    config: Config,
    user_config: Option<PathBuf>,
}

impl Project {
    /// Read again for each Session, so a key set after a failed start is picked up.
    fn api_key(&self) -> Result<Option<String>, ConfigError> {
        let user_text = self
            .user_config
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok());
        let api_key = resolve_api_key(env::var(API_KEY_ENV).ok().as_deref(), user_text.as_deref())?;
        Ok(api_key.map(|key| key.expose().to_owned()))
    }
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
    let outcome = ratatui::run(|terminal| {
        // The legacy Windows console can't do bracketed paste; there a paste arrives as keys.
        let _ = execute!(io::stdout(), EnableBracketedPaste);
        let looped = event_loop(terminal, &mut app, &project, initial_request);
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        looped
    });
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
    let user_config = user_config_path(Platform::current(), |name| env::var(name).ok());
    let config = Config::load(user_config.as_deref(), &root).map_err(|err| err.to_string())?;
    let project = Project {
        root,
        config,
        user_config,
    };
    project.api_key().map_err(|err| err.to_string())?;
    Ok(project)
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
        run.follow(app);
        terminal.draw(|frame| app.draw(frame))?;
        // Take every event already waiting before drawing again, so a paste doesn't trail behind.
        let mut event_waiting = event::poll(FRAME)?;
        while event_waiting {
            let terminal_event = event::read()?;
            event_waiting = event::poll(Duration::ZERO)?;
            let key = match terminal_event {
                TerminalEvent::Paste(pasted) => {
                    app.ui_state.paste(&pasted);
                    continue;
                }
                TerminalEvent::Key(key) if key.kind == KeyEventKind::Press => key,
                _ => continue,
            };
            let key = if event_waiting {
                as_pasted(key, app.ui_state.focus)
            } else {
                key
            };
            if run.handle_key(app, project, key).is_break() {
                app.view
                    .push_note("Stopping the Session before leaving…", Tone::Operator);
                terminal.draw(|frame| app.draw(frame))?;
                run.stop();
                return Ok(());
            }
        }
    }
}

/// Where bracketed paste isn't available, a paste arrives as keys all at once. An Enter with more
/// keys right behind it is a pasted newline, not the Human Operator sending the prompt.
fn as_pasted(key: KeyEvent, focus: Focus) -> KeyEvent {
    if focus == Focus::Prompt && key.code == KeyCode::Enter && key.modifiers.is_empty() {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)
    } else {
        key
    }
}

/// The Session behind the screen, once the Human Operator has sent a Request.
#[derive(Default)]
struct Run {
    session: Option<RunningSession>,
}

impl Run {
    fn start(&mut self, app: &mut App, project: &Project, request: Request) {
        if app.view.session().phase() != &Phase::NotStarted {
            app.start_over();
        }
        self.session = start_session(app, project, request);
    }

    /// Applies what the Session thread sent, and forgets the Session once it has ended.
    fn follow(&mut self, app: &mut App) {
        if let Some(session) = &mut self.session
            && !drain_messages(app, session)
        {
            self.session = None;
            app.stop_working();
            app.ui_state.confirming = None;
        }
    }

    fn cancel(&mut self) {
        if let Some(session) = &mut self.session
            && let Some(cancel) = session.cancel.take()
        {
            let _ = cancel.send(());
        }
    }

    /// Cancels the Session and waits for its thread, so a running Check is killed and the
    /// worktree is put back before the process exits.
    fn stop(&mut self) {
        self.cancel();
        if let Some(session) = self.session.take() {
            let _ = session.driver.join();
        }
    }

    fn handle_key(&mut self, app: &mut App, project: &Project, key: KeyEvent) -> ControlFlow<()> {
        match app.ui_state.handle_key(key, &app.view) {
            KeyOutcome::Handled => {}
            KeyOutcome::Quit if self.session.is_none() => return ControlFlow::Break(()),
            KeyOutcome::Quit => app.ui_state.confirming = Some(Confirmation::Quit),
            KeyOutcome::Cancel if self.session.is_some() => {
                app.ui_state.confirming = Some(Confirmation::Cancel);
            }
            KeyOutcome::Cancel => {}
            KeyOutcome::Confirmed(Confirmation::Quit) => return ControlFlow::Break(()),
            KeyOutcome::Confirmed(Confirmation::Cancel) => self.cancel(),
            KeyOutcome::Submitted(text) if self.session.is_none() => {
                self.start(app, project, Request { text, issue: None });
                if self.session.is_some() {
                    app.ui_state.focus = Focus::Tree;
                }
            }
            KeyOutcome::Submitted(_) => app.view.push_note(
                "Answers and hints reach the Tiers once the Checkpoint modals exist (#15).",
                Tone::Progress,
            ),
        }
        ControlFlow::Continue(())
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
    let api_key = match project.api_key() {
        Ok(Some(api_key)) => api_key,
        Ok(None) => {
            app.view.push_note(
                format!(
                    "No OpenRouter API key: set {API_KEY_ENV} or api_key in the user config, \
                     then send the Request again."
                ),
                Tone::Failure,
            );
            return None;
        }
        Err(err) => {
            app.view.push_note(err.to_string(), Tone::Failure);
            return None;
        }
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
    let driver = thread::spawn(move || {
        let ending = drive(&root, config, &api_key, request, &sender, cancelled);
        let _ = sender.send(ending);
    });
    Some(RunningSession {
        messages,
        cancel: Some(cancel),
        driver,
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
    if let Err(message) = exclude_sessions(&repo) {
        return failed(format!(
            "Could not keep {SESSIONS_DIR} out of git: {message}"
        ));
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

    let client = OpenRouterClient::new(api_key);
    let observer_sender = sender.clone();
    let autopilot = Autopilot::new(&worktree, &mainline, &client, config);
    let mut autopilot = match autopilot {
        Ok(autopilot) => autopilot
            .log_events_to(event_file)
            .on_update(move |update| {
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
    // Dropping the run future at a cancel kills a running Check (`kill_on_drop`); the worktree is
    // put back after that, below.
    let session_end = runtime.block_on(async {
        tokio::select! {
            session_end = autopilot.run(request) => Some(session_end),
            _ = cancelled => None,
        }
    });
    match session_end {
        Some(session_end) => ending(session_end),
        None => cancelled_ending(autopilot.session().phase(), &worktree),
    }
}

/// After a cancel, puts the worktree back to its last commit when the cancelled step had written
/// files no event records yet, such as a Writer's code still in its Check, so the kept worktree
/// matches the event log.
fn cancelled_ending(phase: &Phase, worktree: &Worktree) -> DriverMessage {
    let unrecorded_edits = matches!(
        phase,
        Phase::Writing { .. } | Phase::Scaffolding { .. } | Phase::Revising { .. }
    );
    if !unrecorded_edits {
        return DriverMessage::Ended(
            "Cancelled. The branch and worktree are kept.".to_owned(),
            Tone::Operator,
        );
    }
    match worktree.head().and_then(|head| worktree.reset_to(&head)) {
        Ok(()) => DriverMessage::Ended(
            "Cancelled. The unfinished step's edits were undone; the branch and worktree are kept \
             at their last commit."
                .to_owned(),
            Tone::Operator,
        ),
        Err(err) => DriverMessage::Ended(
            format!(
                "Cancelled, but the unfinished step's edits couldn't be undone: {err}. Run \
                 `git reset --hard` in {} before using it.",
                worktree.path().display()
            ),
            Tone::Failure,
        ),
    }
}

fn ending(session_end: Result<SessionEnd, arbor_session::AutopilotError>) -> DriverMessage {
    match session_end {
        Ok(SessionEnd::Finished) => {
            DriverMessage::Ended("Session finished.".to_owned(), Tone::Success)
        }
        Ok(SessionEnd::NeedsOperator(effect)) => DriverMessage::Ended(
            format!(
                "Stopped: {} needs the Checkpoint modals (#15).",
                operator_step(&effect)
            ),
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

/// What the Session waits on the Human Operator for, in a few words.
fn operator_step(effect: &Effect) -> String {
    match effect {
        Effect::ShowPlanCheckpoint => "the Plan Checkpoint".to_owned(),
        Effect::ShowScaffoldCheckpoint { task } => {
            format!("the Scaffold Checkpoint of Task {}", task + 1)
        }
        Effect::ShowRevisionCheckpoint { task, .. } => {
            format!("the Revision Checkpoint of Task {}", task + 1)
        }
        Effect::ShowEscalationCheckpoint { task, .. } => {
            format!("the Escalation Checkpoint of Task {}", task + 1)
        }
        Effect::ShowReplanCheckpoint { task, .. } => {
            format!("the Replan Checkpoint of Task {}", task + 1)
        }
        Effect::ShowMarkerCheckpoint { remaining } => format!(
            "the Marker Checkpoint ({} Marker{} left)",
            remaining.len(),
            if remaining.len() == 1 { "" } else { "s" }
        ),
        Effect::ShowBudgetCheckpoint => "the Budget Checkpoint".to_owned(),
        Effect::RunPlanner { .. } | Effect::RunScaffolder { .. } => {
            "reworking after a rejected Checkpoint".to_owned()
        }
        Effect::RunReplan { task, .. } => format!("the Replan of Task {}", task + 1),
        Effect::ResetTask { task } => format!("resetting Task {}", task + 1),
        _ => "the next step".to_owned(),
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

/// Adds the Sessions folder to the `info/exclude` every worktree shares, so the worktrees inside
/// the project don't show up as untracked files, even when arbor runs from a linked worktree.
fn exclude_sessions(repo: &Repo) -> Result<(), String> {
    let info_dir = repo
        .common_dir()
        .map_err(|err| err.to_string())?
        .join("info");
    fs::create_dir_all(&info_dir).map_err(|err| err.to_string())?;
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
        .map_err(|err| err.to_string())
}
