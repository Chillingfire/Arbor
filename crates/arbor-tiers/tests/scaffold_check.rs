use arbor_config::Checks;
use arbor_core::{Priority, Snippet, WorkUnit, WorkUnitId};
use arbor_tiers::{
    NO_SCAFFOLD_CHECK, ScaffoldCheck, field_problems, marker_problems, writer_cap_report,
    writer_input_tokens,
};
use tempfile::TempDir;

fn unit(id: u32, file: &str) -> WorkUnit {
    WorkUnit {
        id: WorkUnitId(id),
        file: file.into(),
        spec: "Return the sum.".into(),
        context: vec![],
        acceptance_criteria: vec![],
        test_filter: format!("tests::wu_{id}"),
    }
}

/// A Work Unit whose spec alone is estimated at `tokens` before the margin.
fn sized(id: u32, tokens: usize) -> WorkUnit {
    WorkUnit {
        spec: "a".repeat(tokens * 5 / 2),
        ..unit(id, "src/lib.rs")
    }
}

const SKELETON: &str = "pub fn add(a: i32, b: i32) -> i32 {\n    // ARBOR:BEGIN WU-1\n    \
                        todo!()\n    // ARBOR:END WU-1\n}\n";

fn worktree(files: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().unwrap();
    for (path, text) in files {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn check(dir: &TempDir, scaffold: Option<&str>) -> ScaffoldCheck {
    let checks = Checks {
        scaffold: scaffold.map(str::to_owned),
        ..Checks::default()
    };
    ScaffoldCheck::new(dir.path(), checks, 8000, WorkUnitId(1))
}

#[test]
fn the_writer_input_estimate_counts_spec_criteria_and_context_pack_plus_ten_percent() {
    let unit = WorkUnit {
        spec: "abcde".into(),
        acceptance_criteria: vec!["abcde".into()],
        context: vec![Snippet {
            path: "a.rs".into(),
            text: "0123456789".into(),
            priority: Priority::Required,
        }],
        ..unit(1, "src/lib.rs")
    };
    // 2 + 2 + (2 + 4) = 10 tokens, and 10% on top.
    assert_eq!(writer_input_tokens(&unit), 11);
    assert_eq!(writer_input_tokens(&sized(1, 4000)), 4400);
    assert_eq!(writer_input_tokens(&sized(1, 1)), 2, "the margin rounds up");
}

#[test]
fn over_cap_work_units_are_reported_with_every_work_units_size() {
    assert_eq!(writer_cap_report(&[sized(6, 4000)], 4400), None);
    let report = writer_cap_report(&[sized(6, 4000), sized(7, 9000)], 8000).unwrap();
    assert_eq!(
        report,
        "Estimated Writer input per Work Unit against the Writer Cap, in tokens (10% margin \
         included):\n- WU-6: 4,400 / 8,000\n- WU-7: 9,900 / 8,000 — over, trim or split"
    );
}

#[test]
fn field_problems_are_all_listed() {
    let cases: [(Vec<WorkUnit>, &[&str]); 6] = [
        (vec![unit(1, "a.rs"), unit(2, "a.rs")], &[]),
        (vec![], &["there are no Work Units; submit at least one"]),
        (
            vec![unit(1, "a.rs"), unit(1, "b.rs")],
            &["WU-1 is used by more than one Work Unit"],
        ),
        (
            vec![unit(0, "a.rs")],
            &["WU-0 is taken by an earlier Task; number Work Units from WU-1"],
        ),
        (
            vec![unit(1, "../a.rs"), unit(2, "/etc/passwd")],
            &[
                "WU-1: file \"../a.rs\" must be a path inside the worktree, relative to its root",
                "WU-2: file \"/etc/passwd\" must be a path inside the worktree, relative to its \
                 root",
            ],
        ),
        (
            vec![WorkUnit {
                spec: " ".into(),
                test_filter: "--ignored".into(),
                ..unit(1, "a.rs")
            }],
            &[
                "WU-1 has no spec",
                "WU-1: a test filter may not start with -",
            ],
        ),
    ];
    for (units, expected) in cases {
        assert_eq!(field_problems(&units, WorkUnitId(1)), expected, "{units:?}");
    }
}

#[test]
fn marker_problems_name_the_file_and_include_regions_without_a_work_unit() {
    assert!(marker_problems("src/lib.rs", SKELETON, &[WorkUnitId(1)]).is_empty());
    assert_eq!(
        marker_problems("src/lib.rs", SKELETON, &[WorkUnitId(2)]),
        [
            "src/lib.rs: WU-2 has no ARBOR:BEGIN/ARBOR:END Marker pair",
            "src/lib.rs: WU-1 has a Region here, but no submitted Work Unit names this file",
        ]
    );
    let doubled = format!("{SKELETON}{SKELETON}");
    assert_eq!(
        marker_problems("src/lib.rs", &doubled, &[WorkUnitId(1)]),
        ["src/lib.rs: WU-1 has more than one Region, on lines 2, 7"]
    );
}

#[test]
fn review_reads_each_work_units_file_and_combines_every_problem() {
    let dir = worktree(&[("src/lib.rs", SKELETON)]);
    let check = check(&dir, None);
    assert_eq!(check.review(&[unit(1, "src/lib.rs")]), Ok(()));

    let units = [sized(1, 9000), unit(2, "src/missing.rs")];
    assert_eq!(
        check.review(&units).unwrap_err(),
        "the Work Units were not accepted.\n\n\
         - src/missing.rs doesn't exist; write the Skeleton there before submitting\n\n\
         Estimated Writer input per Work Unit against the Writer Cap, in tokens (10% margin \
         included):\n- WU-1: 9,900 / 8,000 — over, trim or split\n- WU-2: 7 / 8,000"
    );
}

#[tokio::test]
async fn without_a_scaffold_check_only_the_review_runs_and_a_warning_is_offered() {
    let dir = worktree(&[("src/lib.rs", SKELETON)]);
    let check = check(&dir, None);
    assert_eq!(check.warning(), Some(NO_SCAFFOLD_CHECK));
    assert_eq!(check.run(&[unit(1, "src/lib.rs")]).await, Ok(()));
}

#[tokio::test]
async fn the_scaffold_check_runs_after_a_clean_review() {
    let dir = worktree(&[("src/lib.rs", SKELETON)]);
    let units = [unit(1, "src/lib.rs")];

    let passing = check(&dir, Some("cargo --version"));
    assert_eq!(passing.warning(), None);
    assert_eq!(passing.run(&units).await, Ok(()));

    let failing = check(&dir, Some("cargo no-such-subcommand-for-arbor"));
    let error = failing.run(&units).await.unwrap_err();
    assert!(
        error.starts_with("the Scaffold Check failed, so the Skeleton doesn't build."),
        "{error}"
    );
    assert!(error.contains("no-such-subcommand-for-arbor"), "{error}");

    let unpaired = [unit(2, "src/lib.rs")];
    let error = failing.run(&unpaired).await.unwrap_err();
    assert!(
        error.starts_with("the Work Units were not accepted."),
        "the check doesn't run while the review fails: {error}"
    );
}
