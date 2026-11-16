//! Runs the TUI and, once there is a Request, a live Session behind it. The Session runs on its own
//! thread with the Autopilot and streams its updates to the screen. At a Checkpoint or a Tier's
//! questions it waits for the Human Operator's reply from a modal.

use std::env;
use std::fs;
use std::io;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arbor_config::{API_KEY_ENV, Config, ConfigError, Platform, load_api_key, user_config_path};
use arbor_core::{Effect, Phase, Request, Tier};
use arbor_git::{Repo, SESSIONS_DIR, SessionBranch, Worktree};
use arbor_llm::OpenRouterClient;
use arbor_session::{
    Autopilot, AutopilotUpdate, Checkpoint, Decision, Operator, Pending, SessionEnd,
};
use arbor_tui::{App, Confirmation, Focus, KeyOutcome, Modal, StatusInfo, Tone, UnfinishedSession};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event as TerminalEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::execute;
use tokio::sync::oneshot;

use crate::leaving::{self, StartedSession};
use crate::startup;

/// The Session worktree's folder inside its Session folder.
const WORKTREE_DIR: &str = "worktree";

/// How often the screen redraws while waiting, so the spinner moves.
const FRAME: Duration = Duration::from_millis(100);

/// What the Session thread sends to the screen.
enum DriverMessage {
    Started {
        branch: String,
        session_dir: PathBuf,
    },
    Update(AutopilotUpdate),
    Checkpoint {
        checkpoint: Checkpoint,
        reply: oneshot::Sender<Decision>,
    },
    Questions {
        tier: Tier,
        questions: String,
        reply: oneshot::Sender<Option<String>>,
    },
    Ended(String, Tone),
}

/// Where the Human Operator's reply to the open modal goes.
enum AwaitedReply {
    Decision(oneshot::Sender<Decision>),
    Answers(oneshot::Sender<Option<String>>),
}

/// The Human Operator at the screen: each Checkpoint and question opens a modal there, and the
/// Session waits for the reply.
struct ScreenOperator {
    sender: Sender<DriverMessage>,
}

/// Without a screen left to reply, the Session waits until it is cancelled rather than go on
/// undecided.
async fn screen_reply<T>(replied: oneshot::Receiver<T>) -> T {
    match replied.await {
        Ok(reply) => reply,
        Err(_) => std::future::pending().await,
    }
}

impl Operator for ScreenOperator {
    fn decide<'a>(&'a mut self, checkpoint: &'a Checkpoint) -> Pending<'a, Decision> {
        let (reply, replied) = oneshot::channel();
        let _ = self.sender.send(DriverMessage::Checkpoint {
            checkpoint: checkpoint.clone(),
            reply,
        });
        Box::pin(screen_reply(replied))
    }

    fn answer<'a>(&'a mut self, tier: Tier, questions: &'a str) -> Pending<'a, Option<String>> {
        let (reply, replied) = oneshot::channel();
        let _ = self.sender.send(DriverMessage::Questions {
            tier,
            questions: questions.to_owned(),
            reply,
        });
        Box::pin(screen_reply(replied))
    }
}

/// The Session running behind the screen.
struct RunningSession {
    messages: Receiver<DriverMessage>,
    cancel: Option<oneshot::Sender<()>>,
    driver: JoinHandle<()>,
    awaited_reply: Option<AwaitedReply>,
    /// Set once the Session's branch and worktree exist.
    started: Option<StartedSession>,
}

struct Project {
    root: PathBuf,
    config: Config,
    user_config: Option<PathBuf>,
}

