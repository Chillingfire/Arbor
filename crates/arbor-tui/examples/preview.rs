//! Shows the TUI layout for a Session partway through its second Task, from a made-up event log.
//!
//! `cargo run -p arbor-tui --example preview` opens it in the terminal: Tab moves focus, ↑/↓ move
//! or scroll, ←/→ switch tabs, `q` quits. `-- --snapshot 120x36` prints one frame as text instead.

use std::collections::BTreeSet;

use arbor_config::ClarificationMode;
use arbor_core::{
    Event, Plan, Priority, Request, Snippet, Task, Tier, Usage, Usd, WorkUnit, WorkUnitId,
};
use arbor_tui::{
    DetailTab, Focus, KeyOutcome, ReplyOutcome, SessionView, StatusInfo, UiState,
    WriterReplyRecord, draw, tree_nodes,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{self, Event as TerminalEvent, KeyEventKind};

fn work_unit(id: u32, spec: &str, criterion: &str, test_filter: &str) -> WorkUnit {
    WorkUnit {
        id: WorkUnitId(id),
        file: "src/lib.rs".to_owned(),
        spec: spec.to_owned(),
        context: vec![
            Snippet {
                path: "src/lib.rs".to_owned(),
                text: "pub fn word_count(text: &str) -> usize {\n    text.split_whitespace().count()\n}\n"
                    .to_owned(),
                priority: Priority::Required,
            },
            Snippet {
                path: "README.md".to_owned(),
                text: "# wordy\nSmall helpers for words.\n".to_owned(),
                priority: Priority::Helpful,
            },
        ],
        acceptance_criteria: vec![criterion.to_owned()],
        test_filter: test_filter.to_owned(),
    }
}

fn usage(tier: Tier, input_tokens: u64, output_tokens: u64, cached: u64, dollars: f64) -> Event {
    Event::UsageRecorded {
        tier,
        usage: Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens: cached,
            cache_write_tokens: 0,
            cost: Usd::from_dollars(dollars),
        },
    }
}

fn demo_events() -> Vec<Event> {
    let plan = Plan {
        goal: "Add word statistics to wordy".to_owned(),
        constraints: vec![
            "Keep the public API free of allocation where a borrow will do".to_owned(),
            "Assumption: words are split on Unicode whitespace, as word_count does".to_owned(),
        ],
        tasks: vec![
            Task {
                title: "Add longest_word".to_owned(),
                description: "Return the longest word in a text, or None when it has none. \
                              Ties keep the first word."
                    .to_owned(),
            },
            Task {
                title: "Add unique_words".to_owned(),
                description: "Return the distinct words of a text in first-appearance order, \
                              comparing case-insensitively."
                    .to_owned(),
            },
        ],
    };
    let first_task_units = vec![
        work_unit(
            1,
            "Return the longest whitespace-separated word in `text`, keeping the first of \
             equally long words.",
            "`longest_word(\"cat dog\")` is `Some(\"cat\")`",
            "longest_word_tests",
        ),
        work_unit(
            2,
            "Document `longest_word` in the README's list of helpers.",
            "The README lists `longest_word`",
            "readme_tests",
        ),
    ];
    let second_task_units = vec![
        work_unit(
            3,
            "Lower-case each word to decide whether it was seen before.",
            "`seen(\"The\", \"the\")` is true",
            "unique_words_tests::seen",
        ),
        work_unit(
            4,
            "Return the distinct words of `text` in the order they first appear, keeping each \
             word's first spelling.",
            "`unique_words(\"b a B\")` is `[\"b\", \"a\"]`",
            "unique_words_tests",
        ),
        work_unit(
            5,
            "Re-export `unique_words` from the crate root next to `word_count`.",
            "`wordy::unique_words` resolves",
            "reexport_tests",
        ),
    ];
    vec![
        Event::SessionStarted {
            request: Request {
                text: "Add longest_word and unique_words to wordy.".to_owned(),
                issue: None,
            },
        },
        Event::PlanSubmitted { plan },
        usage(Tier::Planner, 14_200, 1_350, 9_800, 0.0251),
        Event::PlanApproved {
            task_order: vec![0, 1],
        },
        Event::WorkUnitsSubmitted {
            work_units: first_task_units,
        },
        usage(Tier::Scaffolder, 24_600, 2_100, 18_900, 0.0402),
        Event::ScaffoldChecksPassed,
        Event::ScaffoldApproved,
        Event::SkeletonCommitted {
            commit: Some("3f9c2a1d".to_owned()),
        },
        Event::WorkUnitPassed { id: WorkUnitId(1) },
        usage(Tier::Writer, 1_900, 420, 0, 0.0012),
        Event::WorkUnitCommitted {
            id: WorkUnitId(1),
            commit: "a71be04c".to_owned(),
        },
        Event::WorkUnitPassed { id: WorkUnitId(2) },
        usage(Tier::Writer, 1_400, 180, 0, 0.0008),
        Event::WorkUnitCommitted {
            id: WorkUnitId(2),
            commit: "c02d9e7a".to_owned(),
        },
        Event::TaskChecksPassed,
        Event::WorkUnitsSubmitted {
            work_units: second_task_units,
        },
        usage(Tier::Scaffolder, 27_300, 2_650, 22_100, 0.0447),
        Event::ScaffoldChecksPassed,
        Event::ScaffoldApproved,
        Event::SkeletonCommitted {
            commit: Some("e5a4410b".to_owned()),
        },
        Event::WorkUnitPassed { id: WorkUnitId(3) },
        usage(Tier::Writer, 1_700, 260, 0, 0.0009),
        Event::WorkUnitCommitted {
            id: WorkUnitId(3),
            commit: "9b81f3e2".to_owned(),
        },
    ]
}

