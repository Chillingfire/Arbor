use std::fs::{self, File};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};

use arbor_core::{Event, EventLog, Phase, Request, RunOutcome, Tier, Usage, Usd, rewind_points};
use arbor_git::{Mainline, Repo, SessionBranch};
use arbor_session::{
    SessionFolder, SessionStatus, delete_session, list_sessions, prune_sessions, rewind_session,
};
use tempfile::{TempDir, tempdir};

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

fn git(dir: &Path, git_args: &[&str]) -> String {
    let git_output = Command::new("git")
        .args(git_args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        git_output.status.success(),
        "git {git_args:?} failed: {}",
        String::from_utf8_lossy(&git_output.stderr)
    );
    String::from_utf8(git_output.stdout)
        .unwrap()
        .trim()
        .to_owned()
}

struct Project {
    dir: TempDir,
    repo: Repo,
    mainline: Mainline,
}

impl Project {
    fn new() -> Self {
        let dir = tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "--quiet", "-b", "main"]);
        git(root, &["config", "user.name", "Test Operator"]);
        git(root, &["config", "user.email", "operator@example.com"]);
        git(root, &["config", "commit.gpgsign", "false"]);
        git(root, &["config", "core.autocrlf", "false"]);
        fs::write(root.join("README.md"), "A project.\n").unwrap();
        fs::write(root.join(".gitignore"), "/.arbor/\n").unwrap();
        git(root, &["add", "--all"]);
        git(root, &["commit", "--quiet", "-m", "Start"]);
        let repo = Repo::open(root).unwrap();
        let mainline = repo.mainline(None).unwrap();
        Self {
            dir,
            repo,
            mainline,
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// A Session started `started_secs` after the epoch, with `events` in its log, last written
    /// `idle` ago.
    fn session(
        &self,
        feature: &str,
        started_secs: u64,
        events: &[Event],
        idle: Duration,
    ) -> SessionFolder {
        let branch = SessionBranch::for_feature(feature).unwrap();
        let started = SystemTime::UNIX_EPOCH + Duration::from_secs(started_secs);
        let folder = SessionFolder::for_new_session(self.root(), started, branch.as_str());
        fs::create_dir_all(folder.dir()).unwrap();
        self.repo
            .create_worktree(&self.mainline, &branch, &folder.worktree_path())
            .unwrap();
        let mut event_log = EventLog::new(File::create(folder.event_log_path()).unwrap());
        for event in events {
            event_log.append(event).unwrap();
        }
        drop(event_log);
        let event_file = File::options()
            .write(true)
            .open(folder.event_log_path())
            .unwrap();
        event_file.set_modified(SystemTime::now() - idle).unwrap();
        folder
    }
}

fn commit_in(folder: &SessionFolder, path: &str, file_text: &str) -> String {
    let worktree = folder.worktree_path();
    fs::write(worktree.join(path), file_text).unwrap();
    git(&worktree, &["add", "--all"]);
    git(&worktree, &["commit", "--quiet", "-m", "Change"]);
    git(&worktree, &["rev-parse", "HEAD"])
}

fn started(text: &str) -> Event {
    Event::SessionStarted {
        request: Request {
            text: text.to_owned(),
            issue: None,
        },
    }
}

fn spent(nanos: u64) -> Event {
    Event::UsageRecorded {
        tier: Tier::Planner,
        usage: Usage {
            cost: Usd::from_nanos(nanos),
            ..Usage::default()
        },
    }
}

fn cancelled() -> Event {
    Event::RunEnded {
        outcome: RunOutcome::Cancelled,
    }
}

#[test]
fn the_listing_shows_status_cost_age_and_time_until_pruned() {
    let project = Project::new();
    let cancelled_session = project.session(
        "Add a flag",
        1,
        &[started("Add a flag\nwith details"), spent(40), cancelled()],
        3 * DAY,
    );
    project.session("Crashed run", 2, &[started("Crashed run")], DAY);
    let running = project.session("Running one", 3, &[started("Running one")], Duration::ZERO);
    let _lock = running.lock_run().unwrap();

    let summaries = list_sessions(
        &project.repo,
        Some(&project.mainline),
        30,
        SystemTime::now(),
    )
    .unwrap();

    let statuses: Vec<&SessionStatus> = summaries.iter().map(|summary| &summary.status).collect();
    assert_eq!(statuses[0], &SessionStatus::Running);
    assert!(matches!(statuses[1], SessionStatus::Failed(_)));
    assert_eq!(statuses[2], &SessionStatus::Cancelled);
    let summary = summaries
        .iter()
        .find(|summary| summary.folder == cancelled_session)
        .unwrap();
    assert_eq!(summary.request, "Add a flag");
    assert_eq!(summary.branch, "arbor/add-a-flag");
    assert_eq!(summary.cost, Usd::from_nanos(40));
    assert_eq!(summary.phase, Some(Phase::Planning { feedback: None }));
    assert_eq!(summary.idle.as_secs() / DAY.as_secs(), 3);
    assert_eq!(
        summary.pruned_in.unwrap().as_secs() / DAY.as_secs(),
        26,
        "{:?}",
        summary.pruned_in
    );
    assert_eq!(summary.markers_at_tip, Some(0));
}