impl Project {
    /// Read again for each Session, so a key set after a failed start is picked up.
    fn api_key(&self) -> Result<Option<String>, ConfigError> {
        let api_key = load_api_key(self.user_config.as_deref())?;
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
    startup::check(&project.root, &project.config);
    let roles = &project.config.roles;
    let mut app = App::new(StatusInfo {
        models: [
            roles.planner.model.clone(),
            roles.scaffolder.model.clone(),
            roles.writer.model.clone(),
        ],
        clarification_mode: project.config.clarification_mode,
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
        Ok(started_sessions) => {
            leaving::warn_on_stderr(&started_sessions);
            ExitCode::SUCCESS
        }
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
) -> io::Result<Vec<StartedSession>> {
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
                return Ok(run.started_sessions);
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
    /// The Sessions that have ended, to warn about any left unfinished.
    started_sessions: Vec<StartedSession>,
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
            let started = self.session.take().and_then(|session| session.started);
            app.stop_working();
            app.ui_state.confirming = None;
            app.ui_state.modal = None;
            if let Some(unfinished) = started.as_ref().and_then(|ended| ended.unfinished(false)) {
                app.view
                    .push_note(unfinished.warning().join(" "), Tone::Operator);
            }
            self.started_sessions.extend(started);
        }
    }

    /// What leaving now would leave unfinished: the running Session, or the last one that ended.
    fn leaving_warning(&self) -> Option<UnfinishedSession> {
        match &self.session {
            Some(session) => session.started.as_ref()?.unfinished(true),
            None => self
                .started_sessions
                .iter()
                .rev()
                .find_map(|ended| ended.unfinished(false)),
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
        let Some(RunningSession {
            messages,
            driver,
            mut started,
            ..
        }) = self.session.take()
        else {
            return;
        };
        let _ = driver.join();
        for message in messages.try_iter() {
            match message {
                DriverMessage::Started {
                    branch,
                    session_dir,
                } => started = Some(started_session(branch, &session_dir)),
                DriverMessage::Ended(_, tone) => mark_ended(&mut started, tone),
                _ => {}
            }
        }
        self.started_sessions.extend(started);
    }

    fn handle_key(&mut self, app: &mut App, project: &Project, key: KeyEvent) -> ControlFlow<()> {
        match app.ui_state.handle_key(key, &app.view) {
            KeyOutcome::Handled => {}
            KeyOutcome::Quit => {
                let leaving = self.leaving_warning();
                if self.session.is_none() && leaving.is_none() {
                    return ControlFlow::Break(());
                }
                app.ui_state.leaving = leaving;
                app.ui_state.confirming = Some(Confirmation::Quit);
            }
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
                "A Session is running; its Checkpoints and questions open in a modal.",
                Tone::Progress,
            ),
            KeyOutcome::Decided(decision) => self.send_decision(app, decision),
            KeyOutcome::Answered(answers) => self.send_answers(app, answers),
        }
        ControlFlow::Continue(())
    }

    fn take_awaited_reply(&mut self) -> Option<AwaitedReply> {
        self.session
            .as_mut()
            .and_then(|session| session.awaited_reply.take())
    }

    fn send_decision(&mut self, app: &mut App, decision: Decision) {
        if let Some(AwaitedReply::Decision(reply)) = self.take_awaited_reply() {
            app.view.push_note(decision_note(&decision), Tone::Operator);
            let _ = reply.send(decision);
        }
    }

    fn send_answers(&mut self, app: &mut App, answers: String) {
        if let Some(AwaitedReply::Answers(reply)) = self.take_awaited_reply() {
            app.view
                .push_note("Answered the Tier's questions.", Tone::Operator);
            let _ = reply.send(Some(answers));
        }
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
                session.started = Some(started_session(branch.clone(), &session_dir));
                app.status.branch = Some(branch);
                app.view.push_note(
                    format!("Session folder {}", session_dir.display()),
                    Tone::Progress,
                );
            }
            Ok(DriverMessage::Update(update)) => app.apply_update(update),
            Ok(DriverMessage::Checkpoint { checkpoint, reply }) => {
                app.stop_working();
                app.ui_state.open_modal(Modal::checkpoint(checkpoint));
                session.awaited_reply = Some(AwaitedReply::Decision(reply));
            }
            Ok(DriverMessage::Questions {
                tier,
                questions,
                reply,
            }) => {
                app.stop_working();
                app.ui_state.open_modal(Modal::questions(tier, questions));
                session.awaited_reply = Some(AwaitedReply::Answers(reply));
            }
            Ok(DriverMessage::Ended(message, tone)) => {
                mark_ended(&mut session.started, tone);
                app.view.push_note(message, tone);
                return false;
            }
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
}

fn started_session(branch: String, session_dir: &Path) -> StartedSession {
    StartedSession {
        branch,
        worktree: session_dir.join(WORKTREE_DIR),
        finished: false,
    }
}

/// Only a Session that ended with success is finished; any other ending leaves it unfinished.
fn mark_ended(started: &mut Option<StartedSession>, tone: Tone) {
    if let Some(started) = started {
        started.finished = tone == Tone::Success;
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
    let (sender, messages) = mpsc::channel();
    let (cancel, cancelled) = oneshot::channel();
    let root = project.root.clone();
    let config = project.config.clone();
    let driver = thread::spawn(move || {
        let ending = drive(&root, config, &api_key, request, &sender, cancelled);
        let _ = sender.send(ending);
    });
    Some(RunningSession {
        messages,
        cancel: Some(cancel),
        driver,
        awaited_reply: None,
        started: None,
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
    try_drive(root, config, api_key, request, sender, cancelled)
        .unwrap_or_else(|message| DriverMessage::Ended(message, Tone::Failure))
}

/// `Err` holds the message for a Session that couldn't start.
fn try_drive(
    root: &Path,
    config: Config,
    api_key: &str,
    request: Request,
    sender: &Sender<DriverMessage>,
    cancelled: oneshot::Receiver<()>,
) -> Result<DriverMessage, String> {
    let repo = Repo::open(root).map_err(|err| format!("Could not open the repository: {err}"))?;
    let mainline = repo
        .mainline(config.mainline.as_deref())
        .map_err(|err| format!("Could not find the mainline: {err}"))?;
    let branch = free_branch(&repo, &request.text)?;
    let session_dir = root.join(SESSIONS_DIR).join(session_id(&branch));
    exclude_sessions(&repo)
        .map_err(|message| format!("Could not keep {SESSIONS_DIR} out of git: {message}"))?;
    fs::create_dir_all(&session_dir)
        .map_err(|err| format!("Could not create {}: {err}", session_dir.display()))?;
    let worktree = repo
        .create_worktree(&mainline, &branch, &session_dir.join(WORKTREE_DIR))
        .map_err(|err| format!("Could not create the Session worktree: {err}"))?;
    let event_file = fs::File::create(session_dir.join("events.jsonl"))
        .map_err(|err| format!("Could not create the event log: {err}"))?;
    let _ = sender.send(DriverMessage::Started {
        branch: branch.as_str().to_owned(),
        session_dir: session_dir.clone(),
    });

    let client = OpenRouterClient::new(api_key);
    let observer_sender = sender.clone();
    let operator = ScreenOperator {
        sender: sender.clone(),
    };
    let mut autopilot = Autopilot::new(&worktree, &mainline, &client, config)
        .map_err(|err| err.to_string())?
        .with_operator(operator)
        .log_events_to(event_file)
        .on_update(move |update| {
            let _ = observer_sender.send(DriverMessage::Update(update));
        });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("Could not start the async runtime: {err}"))?;
    // Dropping the run future at a cancel kills a running Check (`kill_on_drop`); the worktree is
    // put back after that, below.
    let session_end = runtime.block_on(async {
        tokio::select! {
            session_end = autopilot.run(request) => Some(session_end),
            _ = cancelled => None,
        }
    });
    Ok(match session_end {
        Some(session_end) => ending(session_end),
        None => cancelled_ending(autopilot.session().phase(), &worktree),
    })
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
    let (message, tone) = match session_end {
        Ok(SessionEnd::Finished) => ("Session finished.".to_owned(), Tone::Success),
        Ok(SessionEnd::NeedsOperator(effect)) => (
            format!(
                "Stopped at {}. The branch and worktree are kept.",
                operator_step(&effect)
            ),
            Tone::Operator,
        ),
        Ok(SessionEnd::StoppedAtBudget) => (
            "Stopped at the Budget Cap. The branch and worktree are kept.".to_owned(),
            Tone::Operator,
        ),
        Ok(SessionEnd::TierStopped { tier, reason }) => {
            (format!("Stopped: the {tier:?} {reason}"), Tone::Failure)
        }
        Ok(SessionEnd::ScaffoldChecksFailed { check_output }) => (
            format!(
                "Stopped: the Scaffold Check failed: {}",
                check_output.lines().last().unwrap_or_default()
            ),
            Tone::Failure,
        ),
        Err(err) => (format!("Stopped: {err}"), Tone::Failure),
    };
    DriverMessage::Ended(message, tone)
}

/// The event log line for the Human Operator's decision at a Checkpoint.
fn decision_note(decision: &Decision) -> String {
    match decision {
        Decision::Approve => "Approved the Checkpoint.".to_owned(),
        Decision::ApproveTasks(task_order) => {
            let task_numbers: Vec<String> = task_order
                .iter()
                .map(|task| (task + 1).to_string())
                .collect();
            format!(
                "Approved the Plan with Tasks {} in that order.",
                task_numbers.join(", ")
            )
        }
        Decision::Reject(feedback) => format!("Rejected the Checkpoint: {feedback}"),
        Decision::Fixed => "Fixed it by hand; checking the fix.".to_owned(),
        Decision::Replan => "Sent the Task to the Planner.".to_owned(),
        Decision::Stop => "Stopped at the Checkpoint.".to_owned(),
        Decision::SetBudgetCap(Some(cap)) => {
            format!("Raised the Budget Cap to ${:.2}.", cap.dollars())
        }
        Decision::SetBudgetCap(None) => "Removed the Budget Cap.".to_owned(),
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