/// What a live run records beside the events: WU-1's replies and its committed diff.
fn add_writer_details(view: &mut SessionView) {
    let first_unit = WorkUnitId(1);
    view.record_writer_reply(
        first_unit,
        WriterReplyRecord {
            reply: "```rust
    text.split_whitespace().max_by_key(|word| word.len())
```"
            .to_owned(),
            outcome: ReplyOutcome::Failed(
                "test longest_word_tests::keeps_the_first_of_tied_words ... FAILED
  left: Some(\"dog\")
 right: Some(\"cat\")"
                    .to_owned(),
            ),
        },
    );
    view.record_writer_reply(
        first_unit,
        WriterReplyRecord {
            reply: "```rust
    let mut longest: Option<&str> = None;
    for word in text.split_whitespace() {
        if longest.is_none_or(|current| word.len() > current.len()) {
            longest = Some(word);
        }
    }
    longest
```"
            .to_owned(),
            outcome: ReplyOutcome::Passed,
        },
    );
    view.set_diff(
        first_unit,
        "diff --git a/src/lib.rs b/src/lib.rs
index 3f9c2a1..a71be04 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -8,5 +8,11 @@ pub fn word_count(text: &str) -> usize {
 pub fn longest_word(text: &str) -> Option<&str> {
-    // ARBOR:BEGIN WU-1
-    todo!()
-    // ARBOR:END WU-1
+    let mut longest: Option<&str> = None;
+    for word in text.split_whitespace() {
+        if longest.is_none_or(|current| word.len() > current.len()) {
+            longest = Some(word);
+        }
+    }
+    longest
 }"
        .to_owned(),
    );
}

fn status_info() -> StatusInfo {
    StatusInfo {
        models: [
            "claude-opus-5.5".to_owned(),
            "claude-sonnet-5.5".to_owned(),
            "claude-haiku-5.5".to_owned(),
        ],
        clarification_mode: ClarificationMode::Ask,
        branch: Some("arbor/add-word-statistics".to_owned()),
        activity: None,
    }
}

fn main() -> std::io::Result<()> {
    let mut view = SessionView::from_events(&demo_events()).expect("the demo events fit a Session");
    add_writer_details(&mut view);
    let status = status_info();
    let preview_args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, size] = preview_args.as_slice()
        && flag == "--snapshot"
    {
        let (width, height) = size
            .split_once('x')
            .and_then(|(width, height)| Some((width.parse().ok()?, height.parse().ok()?)))
            .expect("the size looks like 120x36");
        let env_number = |name: &str| {
            std::env::var(name)
                .ok()
                .and_then(|number| number.parse::<usize>().ok())
                .unwrap_or(0)
        };
        let ui_state = UiState {
            selected: tree_nodes(&view, &BTreeSet::new())
                .get(env_number("PREVIEW_SELECTED"))
                .copied()
                .unwrap_or_default(),
            log_top: std::env::var("PREVIEW_LOG_TOP")
                .ok()
                .and_then(|top| top.parse().ok()),
            focus: match std::env::var("PREVIEW_FOCUS").as_deref() {
                Ok("detail") => Focus::Detail,
                Ok("log") => Focus::Log,
                Ok("prompt") => Focus::Prompt,
                _ => Focus::Tree,
            },
            detail_tab: match std::env::var("PREVIEW_TAB").as_deref() {
                Ok("transcript") => DetailTab::Transcript,
                Ok("diff") => DetailTab::Diff,
                _ => DetailTab::Spec,
            },
            prompt: std::env::var("PREVIEW_PROMPT").unwrap_or_default(),
            ..UiState::default()
        };
        let Ok(mut terminal) = Terminal::new(TestBackend::new(width, height));
        let Ok(_) = terminal.draw(|frame| draw(frame, &view, &ui_state, &status));
        println!("{}", terminal.backend());
        return Ok(());
    }

    let mut ui_state = UiState::default();
    ratatui::run(|terminal| -> std::io::Result<()> {
        loop {
            terminal.draw(|frame| draw(frame, &view, &ui_state, &status))?;
            if let TerminalEvent::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                match ui_state.handle_key(key, &view) {
                    KeyOutcome::Quit => return Ok(()),
                    KeyOutcome::Handled
                    | KeyOutcome::Submitted(_)
                    | KeyOutcome::Cancel
                    | KeyOutcome::Confirmed(_) => {}
                }
            }
        }
    })
}
