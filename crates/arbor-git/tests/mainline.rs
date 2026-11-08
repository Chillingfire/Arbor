mod common;

use arbor_git::{Fetch, Repo};
use common::{Cloned, Local, git};
use tempfile::tempdir;

#[test]
fn opening_a_directory_outside_any_repository_fails() {
    let dir = tempdir().unwrap();
    assert!(Repo::open(dir.path()).is_err());
}

#[test]
fn opening_a_subdirectory_finds_the_repository_root() {
    let repo = Local::new();
    common::write(repo.path(), "src/lib.rs", "");
    let opened = Repo::open(repo.path().join("src")).unwrap();
    assert_eq!(
        opened.root().canonicalize().unwrap(),
        repo.path().canonicalize().unwrap()
    );
}

#[test]
fn mainline_comes_from_origin_head() {
    let cloned = Cloned::with_default_branch("trunk");
    let repo = Repo::open(&cloned.clone).unwrap();
    assert_eq!(repo.fetch().unwrap(), Fetch::Origin);
    let mainline = repo.mainline(None).unwrap();
    assert_eq!(mainline.name(), "trunk");
    assert!(mainline.is_remote());
    assert_eq!(mainline.start_point(), "origin/trunk");
}

#[test]
fn a_chosen_mainline_overrides_origin_head() {
    let cloned = Cloned::with_default_branch("trunk");
    let repo = Repo::open(&cloned.clone).unwrap();
    repo.fetch().unwrap();
    let mainline = repo.mainline(Some("main")).unwrap();
    assert_eq!(mainline.name(), "main");
    assert_eq!(mainline.start_point(), "origin/main");
}

#[test]
fn a_chosen_mainline_that_does_not_exist_is_an_error() {
    let cloned = Cloned::new();
    let repo = Repo::open(&cloned.clone).unwrap();
    repo.fetch().unwrap();
    let err = repo.mainline(Some("no-such-branch")).unwrap_err();
    assert!(err.to_string().contains("no-such-branch"), "{err}");
}

#[test]
fn mainline_is_found_on_origin_when_origin_head_is_not_set_locally() {
    let cloned = Cloned::with_default_branch("trunk");
    let repo_dir = tempdir().unwrap();
    git(repo_dir.path(), &["init", "--quiet", "-b", "main"]);
    git(
        repo_dir.path(),
        &["remote", "add", "origin", cloned.origin.to_str().unwrap()],
    );
    let repo = Repo::open(repo_dir.path()).unwrap();
    repo.fetch().unwrap();
    assert_eq!(repo.mainline(None).unwrap().name(), "trunk");
}

#[test]
fn without_a_remote_the_local_mainline_is_used() {
    let local = Local::new();
    let repo = Repo::open(local.path()).unwrap();
    assert_eq!(repo.fetch().unwrap(), Fetch::NoRemote);
    let mainline = repo.mainline(None).unwrap();
    assert_eq!(mainline.name(), "main");
    assert!(!mainline.is_remote());
    assert_eq!(mainline.start_point(), "main");
    assert!(mainline.to_string().contains("no remote"), "{mainline}");
}

#[test]
fn a_chosen_mainline_must_be_a_plain_branch_name() {
    let local = Local::new();
    common::commit_file(local.path(), "a.txt", "a\n", "Second commit");
    let repo = Repo::open(local.path()).unwrap();
    assert!(repo.mainline(Some("main~1")).is_err());
    assert!(repo.mainline(Some("--help")).is_err());
}

#[test]
fn without_a_remote_a_chosen_local_branch_can_be_the_mainline() {
    let local = Local::new();
    git(local.path(), &["branch", "develop"]);
    let repo = Repo::open(local.path()).unwrap();
    assert_eq!(repo.mainline(Some("develop")).unwrap().name(), "develop");
}

#[test]
fn fetch_updates_origin_but_never_the_local_mainline() {
    let cloned = Cloned::new();
    let local_main = git(&cloned.clone, &["rev-parse", "main"]);
    let upstream = cloned.advance_origin("upstream.txt");

    let repo = Repo::open(&cloned.clone).unwrap();
    repo.fetch().unwrap();

    assert_eq!(git(&cloned.clone, &["rev-parse", "origin/main"]), upstream);
    assert_eq!(git(&cloned.clone, &["rev-parse", "main"]), local_main);
}

#[test]
fn branches_lists_the_candidates_for_mainline() {
    let cloned = Cloned::with_default_branch("trunk");
    let repo = Repo::open(&cloned.clone).unwrap();
    repo.fetch().unwrap();
    assert_eq!(repo.branches().unwrap(), ["main", "trunk"]);

    let local = Local::new();
    git(local.path(), &["branch", "develop"]);
    let repo = Repo::open(local.path()).unwrap();
    assert_eq!(repo.branches().unwrap(), ["develop", "main"]);
}
