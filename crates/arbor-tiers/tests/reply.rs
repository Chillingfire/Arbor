use arbor_tiers::reply::{CodeReply, ReplyError, Side, WriterReply, parse_reply};

fn code(language: &str, lines: &[&str]) -> WriterReply {
    WriterReply::Code(CodeReply {
        language: language.to_owned(),
        lines: lines.iter().map(|&line| line.to_owned()).collect(),
    })
}

fn code_reply(text: &str) -> CodeReply {
    match parse_reply(text) {
        Ok(WriterReply::Code(reply)) => reply,
        other => panic!("expected code, got {other:?}"),
    }
}

#[test]
fn a_single_code_block_is_read_line_by_line() {
    let cases = [
        (
            "```rust\na\n\n    b\n```",
            code("rust", &["a", "", "    b"]),
        ),
        ("\n\n```rust\na\n```\n\n", code("rust", &["a"])),
        ("```python title=x\na\n```", code("python", &["a"])),
        ("```\na\n```", code("", &["a"])),
        ("```rust\r\na\r\nb\r\n```\r\n", code("rust", &["a", "b"])),
        ("```rust\n```", code("rust", &[])),
        // A longer fence holds code that contains a shorter one.
        (
            "````markdown\n```sh\nls\n```\n````",
            code("markdown", &["```sh", "ls", "```"]),
        ),
        // A doc comment with a fence is not a fence.
        (
            "```rust\n/// ```\n/// x\n/// ```\nfn f() {}\n```",
            code("rust", &["/// ```", "/// x", "/// ```", "fn f() {}"]),
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(parse_reply(text), Ok(expected), "{text:?}");
    }
}

#[test]
fn a_missing_block_lists_one_item_per_line() {
    let cases = [
        (
            "```missing\nthe Account type\n\nthe signature of fn charge\n```",
            vec!["the Account type", "the signature of fn charge"],
        ),
        (
            "```Missing\n- one\n* two\n  three  \n```",
            vec!["one", "two", "three"],
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(
            parse_reply(text),
            Ok(WriterReply::Missing(
                expected.into_iter().map(str::to_owned).collect()
            )),
            "{text:?}"
        );
    }
}

#[test]
fn anything_else_is_rejected() {
    let cases = [
        ("", ReplyError::Empty),
        ("  \n ", ReplyError::Empty),
        ("fn add(a: i32) -> i32 { a }", ReplyError::NoBlock),
        ("``rust\na\n``", ReplyError::NoBlock),
        ("```rust\na\n", ReplyError::Unclosed),
        ("````rust\na\n```\n", ReplyError::Unclosed),
        (
            "```rust\na\n```\n```rust\nb\n```",
            ReplyError::SeveralBlocks(2),
        ),
        (
            "Here is the code:\n```rust\na\n```",
            ReplyError::TextOutside,
        ),
        ("```rust\na\n```\nHope this helps.", ReplyError::TextOutside),
        ("```missing\n\n  \n```", ReplyError::EmptyMissing),
        ("```missing\n- \n```", ReplyError::EmptyMissing),
    ];
    for (text, expected) in cases {
        assert_eq!(parse_reply(text), Err(expected), "{text:?}");
    }
}

#[test]
fn fit_returns_the_code_between_the_context_lines() {
    let reply = code_reply(
        "```rust\n\nfn add(a: i32, b: i32) -> i32 {\n    let sum = a + b;\n    sum\n}\n\n```",
    );
    let code = reply
        .fit(&["", "fn add(a: i32, b: i32) -> i32 {"], &["}", ""])
        .unwrap();
    assert_eq!(code, ["    let sum = a + b;", "    sum"]);
}

#[test]
fn fit_ignores_trailing_whitespace_but_not_indentation() {
    let reply = code_reply("```rust\nfn f() {  \nx\n}\t\n```");
    assert_eq!(reply.fit(&["fn f() {"], &["}"]), Ok(vec!["x"]));

    let reply = code_reply("```rust\n  fn f() {\nx\n}\n```");
    assert_eq!(
        reply.fit(&["fn f() {"], &["}"]),
        Err(ReplyError::ContextMismatch {
            side: Side::Before,
            expected: vec!["fn f() {".into()],
            found: vec!["  fn f() {".into()],
        })
    );
}

#[test]
fn fit_uses_only_the_context_the_file_has() {
    let reply = code_reply("```rust\nfn f() {\nx\n}\n```");
    assert_eq!(reply.fit(&["fn f() {"], &["}"]), Ok(vec!["x"]));
    let reply = code_reply("```rust\nx\n```");
    assert_eq!(reply.fit(&[], &[]), Ok(vec!["x"]));
    let reply = code_reply("```rust\na\nb\nc\nd\n```");
    assert_eq!(reply.fit(&["a", "b"], &["c", "d"]), Ok(vec![]));
}

#[test]
fn fit_rejects_wrong_or_missing_context() {
    let before = ["", "fn add() {"];
    let after = ["}", ""];
    let cases = [
        (
            "```rust\nfn add() {\nx\n}\n```",
            ReplyError::TooShort {
                needed: 4,
                found: 3,
            },
        ),
        (
            // The lines after the code were left out, so the code is read as context.
            "```rust\n\nfn add() {\n    a\n    b\n```",
            ReplyError::ContextMismatch {
                side: Side::After,
                expected: vec!["}".into(), "".into()],
                found: vec!["    a".into(), "    b".into()],
            },
        ),
        (
            // The Marker lines were taken as context.
            "```rust\nfn add() {\n    // ARBOR:BEGIN WU-3\n    x\n    // ARBOR:END WU-3\n}\n```",
            ReplyError::ContextMismatch {
                side: Side::Before,
                expected: vec!["".into(), "fn add() {".into()],
                found: vec!["fn add() {".into(), "    // ARBOR:BEGIN WU-3".into()],
            },
        ),
        (
            // The Marker lines were kept between the right context lines.
            "```rust\n\nfn add() {\n    // ARBOR:BEGIN WU-3\n    x\n    // ARBOR:END WU-3\n}\n\n```",
            ReplyError::MarkerInCode {
                line: "    // ARBOR:BEGIN WU-3".into(),
            },
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(
            code_reply(text).fit(&before, &after),
            Err(expected),
            "{text:?}"
        );
    }
}

#[test]
fn rejections_explain_what_to_send_instead() {
    let mismatch = ReplyError::ContextMismatch {
        side: Side::After,
        expected: vec!["}".into(), "".into()],
        found: vec!["    a".into(), "    b".into()],
    };
    assert_eq!(
        mismatch.to_string(),
        "the last 2 lines of the code block must be the lines just below the END Marker, \
         unchanged and without the Marker line.\nExpected:\n```\n}\n\n```\nFound:\n```\n    a\n    \
         b\n```"
    );
    assert!(
        ReplyError::TextOutside
            .to_string()
            .contains("only the block")
    );
    assert!(
        ReplyError::SeveralBlocks(3)
            .to_string()
            .contains("3 fenced blocks")
    );
}
