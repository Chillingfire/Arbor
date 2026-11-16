//! Reading the hunks of a unified diff, as `git diff` writes it.

#[derive(Debug, Clone, Copy)]
pub(crate) struct Hunk {
    pub(crate) old_lines: usize,
    pub(crate) new_start: usize,
    pub(crate) new_lines: usize,
}

/// Parses the rest of `@@ -1,3 +1,4 @@`, where a missing count means 1.
pub(crate) fn parse_hunk(header: &str) -> Option<Hunk> {
    let mut ranges = header.split_whitespace();
    let (_, old_lines) = parse_range(ranges.next()?.strip_prefix('-')?)?;
    let (new_start, new_lines) = parse_range(ranges.next()?.strip_prefix('+')?)?;
    Some(Hunk {
        old_lines,
        new_start,
        new_lines,
    })
}

fn parse_range(range: &str) -> Option<(usize, usize)> {
    match range.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((range.parse().ok()?, 1)),
    }
}

/// `b/src/lib.rs` gives `src/lib.rs`; a deleted file (`/dev/null`) gives `None`. Git ends a path
/// holding a space with a tab, and C-quotes one holding special bytes (`"b/\303\251.rs"`).
pub(crate) fn new_file_path(header: &str) -> Option<String> {
    let header = header.trim_end_matches(['\t', '\r', '\n']);
    let path = match header
        .strip_prefix('"')
        .and_then(|quoted| quoted.strip_suffix('"'))
    {
        Some(quoted) => unquote_c_style(quoted),
        None => header.to_owned(),
    };
    path.strip_prefix("b/").map(str::to_owned)
}

