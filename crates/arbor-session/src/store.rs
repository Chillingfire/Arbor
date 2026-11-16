//! The Sessions kept on disk under `.arbor/sessions/<id>/`: each folder holds the Session's
//! `events.jsonl`, its worktree, and while a run is going, a held `run.lock`. Everything shown
//! about a Session is derived from its event log (ADR-0004).

use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use arbor_core::{
    ApplyError, Event, EventLog, Phase, ReadError, RewindPoint, RunOutcome, Session, Usd,
    read_events,
};
use arbor_git::{CommitId, GitError, SESSIONS_DIR, Worktree};

const EVENT_LOG_FILE: &str = "events.jsonl";
const WORKTREE_DIR: &str = "worktree";
const RUN_LOCK_FILE: &str = "run.lock";
const BRANCH_PREFIX: &str = "arbor/";
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

#[derive(Debug)]
pub enum StoreError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Log(ReadError),
    Replay(ApplyError),
    Git(GitError),
    /// Another run holds the Session, in this arbor or another one.
    Running {
        id: String,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Log(err) => err.fmt(f),
            Self::Replay(err) => write!(f, "replaying the event log: {err}"),
            Self::Git(err) => err.fmt(f),
            Self::Running { id } => write!(f, "the Session {id} is running"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<GitError> for StoreError {
    fn from(err: GitError) -> Self {
        Self::Git(err)
    }
}

pub(crate) fn io_error(path: &Path) -> impl FnOnce(io::Error) -> StoreError + '_ {
    move |source| StoreError::Io {
        path: path.to_owned(),
        source,
    }
}

/// One Session's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFolder {
    dir: PathBuf,
}

impl SessionFolder {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// A new Session's folder, named by when it started and its branch, e.g.
    /// `1763200000-add-oauth-login`.
    pub fn for_new_session(project_root: &Path, started: SystemTime, branch: &str) -> Self {
        let started_secs = started
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since_epoch| since_epoch.as_secs());
        let slug = branch.trim_start_matches(BRANCH_PREFIX);
        Self::new(
            project_root
                .join(SESSIONS_DIR)
                .join(format!("{started_secs}-{slug}")),
        )
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The folder's name, which names the Session.
    pub fn id(&self) -> String {
        self.dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn event_log_path(&self) -> PathBuf {
        self.dir.join(EVENT_LOG_FILE)
    }

    pub fn worktree_path(&self) -> PathBuf {
        self.dir.join(WORKTREE_DIR)
    }

    pub fn read_events(&self) -> Result<Vec<Event>, StoreError> {
        let path = self.event_log_path();
        let event_file = File::open(&path).map_err(io_error(&path))?;
        read_events(BufReader::new(event_file)).map_err(StoreError::Log)
    }

    /// Opens the event log to add events after the ones already in it.
    pub fn append_to_log(&self) -> Result<File, StoreError> {
        let path = self.event_log_path();
        OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .map_err(io_error(&path))
    }

    /// When the Session's last event was written.
    pub fn last_event_at(&self) -> Result<SystemTime, StoreError> {
        let log_path = self.event_log_path();
        let path = if log_path.exists() {
            log_path
        } else {
            self.dir.clone()
        };
        fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .map_err(io_error(&path))
    }

    /// The Session branch: the worktree's, or else the one the folder is named after.
    pub fn branch(&self) -> String {
        match Worktree::open(self.worktree_path()) {
            Ok(worktree) => worktree.branch().to_owned(),
            Err(_) => {
                let id = self.id();
                let slug = id.split_once('-').map_or(id.as_str(), |(_, slug)| slug);
                format!("{BRANCH_PREFIX}{slug}")
            }
        }
    }

    /// Claims the Session for one run until the lock is dropped, so no other run, prune or
    /// delete touches it meanwhile.
    pub fn lock_run(&self) -> Result<RunLock, StoreError> {
        let path = self.dir.join(RUN_LOCK_FILE);
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(io_error(&path))?;
        match lock_file.try_lock() {
            Ok(()) => Ok(RunLock {
                _lock_file: lock_file,
            }),
            Err(TryLockError::WouldBlock) => Err(StoreError::Running { id: self.id() }),
            Err(TryLockError::Error(source)) => Err(StoreError::Io { path, source }),
        }
    }

    /// Whether a run holds the Session's lock.
    pub fn is_running(&self) -> bool {
        let Ok(lock_file) = File::open(self.dir.join(RUN_LOCK_FILE)) else {
            return false;
        };
        matches!(lock_file.try_lock(), Err(TryLockError::WouldBlock))
    }
}

/// A run's claim on its Session, released when dropped.
#[derive(Debug)]
pub struct RunLock {
    _lock_file: File,
}

/// Every Session folder under the project, newest first.
pub fn session_folders(project_root: &Path) -> Result<Vec<SessionFolder>, StoreError> {
    let sessions_dir = project_root.join(SESSIONS_DIR);
    let entries = match fs::read_dir(&sessions_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(StoreError::Io {
                path: sessions_dir,
                source,
            });
        }
    };
    let mut folders = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io_error(&sessions_dir))?;
        if entry.path().is_dir() {
            folders.push(SessionFolder::new(entry.path()));
        }
    }
    folders.sort_by_key(|folder| std::cmp::Reverse(folder.id()));
    Ok(folders)
}

