use std::fs;
use std::path::Path;
use std::process::Command;

use arbor_git::{Repo, TreeSource};
use arbor_tiers::{MarkerLine, scan_markers};
use tempfile::{TempDir, tempdir};

fn git(dir: &Path, args: &[&str]) -> String {
    let finished = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        finished.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&finished.stderr)
    );
    String::from_utf8_lossy(&finished.stdout).trim().to_owned()
}

/// A repository with a clean first commit on `main`.
struct Project {
    dir: TempDir,
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
        let project = Self { dir };
        project.write("src/lib.rs", "pub fn one() -> u32 {\n    1\n}\n");
        project.commit("Start");
        project
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.root().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn commit(&self, subject: &str) {
        git(self.root(), &["add", "--all"]);
        git(self.root(), &["commit", "--quiet", "-m", subject]);
    }

    fn scan(&self, source: TreeSource) -> Vec<String> {
        let repo = Repo::open(self.root()).unwrap();
        scan_markers(&repo, &source)
            .unwrap()
            .iter()
            .map(MarkerLine::to_string)
            .collect()
    }
}

const MARKED: &str = "pub fn two() -> u32 {\n    // ARBOR:BEGIN WU-3\n    todo!()\n    // \
                      ARBOR:END WU-3\n}\n";

#[test]
fn a_clean_tree_has_no_markers() {
    let project = Project::new();
    assert!(project.scan(TreeSource::WorkingTree).is_empty());
    assert!(project.scan(TreeSource::Staged).is_empty());
    assert!(project.scan(TreeSource::Revision("HEAD".into())).is_empty());
}

#[test]
fn the_working_tree_scan_lists_every_marker_line_as_file_line_text() {
    let project = Project::new();
    project.write("src/two.rs", MARKED);
    git(project.root(), &["add", "src/two.rs"]);
    project.write("src/two.rs", &format!("// moved down\n{MARKED}"));

    assert_eq!(
        project.scan(TreeSource::WorkingTree),
        [
            "src/two.rs:3: // ARBOR:BEGIN WU-3",
            "src/two.rs:5: // ARBOR:END WU-3"
        ]
    );
}

#[test]
fn untracked_files_are_left_out_of_the_working_tree_scan() {
    let project = Project::new();
    project.write("notes.rs", MARKED);
    assert!(project.scan(TreeSource::WorkingTree).is_empty());
}

#[test]
fn the_staged_scan_reads_the_index_not_the_disk() {
    let project = Project::new();
    project.write("src/two.rs", MARKED);
    git(project.root(), &["add", "src/two.rs"]);
    project.write("src/two.rs", "pub fn two() -> u32 {\n    2\n}\n");

    assert_eq!(project.scan(TreeSource::Staged).len(), 2);
    assert!(project.scan(TreeSource::WorkingTree).is_empty());
}

#[test]
fn the_revision_scan_reads_that_commits_tree() {
    let project = Project::new();
    project.write("src/two.rs", MARKED);
    project.commit("Add two");
    project.write("src/two.rs", "pub fn two() -> u32 {\n    2\n}\n");
    project.commit("Fill two");

    assert!(project.scan(TreeSource::Revision("HEAD".into())).is_empty());
    assert_eq!(
        project.scan(TreeSource::Revision("HEAD~1".into())),
        [
            "src/two.rs:2: // ARBOR:BEGIN WU-3",
            "src/two.rs:4: // ARBOR:END WU-3"
        ]
    );
}

#[test]
fn malformed_markers_count_but_lookalikes_do_not() {
    let project = Project::new();
    project.write(
        "src/odd.rs",
        "// ARBOR:BEGIN\n// ARBOR:END WU-x\n// ARBOR:BEGINNING WU-1\nlet arbor = \"ARBOR:\";\n",
    );
    project.commit("Odd");

    assert_eq!(
        project.scan(TreeSource::Revision("HEAD".into())),
        ["src/odd.rs:2: // ARBOR:END WU-x"]
    );
}

#[test]
fn crlf_lines_are_reported_without_the_carriage_return() {
    let project = Project::new();
    project.write("src/two.rs", "fn two() {\r\n  // ARBOR:BEGIN WU-1\r\n}\r\n");
    project.commit("CRLF");

    assert_eq!(
        project.scan(TreeSource::Revision("main".into())),
        ["src/two.rs:2: // ARBOR:BEGIN WU-1"]
    );
}

#[test]
fn a_revision_that_is_not_a_commit_or_looks_like_an_option_is_refused() {
    let project = Project::new();
    let repo = Repo::open(project.root()).unwrap();
    for revision in ["no-such-branch", "--output=x", ""] {
        let scanned = scan_markers(&repo, &TreeSource::Revision(revision.into()));
        assert!(scanned.is_err(), "{revision:?}");
    }
}

#[test]
fn only_a_begin_marker_opens_a_region() {
    let project = Project::new();
    project.write("src/two.rs", MARKED);
    let repo = Repo::open(project.root()).unwrap();
    let marker_lines = scan_markers(&repo, &TreeSource::WorkingTree).unwrap();
    assert!(marker_lines.is_empty());
    git(project.root(), &["add", "src/two.rs"]);
    let marker_lines = scan_markers(&repo, &TreeSource::WorkingTree).unwrap();
    let opening: Vec<bool> = marker_lines.iter().map(MarkerLine::opens_region).collect();
    assert_eq!(opening, [true, false]);
}
