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

/// `b/src/lib.rs` gives `src/lib.rs`; a deleted file (`/dev/null`) gives `None`.
pub(crate) fn new_file_path(header: &str) -> Option<String> {
    let path = header.trim_end().trim_matches('"');
    path.strip_prefix("b/").map(str::to_owned)
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
