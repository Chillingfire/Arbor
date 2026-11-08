mod common;

use arbor_git::Repo;
use common::{Local, write};

#[test]
fn a_clean_tree_has_no_changes() {
    let local = Local::new();
    let repo = Repo::open(local.path()).unwrap();
    assert!(repo.uncommitted_changes().unwrap().is_empty());
}

#[test]
fn a_dirty_tree_lists_modified_and_untracked_files() {
    let local = Local::new();
    write(local.path(), "README.md", "edited\n");
    write(local.path(), "notes/new.txt", "new\n");
    let repo = Repo::open(local.path()).unwrap();
    assert_eq!(
        repo.uncommitted_changes().unwrap(),
        ["README.md", "notes/new.txt"]
    );
}