#[test]
fn the_listing_counts_markers_at_the_branch_tip() {
    let project = Project::new();
    let folder = project.session("Markers", 1, &[started("Markers")], DAY);
    commit_in(
        &folder,
        "lib.rs",
        "// ARBOR:BEGIN WU-1\ntodo!()\n// ARBOR:END WU-1\n",
    );
    let summaries =
        list_sessions(&project.repo, Some(&project.mainline), 0, SystemTime::now()).unwrap();
    assert_eq!(summaries[0].markers_at_tip, Some(2));
    assert_eq!(summaries[0].pruned_in, None);
}

#[test]
fn pruning_removes_old_sessions_but_keeps_their_branches() {
    let project = Project::new();
    let old = project.session("Old work", 1, &[started("Old work"), cancelled()], 40 * DAY);
    commit_in(&old, "lib.rs", "// ARBOR:BEGIN WU-2\n");
    let fresh = project.session("Fresh work", 2, &[started("Fresh work")], DAY);

    let report = prune_sessions(
        &project.repo,
        Some(&project.mainline),
        30,
        SystemTime::now(),
    );

    assert_eq!(report.pruned.len(), 1);
    assert!(!old.dir().exists());
    assert!(fresh.dir().exists());
    assert!(project.repo.branch_exists("arbor/old-work").unwrap());
    let notice = report.notice().unwrap();
    assert!(
        notice.contains("Pruned 1 Session idle over 30 days"),
        "{notice}"
    );
    assert!(notice.contains(&old.id()), "{notice}");
    assert!(
        notice.contains("still holding Markers: arbor/old-work (40 days old)"),
        "{notice}"
    );
    assert!(report.warnings().is_empty());
}

#[test]
fn pruning_skips_dirty_and_running_sessions_and_zero_retention_prunes_nothing() {
    let project = Project::new();
    let dirty = project.session("Dirty", 1, &[started("Dirty")], 40 * DAY);
    fs::write(dirty.worktree_path().join("scratch.txt"), "unsaved\n").unwrap();
    let running = project.session("Running", 2, &[started("Running")], 40 * DAY);
    let lock = running.lock_run().unwrap();

    let never = prune_sessions(&project.repo, Some(&project.mainline), 0, SystemTime::now());
    assert_eq!(never.notice(), None);

    let report = prune_sessions(
        &project.repo,
        Some(&project.mainline),
        30,
        SystemTime::now(),
    );
    drop(lock);

    assert!(report.pruned.is_empty());
    assert_eq!(report.notice(), None);
    assert!(dirty.dir().exists() && running.dir().exists());
    let warnings = report.warnings();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("uncommitted changes"), "{warnings:?}");
}

#[test]
fn deleting_removes_the_folder_worktree_and_branch_unless_running() {
    let project = Project::new();
    let folder = project.session("Delete me", 1, &[started("Delete me")], DAY);
    let lock = folder.lock_run().unwrap();
    assert!(delete_session(&project.repo, &folder).is_err());
    drop(lock);

    delete_session(&project.repo, &folder).unwrap();

    assert!(!folder.dir().exists());
    assert!(!project.repo.branch_exists("arbor/delete-me").unwrap());
}

#[test]
fn rewinding_resets_the_worktree_and_records_the_rewind() {
    let project = Project::new();
    let folder = project.session("Rewind", 1, &[], DAY);
    let start = git(&folder.worktree_path(), &["rev-parse", "HEAD"]);
    let skeleton = commit_in(&folder, "skeleton.rs", "fn skeleton() {}\n");
    let written = commit_in(&folder, "written.rs", "fn written() {}\n");
    let plan = arbor_core::Plan {
        goal: "Goal".into(),
        constraints: Vec::new(),
        tasks: vec![arbor_core::Task {
            title: "Task".into(),
            description: "Do it".into(),
        }],
    };
    let work_unit = arbor_core::WorkUnit {
        id: arbor_core::WorkUnitId(1),
        file: "written.rs".into(),
        spec: "Write it".into(),
        context: Vec::new(),
        acceptance_criteria: Vec::new(),
        test_filter: "written".into(),
    };
    let events = vec![
        started("Rewind"),
        Event::PlanSubmitted { plan },
        Event::PlanApproved {
            task_order: vec![0],
        },
        Event::WorkUnitsSubmitted {
            work_units: vec![work_unit],
        },
        Event::ScaffoldChecksPassed,
        Event::ScaffoldApproved,
        Event::SkeletonCommitted {
            commit: Some(skeleton.clone()),
        },
        Event::WorkUnitPassed {
            id: arbor_core::WorkUnitId(1),
        },
        Event::WorkUnitCommitted {
            id: arbor_core::WorkUnitId(1),
            commit: written,
        },
    ];
    let mut event_log = EventLog::new(folder.append_to_log().unwrap());
    for event in &events {
        event_log.append(event).unwrap();
    }
    drop(event_log);
    let points = rewind_points(&events).unwrap();
    let scaffold_point = &points[1];

    rewind_session(&folder, &events, scaffold_point).unwrap();

    let worktree = folder.worktree_path();
    assert_eq!(git(&worktree, &["rev-parse", "HEAD"]), start);
    assert!(worktree.join("skeleton.rs").exists());
    assert!(!worktree.join("written.rs").exists());
    let logged = folder.read_events().unwrap();
    assert_eq!(logged.len(), events.len() + 1);
    assert_eq!(
        logged.last(),
        Some(&Event::Rewound {
            event_count: scaffold_point.event_count
        })
    );
    let session = arbor_core::Session::replay(&logged).unwrap();
    assert_eq!(session.phase(), &Phase::ScaffoldCheckpoint { task: 0 });
}
