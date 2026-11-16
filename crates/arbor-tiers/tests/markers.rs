use arbor_core::WorkUnitId;
use arbor_tiers::markers::{
    MarkerError, Markers, Region, is_begin_marker, is_marker_line, is_well_formed_marker,
    marker_lines_of, splice, strip,
};

fn wu(n: u32) -> WorkUnitId {
    WorkUnitId(n)
}

fn region(n: u32, begin: usize, end: usize) -> Region {
    Region {
        id: wu(n),
        begin,
        end,
    }
}

#[test]
fn markers_are_found_in_any_comment_syntax() {
    let cases = [
        "// ARBOR:BEGIN WU-7\n    todo!()\n// ARBOR:END WU-7\n",
        "# ARBOR:BEGIN WU-7\n    pass\n# ARBOR:END WU-7",
        "<!-- ARBOR:BEGIN WU-7 -->\n<p></p>\n<!-- ARBOR:END WU-7 -->\n",
        "/* ARBOR:BEGIN WU-7 */\r\nreturn 0;\r\n/* ARBOR:END WU-7 */\r\n",
        "-- ARBOR:BEGIN\tWU-7\nselect 1;\n-- ARBOR:END   WU-7-->\n",
    ];
    for source in cases {
        let markers = Markers::parse(source);
        assert_eq!(markers.regions(), [region(7, 0, 2)], "{source:?}");
        assert_eq!(markers.errors(), [], "{source:?}");
        assert_eq!(markers.region(wu(7)), Some(&region(7, 0, 2)));
        assert_eq!(markers.region(wu(7)).unwrap().body(), 1..2);
    }
}

#[test]
fn text_that_only_looks_like_a_marker_is_ignored() {
    let source = "ARBOR:BEGINNING WU-1\nlet arbor = \"ARBOR:END\";\nARBOR:ENDS\n";
    assert_eq!(Markers::parse(source), Markers::default());
}

#[test]
fn several_regions_in_one_file_are_found_in_order() {
    let source = "fn a() {\n  // ARBOR:BEGIN WU-1\n  todo!()\n  // ARBOR:END WU-1\n}\n\
                fn b() {\n  // ARBOR:BEGIN WU-2\n  todo!()\n  // ARBOR:END WU-2\n}\n";
    let markers = Markers::parse(source);
    assert_eq!(markers.regions(), [region(1, 1, 3), region(2, 6, 8)]);
    assert_eq!(markers.check(&[wu(1), wu(2)]), []);
    assert_eq!(markers.region(wu(3)), None);
}

#[test]
fn every_kind_of_broken_pair_is_reported() {
    let cases: [(&str, Vec<MarkerError>); 7] = [
        (
            "// ARBOR:BEGIN WU-1\ntodo!()\n",
            vec![MarkerError::Unclosed { id: wu(1), line: 0 }],
        ),
        (
            "todo!()\n// ARBOR:END WU-1\n",
            vec![MarkerError::Unopened { id: wu(1), line: 1 }],
        ),
        (
            "// ARBOR:BEGIN WU-1\n// ARBOR:END WU-2\n",
            vec![MarkerError::Mismatched {
                begin: wu(1),
                begin_line: 0,
                end: wu(2),
                end_line: 1,
            }],
        ),
        (
            "// ARBOR:BEGIN WU-1\n// ARBOR:BEGIN WU-2\n// ARBOR:END WU-2\n// ARBOR:END WU-1\n",
            vec![
                MarkerError::Nested {
                    outer: wu(1),
                    outer_line: 0,
                    inner: wu(2),
                    line: 1,
                },
                MarkerError::Unopened { id: wu(1), line: 3 },
            ],
        ),
        (
            "// ARBOR:BEGIN WU-1\n// ARBOR:END WU-1\n// ARBOR:BEGIN WU-1\n// ARBOR:END WU-1\n",
            vec![MarkerError::Duplicated {
                id: wu(1),
                lines: vec![0, 2],
            }],
        ),
        (
            "// ARBOR:BEGIN 7\n// ARBOR:END WU-x\n",
            vec![
                MarkerError::Malformed { line: 0 },
                MarkerError::Malformed { line: 1 },
            ],
        ),
        (
            "// ARBOR:BEGIN WU-7a\n",
            vec![MarkerError::Malformed { line: 0 }],
        ),
    ];
    for (source, expected) in cases {
        assert_eq!(Markers::parse(source).errors(), expected, "{source:?}");
    }
}