/// How a Session's last run left it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Running,
    Cancelled,
    /// Why, when the log says.
    Failed(String),
    Finished,
}

/// What the Session browser shows for one Session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub folder: SessionFolder,
    pub branch: String,
    /// The Request's first line.
    pub request: String,
    pub status: SessionStatus,
    /// Where the Session waits, or `None` when its log can't be replayed.
    pub phase: Option<Phase>,
    pub cost: Usd,
    /// Time since the last event.
    pub idle: Duration,
    /// Time until startup prunes the Session, or `None` when retention is off.
    pub pruned_in: Option<Duration>,
    /// Marker lines the branch tip adds to the mainline, or `None` when unknown.
    pub markers_at_tip: Option<usize>,
}

impl SessionSummary {
    pub fn is_finished(&self) -> bool {
        self.status == SessionStatus::Finished
    }

    pub fn is_running(&self) -> bool {
        self.status == SessionStatus::Running
    }
}

/// The retention period, or `None` when `retention_days` is 0 and Sessions are kept forever.
pub fn retention(retention_days: u32) -> Option<Duration> {
    (retention_days > 0).then(|| Duration::from_secs(u64::from(retention_days) * SECONDS_PER_DAY))
}

/// Summarises the Session from its event log. `markers_at_tip` counts the Markers on a branch.
pub fn summarize(
    folder: SessionFolder,
    now: SystemTime,
    retention_days: u32,
    markers_at_tip: impl Fn(&str) -> Option<usize>,
) -> SessionSummary {
    let idle = folder
        .last_event_at()
        .ok()
        .and_then(|last_event_at| now.duration_since(last_event_at).ok())
        .unwrap_or_default();
    let pruned_in = retention(retention_days).map(|kept_for| kept_for.saturating_sub(idle));
    let branch = folder.branch();
    let markers = markers_at_tip(&branch);
    let replayed = folder.read_events().and_then(|events| {
        Ok((
            Session::replay(&events).map_err(StoreError::Replay)?,
            events,
        ))
    });
    let (request, phase, cost, logged_status) = match replayed {
        Ok((session, events)) => (
            session
                .request()
                .map(|request| first_line(&request.text).to_owned())
                .unwrap_or_default(),
            Some(session.phase().clone()),
            session.usage().total().cost,
            status_from_log(&session, &events),
        ),
        Err(err) => (
            String::new(),
            None,
            Usd::ZERO,
            SessionStatus::Failed(err.to_string()),
        ),
    };
    let status = if folder.is_running() {
        SessionStatus::Running
    } else {
        logged_status
    };
    SessionSummary {
        folder,
        branch,
        request,
        status,
        phase,
        cost,
        idle,
        pruned_in,
        markers_at_tip: markers,
    }
}

/// A log that ends without a `RunEnded`, and whose Session isn't done, belongs to a run that
/// was killed or crashed.
fn status_from_log(session: &Session, events: &[Event]) -> SessionStatus {
    match (session.phase(), events.last()) {
        (Phase::Done, _) => SessionStatus::Finished,
        (Phase::Stopped, _) => SessionStatus::Cancelled,
        (
            _,
            Some(Event::RunEnded {
                outcome: RunOutcome::Cancelled,
            }),
        ) => SessionStatus::Cancelled,
        (
            _,
            Some(Event::RunEnded {
                outcome: RunOutcome::Failed { reason },
            }),
        ) => SessionStatus::Failed(reason.clone()),
        _ => SessionStatus::Failed("the run ended without saying why".to_owned()),
    }
}

fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
}

/// Rewinds the Session to `point`: resets the worktree to that Checkpoint's commit, with the
/// edits it reviewed back in place uncommitted, and records the Rewind in the log. The events
/// after the Checkpoint stay in the log. Hold the Session's [`RunLock`] while calling this.
pub fn rewind_session(
    folder: &SessionFolder,
    events: &[Event],
    point: &RewindPoint,
) -> Result<(), StoreError> {
    let worktree = Worktree::open(folder.worktree_path())?;
    let head = match &point.head {
        Some(hash) => CommitId::from(hash.clone()),
        None => branch_start(&worktree, events)?,
    };
    match &point.reviewed_edits {
        Some(edits) => worktree.reset_with_uncommitted(&head, &CommitId::from(edits.clone()))?,
        None => worktree.reset_to(&head)?,
    }
    let rewound = Event::Rewound {
        event_count: point.event_count,
    };
    let event_log_path = folder.event_log_path();
    EventLog::new(folder.append_to_log()?)
        .append(&rewound)
        .map_err(io_error(&event_log_path))
}

/// Where the branch started: the parent of the first commit the log records, or `HEAD` when it
/// records none.
fn branch_start(worktree: &Worktree, events: &[Event]) -> Result<CommitId, StoreError> {
    let first_commit = events.iter().find_map(|event| match event {
        Event::SkeletonCommitted { commit: Some(hash) }
        | Event::RevisionCommitted { commit: Some(hash) }
        | Event::OperatorFixed { commit: Some(hash) }
        | Event::WorkUnitCommitted { commit: hash, .. } => Some(CommitId::from(hash.clone())),
        _ => None,
    });
    Ok(match first_commit {
        Some(first) => worktree.parent_of(&first)?,
        None => worktree.head()?,
    })
}
