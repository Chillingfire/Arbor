mod common;

use arbor_git::{Repo, SessionBranch, Worktree};
use common::{Cloned, Local, git};
use tempfile::tempdir;

#[test]
fn a_session_branch_is_named_from_a_short_feature_slug() {
    let branch = SessionBranch::for_feature("Add OAuth login!").unwrap();
    assert_eq!(branch.as_str(), "arbor/add-oauth-login");
    assert_eq!(
        SessionBranch::for_feature("  Fix   the  `parse()` bug ")
            .unwrap()
            .as_str(),
        "arbor/fix-the-parse-bug"
    );
}

#[test]
fn a_long_feature_is_cut_to_a_short_slug_at_a_word_boundary() {
    let branch = SessionBranch::for_feature(
        "Support exporting the session event log to a newline delimited json file",
    )
    .unwrap();
    let slug = branch.as_str().strip_prefix("arbor/").unwrap();
    assert!(slug.len() <= 40, "{slug}");
    assert!(!slug.ends_with('-'), "{slug}");
    assert!(slug.starts_with("support-exporting-the-session"), "{slug}");
}

#[test]
fn a_feature_with_nothing_to_slug_is_refused() {
    assert!(SessionBranch::for_feature("  !!! ").is_err());
}

#[test]
fn the_worktree_starts_from_the_fetched_origin_mainline() {
    let cloned = Cloned::new();
    let upstream = cloned.advance_origin("upstream.txt");
    let local_main = git(&cloned.clone, &["rev-parse", "main"]);
    let repo = Repo::open(&cloned.clone).unwrap();
    repo.fetch().unwrap();
    let mainline = repo.mainline(None).unwrap();

    let place = tempdir().unwrap();
    let path = place.path().join("session");
    let branch = SessionBranch::for_feature("Add a thing").unwrap();
    let worktree = repo.create_worktree(&mainline, &branch, &path).unwrap();

    assert_eq!(worktree.branch(), "arbor/add-a-thing");
    assert_eq!(worktree.head().unwrap().as_str(), upstream);
    assert!(path.join("upstream.txt").exists());
    assert_eq!(
        git(&path, &["branch", "--show-current"]),
        "arbor/add-a-thing"
    );
    // The operator's checkout and local mainline are untouched.
    assert_eq!(git(&cloned.clone, &["rev-parse", "main"]), local_main);
    assert_eq!(git(&cloned.clone, &["branch", "--show-current"]), "main");
}

#[test]
fn the_session_branch_does_not_track_the_mainline() {
    let cloned = Cloned::new();
    let repo = Repo::open(&cloned.clone).unwrap();
    repo.fetch().unwrap();
    let mainline = repo.mainline(None).unwrap();
    let place = tempdir().unwrap();
    let path = place.path().join("session");
    let branch = SessionBranch::for_feature("thing").unwrap();
    repo.create_worktree(&mainline, &branch, &path).unwrap();

    let upstream = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "arbor/thing@{upstream}"])
        .current_dir(&path)
        .output()
        .unwrap();
    assert!(!upstream.status.success());
}

#[test]
fn without_a_remote_the_worktree_starts_from_the_local_mainline() {
    let local = Local::new();
    let main = git(local.path(), &["rev-parse", "main"]);
    let repo = Repo::open(local.path()).unwrap();
    let mainline = repo.mainline(None).unwrap();
    let place = tempdir().unwrap();
    let path = place.path().join("session");
    let branch = SessionBranch::for_feature("local work").unwrap();

    let worktree = repo.create_worktree(&mainline, &branch, &path).unwrap();

    assert_eq!(worktree.head().unwrap().as_str(), main);
    assert_eq!(git(local.path(), &["rev-parse", "main"]), main);
}

#[test]
fn an_existing_session_branch_is_not_reused() {
    let local = Local::new();
    let repo = Repo::open(local.path()).unwrap();
    let mainline = repo.mainline(None).unwrap();
    let place = tempdir().unwrap();
    let branch = SessionBranch::for_feature("twice").unwrap();
    repo.create_worktree(&mainline, &branch, &place.path().join("one"))
        .unwrap();
    assert!(
        repo.create_worktree(&mainline, &branch, &place.path().join("two"))
            .is_err()
    );
}

#[test]
fn an_existing_worktree_can_be_opened_again() {
    let local = Local::new();
    let repo = Repo::open(local.path()).unwrap();
    let mainline = repo.mainline(None).unwrap();
    let place = tempdir().unwrap();
    let path = place.path().join("session");
    let branch = SessionBranch::for_feature("resume me").unwrap();
    let created = repo.create_worktree(&mainline, &branch, &path).unwrap();

    let opened = Worktree::open(&path).unwrap();

    assert_eq!(opened.branch(), "arbor/resume-me");
    assert_eq!(opened.head().unwrap(), created.head().unwrap());
}
