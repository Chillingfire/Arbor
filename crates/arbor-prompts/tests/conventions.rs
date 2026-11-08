use std::fs;
use std::path::Path;

use arbor_prompts::{CONVENTIONS_CAP_TOKENS, Conventions, ConventionsError, estimate_tokens};
use tempfile::tempdir;

fn write(dir: &Path, path: &str, text: &str) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// `count` lines of about 100 bytes each.
fn long_lines(count: usize) -> String {
    (1..=count)
        .map(|n| format!("{n:04} {}\n", "x".repeat(95)))
        .collect()
}

#[test]
fn estimate_is_four_bytes_per_token_rounded_up() {
    assert_eq!(estimate_tokens(""), 0);
    assert_eq!(estimate_tokens("abcd"), 1);
    assert_eq!(estimate_tokens("abcde"), 2);
    assert_eq!(estimate_tokens("é"), 1);
}

#[test]
fn default_files_load_in_a_fixed_order() {
    let dir = tempdir().unwrap();
    write(dir.path(), "README.md", "readme\n");
    write(dir.path(), "CONTEXT.md", "context\n");
    write(dir.path(), "AGENTS.md", "agents\n");
    write(dir.path(), "CLAUDE.md", "claude\n");
    write(dir.path(), "NOTES.md", "not a default\n");

    let conventions = Conventions::load(dir.path(), &[]).unwrap();
    assert_eq!(
        conventions.text(),
        "<file path=\"AGENTS.md\">\nagents\n</file>\n\n\
         <file path=\"CLAUDE.md\">\nclaude\n</file>\n\n\
         <file path=\"CONTEXT.md\">\ncontext\n</file>\n\n\
         <file path=\"README.md\">\nreadme\n</file>\n"
    );
    assert_eq!(
        conventions.included(),
        ["AGENTS.md", "CLAUDE.md", "CONTEXT.md", "README.md"]
    );
}

#[test]
fn absent_files_are_skipped_and_an_empty_repo_has_no_conventions() {
    let dir = tempdir().unwrap();
    let conventions = Conventions::load(dir.path(), &[]).unwrap();
    assert!(conventions.is_empty());
    assert_eq!(conventions.text(), "");

    write(dir.path(), "CONTEXT.md", "context\n");
    let conventions = Conventions::load(dir.path(), &[]).unwrap();
    assert_eq!(conventions.included(), ["CONTEXT.md"]);
    assert!(conventions.missing().is_empty());
}

#[test]
fn a_readme_without_an_extension_counts() {
    let dir = tempdir().unwrap();
    write(dir.path(), "README", "plain readme\n");
    let conventions = Conventions::load(dir.path(), &[]).unwrap();
    assert_eq!(conventions.included(), ["README"]);
}

#[test]
fn context_files_follow_the_defaults_and_missing_ones_are_reported() {
    let dir = tempdir().unwrap();
    write(dir.path(), "AGENTS.md", "agents\n");
    write(dir.path(), "docs/style.md", "style\n");
    let extra = [
        "docs/style.md".to_owned(),
        "docs/gone.md".to_owned(),
        "AGENTS.md".to_owned(),
    ];

    let conventions = Conventions::load(dir.path(), &extra).unwrap();
    assert_eq!(conventions.included(), ["AGENTS.md", "docs/style.md"]);
    assert_eq!(conventions.missing(), ["docs/gone.md"]);
    assert!(
        conventions
            .text()
            .ends_with("<file path=\"docs/style.md\">\nstyle\n</file>\n")
    );
}

#[test]
fn context_files_outside_the_repository_are_refused() {
    let dir = tempdir().unwrap();
    let repo = dir.path().join("repo");
    write(dir.path(), "secret.md", "secret\n");
    fs::create_dir(&repo).unwrap();

    for path in [
        "../secret.md",
        dir.path().join("secret.md").to_str().unwrap(),
    ] {
        let err = Conventions::load(&repo, &[path.to_owned()]).unwrap_err();
        assert!(
            matches!(&err, ConventionsError::OutsideRepository { path: p } if p == path),
            "{err:?}"
        );
        assert!(err.to_string().contains(path), "{err}");
    }
}

#[test]
fn crlf_line_endings_are_normalised() {
    let dir = tempdir().unwrap();
    write(dir.path(), "AGENTS.md", "one\r\ntwo\r\n\r\n");
    let conventions = Conventions::load(dir.path(), &[]).unwrap();
    assert_eq!(
        conventions.text(),
        "<file path=\"AGENTS.md\">\none\ntwo\n</file>\n"
    );
}

#[test]
fn files_over_the_cap_are_truncated_at_a_line_and_later_ones_listed() {
    let dir = tempdir().unwrap();
    write(dir.path(), "AGENTS.md", "agents\n");
    write(dir.path(), "CONTEXT.md", &long_lines(400));
    write(dir.path(), "README.md", "readme\n");
    let extra = ["docs/more.md".to_owned()];
    write(dir.path(), "docs/more.md", "more\n");

    let conventions = Conventions::load(dir.path(), &extra).unwrap();
    let text = conventions.text();
    assert_eq!(conventions.included(), ["AGENTS.md", "CONTEXT.md"]);
    assert_eq!(conventions.truncated(), Some("CONTEXT.md"));
    assert_eq!(conventions.omitted(), ["README.md", "docs/more.md"]);
    assert!(text.contains("0001 x"));
    assert!(!text.contains("0400 x"));
    assert!(!text.contains("readme"));
    // Lines are kept whole.
    for line in text.lines().filter(|line| line.starts_with('0')) {
        assert_eq!(line.len(), 100, "{line}");
    }
    assert!(text.contains("[truncated: the Conventions cap was reached]"));
    assert!(
        text.ends_with(
            "Not shown because of the Conventions cap: README.md, docs/more.md. \
             Read them with read_file if you need them.\n"
        ),
        "{text}"
    );
    let shown = text.split("Not shown").next().unwrap();
    assert!(estimate_tokens(shown) <= CONVENTIONS_CAP_TOKENS);
    assert!(estimate_tokens(shown) > CONVENTIONS_CAP_TOKENS - 100);
}

#[test]
fn a_file_that_fills_the_cap_exactly_leaves_no_room_for_the_next() {
    let dir = tempdir().unwrap();
    write(dir.path(), "AGENTS.md", &long_lines(1000));
    write(dir.path(), "CLAUDE.md", "claude\n");
    let conventions = Conventions::load(dir.path(), &[]).unwrap();
    assert_eq!(conventions.truncated(), Some("AGENTS.md"));
    assert_eq!(conventions.omitted(), ["CLAUDE.md"]);
}