#[test]
fn a_duplicated_region_cannot_be_located() {
    let source = "// ARBOR:BEGIN WU-1\n// ARBOR:END WU-1\n// ARBOR:BEGIN WU-1\n// ARBOR:END WU-1\n";
    assert_eq!(Markers::parse(source).region(wu(1)), None);
}

#[test]
fn check_adds_a_missing_error_only_for_work_units_never_mentioned() {
    let source = "// ARBOR:BEGIN WU-1\n// ARBOR:END WU-1\n// ARBOR:BEGIN WU-2\n";
    assert_eq!(
        Markers::parse(source).check(&[wu(1), wu(2), wu(3)]),
        [
            MarkerError::Unclosed { id: wu(2), line: 2 },
            MarkerError::Missing { id: wu(3) },
        ]
    );
}

#[test]
fn errors_read_with_one_based_line_numbers() {
    let cases = [
        (
            MarkerError::Malformed { line: 0 },
            "line 1: a Marker needs a Work Unit ID, as in `ARBOR:BEGIN WU-7`",
        ),
        (
            MarkerError::Unclosed { id: wu(3), line: 4 },
            "WU-3: ARBOR:BEGIN on line 5 has no ARBOR:END",
        ),
        (
            MarkerError::Unopened { id: wu(3), line: 4 },
            "WU-3: ARBOR:END on line 5 has no ARBOR:BEGIN",
        ),
        (
            MarkerError::Mismatched {
                begin: wu(1),
                begin_line: 0,
                end: wu(2),
                end_line: 9,
            },
            "ARBOR:BEGIN WU-1 on line 1 is closed by ARBOR:END WU-2 on line 10",
        ),
        (
            MarkerError::Nested {
                outer: wu(1),
                outer_line: 0,
                inner: wu(2),
                line: 2,
            },
            "ARBOR:BEGIN WU-2 on line 3 is inside WU-1, opened on line 1; Regions can't nest",
        ),
        (
            MarkerError::Duplicated {
                id: wu(1),
                lines: vec![0, 5],
            },
            "WU-1 has more than one Region, on lines 1, 6",
        ),
        (
            MarkerError::Missing { id: wu(4) },
            "WU-4 has no ARBOR:BEGIN/ARBOR:END Marker pair",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}

const SKELETON: &str = "\
use std::fmt;

fn add(a: i32, b: i32) -> i32 {
    // ARBOR:BEGIN WU-3
    todo!()
    // ARBOR:END WU-3
}

fn sub(a: i32, b: i32) -> i32 {
";

#[test]
fn context_is_the_two_lines_just_outside_the_markers() {
    let region = *Markers::parse(SKELETON).region(wu(3)).unwrap();
    let lines: Vec<&str> = SKELETON.lines().collect();
    assert_eq!(
        lines[region.context_before()],
        ["", "fn add(a: i32, b: i32) -> i32 {"]
    );
    assert_eq!(lines[region.context_after(lines.len())], ["}", ""]);
}

#[test]
fn context_is_shorter_at_the_edges_of_the_file() {
    let source = "fn f() {\n// ARBOR:BEGIN WU-1\ntodo!()\n// ARBOR:END WU-1\n}";
    let region = *Markers::parse(source).region(wu(1)).unwrap();
    let lines: Vec<&str> = source.lines().collect();
    assert_eq!(lines[region.context_before()], ["fn f() {"]);
    assert_eq!(lines[region.context_after(lines.len())], ["}"]);

    let bare = "// ARBOR:BEGIN WU-1\ntodo!()\n// ARBOR:END WU-1\n";
    let region = *Markers::parse(bare).region(wu(1)).unwrap();
    let lines: Vec<&str> = bare.lines().collect();
    assert!(region.context_before().is_empty());
    assert!(region.context_after(lines.len()).is_empty());
}

#[test]
fn splice_replaces_only_the_body() {
    let region = *Markers::parse(SKELETON).region(wu(3)).unwrap();
    let spliced = splice(SKELETON, &region, &["    let sum = a + b;", "    sum"]);
    assert_eq!(
        spliced,
        SKELETON.replace("    todo!()\n", "    let sum = a + b;\n    sum\n")
    );
    // The Markers are still there, around the new body.
    assert_eq!(Markers::parse(&spliced).region(wu(3)).unwrap().body(), 4..6);
}

#[test]
fn splice_keeps_crlf_line_endings_and_allows_an_empty_body() {
    let source = "a\r\n// ARBOR:BEGIN WU-1\r\nold\r\n// ARBOR:END WU-1\r\nb";
    let region = *Markers::parse(source).region(wu(1)).unwrap();
    assert_eq!(
        splice(source, &region, &["x", "y"]),
        "a\r\n// ARBOR:BEGIN WU-1\r\nx\r\ny\r\n// ARBOR:END WU-1\r\nb"
    );
    assert_eq!(
        splice(source, &region, &[]),
        "a\r\n// ARBOR:BEGIN WU-1\r\n// ARBOR:END WU-1\r\nb"
    );
}

#[test]
fn strip_removes_only_that_work_units_markers() {
    let source = "\
// ARBOR:BEGIN WU-1
one
// ARBOR:END WU-1
// ARBOR:BEGIN WU-2
two
// ARBOR:END WU-2";
    let first = *Markers::parse(source).region(wu(1)).unwrap();
    let stripped = strip(source, &first);
    assert_eq!(stripped, "one\n// ARBOR:BEGIN WU-2\ntwo\n// ARBOR:END WU-2");
    let markers = Markers::parse(&stripped);
    assert_eq!(markers.regions(), [region(2, 1, 3)]);

    let second = *markers.region(wu(2)).unwrap();
    assert_eq!(strip(&stripped, &second), "one\ntwo\n");
}

#[test]
fn marker_lines_are_recognised() {
    assert!(is_marker_line("    // ARBOR:BEGIN WU-3"));
    assert!(is_marker_line("# ARBOR:END WU-3"));
    assert!(is_marker_line("// ARBOR:END WU-oops"));
    assert!(!is_marker_line("let sum = a + b;"));
    assert!(!is_marker_line("ARBOR:BEGINNING"));
}

#[test]
fn a_well_formed_marker_names_a_work_unit() {
    for line in [
        "    // ARBOR:BEGIN WU-3",
        "# ARBOR:END WU-12",
        "<!-- ARBOR:END WU-1 -->",
    ] {
        assert!(is_well_formed_marker(line), "{line}");
    }
    for line in [
        "// ARBOR:END WU-oops",
        "// ARBOR:BEGIN WU-<n>",
        "ARBOR:BEGINNING",
        "const BEGIN: &str = \"ARBOR:BEGIN\";",
        "let sum = a + b;",
    ] {
        assert!(!is_well_formed_marker(line), "{line}");
    }
}

#[test]
fn regions_need_two_lines_between_them() {
    let one_apart = "a\nb\n// ARBOR:BEGIN WU-1\nx\n// ARBOR:END WU-1\n}\n\
                     // ARBOR:BEGIN WU-2\ny\n// ARBOR:END WU-2\n}\nz\n";
    let error = MarkerError::TooClose {
        first: wu(1),
        end_line: 4,
        second: wu(2),
        begin_line: 6,
    };
    assert_eq!(
        Markers::parse(one_apart).errors(),
        std::slice::from_ref(&error)
    );
    assert_eq!(
        error.to_string(),
        "ARBOR:BEGIN WU-2 on line 7 is too close to ARBOR:END WU-1 on line 5; leave at least 2 \
         lines between Regions so neither one's context lines hold a Marker"
    );

    let two_apart = one_apart.replace("}\n// ARBOR:BEGIN WU-2", "}\n\n// ARBOR:BEGIN WU-2");
    assert_eq!(Markers::parse(&two_apart).errors(), []);
}

#[test]
fn marker_lines_of_finds_only_that_work_units_well_formed_markers() {
    let source = "// ARBOR:BEGIN WU-1
// ARBOR:END WU-2
x
// ARBOR:END WU-1
// ARBOR:END WU-1x
";
    assert_eq!(marker_lines_of(source, wu(1)), [0, 3]);
    assert_eq!(marker_lines_of(source, wu(3)), Vec::<usize>::new());
}

#[test]
fn only_a_well_formed_begin_opens_a_region() {
    assert!(is_begin_marker("    # ARBOR:BEGIN WU-4"));
    assert!(!is_begin_marker("    # ARBOR:END WU-4"));
    assert!(!is_begin_marker("    # ARBOR:BEGIN WU-"));
}