fn unquote_c_style(quoted: &str) -> String {
    let mut path_bytes = Vec::with_capacity(quoted.len());
    let mut bytes = quoted.bytes().peekable();
    while let Some(byte) = bytes.next() {
        if byte != b'\\' {
            path_bytes.push(byte);
            continue;
        }
        let Some(escaped) = bytes.next() else { break };
        let unescaped = match escaped {
            b'a' => 0x07,
            b'b' => 0x08,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 0x0b,
            b'f' => 0x0c,
            b'r' => b'\r',
            b'0'..=b'7' => {
                let mut octal = u32::from(escaped - b'0');
                for _ in 0..2 {
                    match bytes.peek() {
                        Some(digit @ b'0'..=b'7') => {
                            octal = octal * 8 + u32::from(digit - b'0');
                            bytes.next();
                        }
                        _ => break,
                    }
                }
                octal as u8
            }
            other => other,
        };
        path_bytes.push(unescaped);
    }
    String::from_utf8_lossy(&path_bytes).into_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HunkLineKind<'a> {
    /// The added line, without its `+`.
    Added(&'a str),
    Removed,
    Context,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HunkLine<'a> {
    pub(crate) kind: HunkLineKind<'a>,
    /// How many of the hunk's new-file lines come before this one.
    pub(crate) new_offset: usize,
}

/// Consumes exactly the hunk's lines, so an added line that starts with `++` is never taken for
/// a file header.
pub(crate) fn hunk_lines<'a>(lines: &mut std::str::Lines<'a>, hunk: Hunk) -> Vec<HunkLine<'a>> {
    let mut hunk_lines = Vec::new();
    let (mut old_left, mut new_left) = (hunk.old_lines, hunk.new_lines);
    let mut new_offset = 0;
    while old_left + new_left > 0 {
        let Some(line) = lines.next() else { break };
        let kind = match line.as_bytes().first() {
            Some(b'+') => HunkLineKind::Added(&line[1..]),
            Some(b'-') => HunkLineKind::Removed,
            Some(b'\\') => continue,
            _ => HunkLineKind::Context,
        };
        hunk_lines.push(HunkLine { kind, new_offset });
        match kind {
            HunkLineKind::Added(_) => {
                new_offset += 1;
                new_left = new_left.saturating_sub(1);
            }
            HunkLineKind::Removed => old_left = old_left.saturating_sub(1),
            HunkLineKind::Context => {
                new_offset += 1;
                old_left = old_left.saturating_sub(1);
                new_left = new_left.saturating_sub(1);
            }
        }
    }
    hunk_lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each hunk's `new_start` with its lines, skipping whatever lies between hunks as the
    /// callers do.
    fn parse_hunks(diff_text: &str) -> Vec<(usize, Vec<HunkLine<'_>>)> {
        let mut lines = diff_text.lines();
        let mut hunks = Vec::new();
        while let Some(line) = lines.next() {
            if let Some(header) = line.strip_prefix("@@ ") {
                let hunk = parse_hunk(header).expect("hunk header parses");
                hunks.push((hunk.new_start, hunk_lines(&mut lines, hunk)));
            }
        }
        hunks
    }

    fn line(kind: HunkLineKind<'_>, new_offset: usize) -> HunkLine<'_> {
        HunkLine { kind, new_offset }
    }

    #[test]
    fn missing_count_means_one_line() {
        let hunk = parse_hunk("-3 +4 @@ fn main()").expect("hunk header parses");
        assert_eq!((hunk.old_lines, hunk.new_start, hunk.new_lines), (1, 4, 1));
    }

    #[test]
    fn several_hunks_in_one_file_each_count_their_own_offsets() {
        let diff_text = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,3 @@
 first
-second
+changed
 third
@@ -10,2 +10,3 @@
 tenth
+inserted
 eleventh
";
        assert_eq!(
            parse_hunks(diff_text),
            vec![
                (
                    1,
                    vec![
                        line(HunkLineKind::Context, 0),
                        line(HunkLineKind::Removed, 1),
                        line(HunkLineKind::Added("changed"), 1),
                        line(HunkLineKind::Context, 2),
                    ]
                ),
                (
                    10,
                    vec![
                        line(HunkLineKind::Context, 0),
                        line(HunkLineKind::Added("inserted"), 1),
                        line(HunkLineKind::Context, 2),
                    ]
                ),
            ]
        );
    }

    #[test]
    fn no_newline_marker_inside_a_hunk_is_skipped() {
        let diff_text = "\
@@ -1 +1 @@
-old last line
\\ No newline at end of file
+new last line
\\ No newline at end of file
@@ -5 +5 @@
 after
";
        assert_eq!(
            parse_hunks(diff_text),
            vec![
                (
                    1,
                    vec![
                        line(HunkLineKind::Removed, 0),
                        line(HunkLineKind::Added("new last line"), 0),
                    ]
                ),
                (5, vec![line(HunkLineKind::Context, 0)]),
            ]
        );
    }

    #[test]
    fn no_newline_marker_after_a_hunk_is_left_for_the_caller() {
        let mut lines = "-old\n+new\n\\ No newline at end of file\n".lines();
        let hunk = parse_hunk("-1 +1 @@").expect("hunk header parses");
        assert_eq!(hunk_lines(&mut lines, hunk).len(), 2);
        assert_eq!(lines.next(), Some("\\ No newline at end of file"));
    }

    #[test]
    fn hunk_into_an_empty_file_has_no_old_lines() {
        let diff_text = "\
@@ -0,0 +1,2 @@
+one
+two
";
        assert_eq!(
            parse_hunks(diff_text),
            vec![(
                1,
                vec![
                    line(HunkLineKind::Added("one"), 0),
                    line(HunkLineKind::Added("two"), 1),
                ]
            )]
        );
    }

    #[test]
    fn hunk_emptying_a_file_has_no_new_lines() {
        let diff_text = "\
@@ -1,2 +0,0 @@
-one
-two
";
        assert_eq!(
            parse_hunks(diff_text),
            vec![(
                0,
                vec![
                    line(HunkLineKind::Removed, 0),
                    line(HunkLineKind::Removed, 0),
                ]
            )]
        );
    }

    #[test]
    fn added_line_starting_with_plus_plus_is_not_a_file_header() {
        let diff_text = "\
@@ -1 +1,2 @@
 counter
+++ b/not/a/header
";
        assert_eq!(
            parse_hunks(diff_text),
            vec![(
                1,
                vec![
                    line(HunkLineKind::Context, 0),
                    line(HunkLineKind::Added("++ b/not/a/header"), 1),
                ]
            )]
        );
    }

    #[test]
    fn new_file_path_drops_the_b_prefix() {
        assert_eq!(new_file_path("b/src/lib.rs"), Some("src/lib.rs".to_owned()));
        assert_eq!(new_file_path("/dev/null"), None);
    }

    #[test]
    fn new_file_path_drops_the_tab_git_writes_after_a_path_with_a_space() {
        assert_eq!(
            new_file_path("b/has space.rs\t"),
            Some("has space.rs".to_owned())
        );
    }

    #[test]
    fn new_file_path_decodes_a_c_quoted_path() {
        assert_eq!(
            new_file_path("\"b/\\303\\251.rs\""),
            Some("\u{e9}.rs".to_owned())
        );
        assert_eq!(
            new_file_path("\"b/tab\\there \\\"quoted\\\" back\\\\slash.rs\""),
            Some("tab\there \"quoted\" back\\slash.rs".to_owned())
        );
    }
}
