use arbor_tiers::MarkerAllowlist;

fn allowlist(allowlist_text: &str) -> MarkerAllowlist {
    MarkerAllowlist::parse(allowlist_text).unwrap()
}

#[test]
fn a_missing_or_empty_allowlist_allows_nothing() {
    let empty = allowlist("# only a comment\n\n");
    assert!(!empty.allows("src/lib.rs"));
    assert!(!MarkerAllowlist::default().allows("README.md"));
}

#[test]
fn a_directory_pattern_covers_everything_inside_it() {
    let docs = allowlist("docs/\n");
    assert!(docs.allows("docs/adr/0002.md"));
    assert!(docs.allows("crates/a/docs/notes.md"));
    assert!(!docs.allows("docs"));
    assert!(!docs.allows("src/docs.rs"));
}

#[test]
fn a_pattern_with_a_slash_is_anchored_at_the_top_level() {
    let anchored = allowlist("crates/*/tests/\n/README.md\n");
    assert!(anchored.allows("crates/arbor/tests/fixtures/run.json"));
    assert!(!anchored.allows("vendor/crates/arbor/tests/a.rs"));
    assert!(anchored.allows("README.md"));
    assert!(!anchored.allows("docs/README.md"));
}

#[test]
fn a_name_without_a_slash_matches_at_any_depth() {
    let names = allowlist("*.snap\nfixtures\n");
    assert!(names.allows("tests/__snapshots__/view.snap"));
    assert!(names.allows("crates/arbor/tests/fixtures/run.json"));
    assert!(!names.allows("src/lib.rs"));
}

#[test]
fn a_double_star_spans_any_number_of_directories() {
    let deep = allowlist("crates/**/markers.rs\n");
    assert!(deep.allows("crates/markers.rs"));
    assert!(deep.allows("crates/arbor-tiers/src/markers.rs"));
    assert!(!deep.allows("src/markers.rs"));
}

#[test]
fn a_question_mark_matches_one_character() {
    let single = allowlist("docs/adr/000?-*.md\n");
    assert!(single.allows("docs/adr/0002-markers.md"));
    assert!(!single.allows("docs/adr/0010-writers.md"));
}

#[test]
fn a_pattern_that_allows_every_path_is_refused() {
    for everything in ["*", "**", "/", "/*", "**/*", "*/", "*/**", "?*"] {
        let refused = MarkerAllowlist::parse(&format!("docs/\n{everything}\n")).unwrap_err();
        assert_eq!(refused.line, 2, "{everything}");
        assert!(
            refused
                .to_string()
                .contains("would allow Markers everywhere"),
            "{refused}"
        );
    }
}

#[test]
fn a_negated_pattern_is_refused() {
    let refused = MarkerAllowlist::parse("!src/lib.rs\n").unwrap_err();
    assert!(refused.to_string().contains("negates"), "{refused}");
}
