//! The text model: bullet parsing, indent/outdent, and cursor-relative edits.
//!
//! The on-screen string is the single source of truth. A "bullet line" is
//! recognised by its prefix -- some multiple of four spaces, a marker
//! character, then one space -- so every edit the user makes by hand stays
//! consistent with what the hotkeys produce, and nothing has to be kept in
//! sync with a parallel data structure.
//!
//! All public offsets are **character** indices, because that is what egui's
//! text cursor uses. Byte offsets stay inside this module; the markers are
//! multi-byte, so mixing the two is the one mistake that really matters here.

/// Spaces of indentation per nesting level, on screen and in markdown.
pub const INDENT_WIDTH: usize = 4;

/// How deep bullets may nest. Deeper than this and the markers stop being
/// distinguishable anyway.
pub const MAX_LEVEL: usize = 8;

/// Level 1 is a filled bullet, level 2 a hollow one, level 3 a square, then
/// the cycle repeats -- the same convention Word and Google Docs use.
pub const MARKERS: [char; 3] = ['\u{2022}', '\u{25E6}', '\u{25AA}'];

/// The marker character for a nesting level (0 = top level).
pub fn marker_for_level(level: usize) -> char {
    MARKERS[level % MARKERS.len()]
}

/// The full on-screen prefix for a bullet at `level`: indent, marker, space.
pub fn bullet_prefix(level: usize) -> String {
    format!(
        "{}{} ",
        " ".repeat(level * INDENT_WIDTH),
        marker_for_level(level)
    )
}

/// A single line, split into its bullet prefix (if any) and its content.
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedLine<'a> {
    /// `Some(level)` for a bullet line, `None` for ordinary text.
    pub level: Option<usize>,
    /// The content after the bullet prefix; the whole line when not a bullet.
    pub text: &'a str,
}

/// Split a line into its bullet level and its text.
///
/// Any marker character counts as a bullet regardless of which level its
/// indentation implies, so hand-edited indentation still round-trips.
pub fn parse_line(line: &str) -> ParsedLine<'_> {
    let spaces = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[spaces..];

    if let Some(marker) = rest.chars().next() {
        if MARKERS.contains(&marker) {
            let after = &rest[marker.len_utf8()..];
            let level = (spaces / INDENT_WIDTH).min(MAX_LEVEL);
            // A marker alone on the line is an empty bullet; otherwise the
            // marker must be followed by a space to count as one.
            if after.is_empty() {
                return ParsedLine {
                    level: Some(level),
                    text: "",
                };
            }
            if let Some(text) = after.strip_prefix(' ') {
                return ParsedLine {
                    level: Some(level),
                    text,
                };
            }
        }
    }

    ParsedLine {
        level: None,
        text: line,
    }
}

/// Re-render a bullet line one level deeper. `None` if the line is not a
/// bullet, or is already at [`MAX_LEVEL`].
pub fn indent_line(line: &str) -> Option<String> {
    let parsed = parse_line(line);
    let level = parsed.level?;
    if level >= MAX_LEVEL {
        return None;
    }
    Some(format!("{}{}", bullet_prefix(level + 1), parsed.text))
}

/// Re-render a bullet line one level shallower. `None` if the line is not a
/// bullet, or is already at the top level -- bullets are only ever created and
/// removed deliberately, so outdenting never silently strips one.
pub fn outdent_line(line: &str) -> Option<String> {
    let parsed = parse_line(line);
    let level = parsed.level?;
    if level == 0 {
        return None;
    }
    Some(format!("{}{}", bullet_prefix(level - 1), parsed.text))
}

// ---------------------------------------------------------------------------
// offset helpers
// ---------------------------------------------------------------------------

fn char_to_byte(text: &str, ci: usize) -> usize {
    text.char_indices()
        .nth(ci)
        .map(|(b, _)| b)
        .unwrap_or(text.len())
}

fn byte_to_char(text: &str, bi: usize) -> usize {
    text[..bi].chars().count()
}

/// Byte range of the line containing byte offset `b`, excluding the newline.
fn line_byte_range(text: &str, b: usize) -> (usize, usize) {
    let start = text[..b].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = text[b..].find('\n').map(|i| b + i).unwrap_or(text.len());
    (start, end)
}

/// 1-based line and column of the cursor, for the status bar.
pub fn line_col(text: &str, cursor: usize) -> (usize, usize) {
    let b = char_to_byte(text, cursor);
    let before = &text[..b];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = text[line_start..b].chars().count() + 1;
    (line, col)
}

// ---------------------------------------------------------------------------
// cursor-relative edits
// ---------------------------------------------------------------------------

/// The new-bullet hotkey: start a bullet at the cursor, at the same level as
/// the line the cursor is on, so pressing it on a sub-bullet continues at that
/// depth. Returns the new text and the cursor position after the marker.
///
/// A blank line is reused rather than pushed down, so the hotkey never leaves
/// an empty line behind.
pub fn insert_new_bullet(text: &str, cursor: usize) -> (String, usize) {
    let b = char_to_byte(text, cursor);
    let (line_start, line_end) = line_byte_range(text, b);
    let line = &text[line_start..line_end];
    let level = parse_line(line).level.unwrap_or(0);
    let prefix = bullet_prefix(level);

    let mut out = String::with_capacity(text.len() + prefix.len() + 1);
    let new_byte;
    if line.trim().is_empty() {
        out.push_str(&text[..line_start]);
        out.push_str(&prefix);
        new_byte = out.len();
        out.push_str(&text[line_end..]);
    } else {
        out.push_str(&text[..b]);
        out.push('\n');
        out.push_str(&prefix);
        new_byte = out.len();
        out.push_str(&text[b..]);
    }
    let pos = byte_to_char(&out, new_byte);
    (out, pos)
}

/// Enter on an empty bullet removes the marker instead of inserting a line.
/// `None` means "not an empty bullet", and the caller should let the normal
/// Enter happen.
pub fn enter_on_empty_bullet(text: &str, cursor: usize) -> Option<(String, usize)> {
    let b = char_to_byte(text, cursor);
    let (line_start, line_end) = line_byte_range(text, b);
    let line = &text[line_start..line_end];
    let parsed = parse_line(line);
    parsed.level?;
    if !parsed.text.trim().is_empty() {
        return None;
    }

    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..line_start]);
    let new_byte = out.len();
    out.push_str(&text[line_end..]);
    let pos = byte_to_char(&out, new_byte);
    Some((out, pos))
}

/// Tab / Shift+Tab over the cursor or selection.
///
/// Returns the new text and the new selection. `None` means no line in range
/// was an indentable bullet, and the caller should insert a literal tab (which
/// is what Notepad does).
pub fn reindent(
    text: &str,
    sel_start: usize,
    sel_end: usize,
    outdent: bool,
) -> Option<(String, usize, usize)> {
    let apply = |line: &str| {
        if outdent {
            outdent_line(line)
        } else {
            indent_line(line)
        }
    };

    let lo = sel_start.min(sel_end);
    let hi = sel_start.max(sel_end);
    let blo = char_to_byte(text, lo);

    // No selection: re-indent just this line and carry the cursor with it.
    if lo == hi {
        let (line_start, line_end) = line_byte_range(text, blo);
        let old = &text[line_start..line_end];
        let new = apply(old)?;

        let mut out = String::with_capacity(text.len() + INDENT_WIDTH);
        out.push_str(&text[..line_start]);
        out.push_str(&new);
        out.push_str(&text[line_end..]);

        let delta = new.chars().count() as isize - old.chars().count() as isize;
        let floor = byte_to_char(&out, line_start) as isize;
        let pos = (lo as isize + delta).max(floor) as usize;
        return Some((out, pos, pos));
    }

    // Selection: re-indent every line it touches, and keep it covering them.
    let bhi = char_to_byte(text, hi);
    let (first_start, _) = line_byte_range(text, blo);
    let (_, last_end) = line_byte_range(text, bhi);

    let mut changed = false;
    let block: Vec<String> = text[first_start..last_end]
        .split('\n')
        .map(|line| match apply(line) {
            Some(new) => {
                changed = true;
                new
            }
            None => line.to_string(),
        })
        .collect();
    if !changed {
        return None;
    }
    let block = block.join("\n");

    let mut out = String::with_capacity(text.len() + block.len());
    out.push_str(&text[..first_start]);
    out.push_str(&block);
    out.push_str(&text[last_end..]);

    let new_start = byte_to_char(&out, first_start);
    let new_end = new_start + block.chars().count();
    Some((out, new_start, new_end))
}

/// Insert `insertion` at `cursor`, returning the text and the cursor after it.
pub fn insert_at(text: &str, cursor: usize, insertion: &str) -> (String, usize) {
    let b = char_to_byte(text, cursor);
    let mut out = String::with_capacity(text.len() + insertion.len());
    out.push_str(&text[..b]);
    out.push_str(insertion);
    let new_byte = out.len();
    out.push_str(&text[b..]);
    let pos = byte_to_char(&out, new_byte);
    (out, pos)
}

/// Replace the characters in `range` with `replacement`.
///
/// This is how a streaming transcription updates its in-progress phrase in
/// place: the partial text occupies a known range, and each refinement swaps
/// that range for a better guess.
pub fn replace_range(
    text: &str,
    range: std::ops::Range<usize>,
    replacement: &str,
) -> (String, usize) {
    let start = char_to_byte(text, range.start);
    let end = char_to_byte(text, range.end).max(start);
    let mut out = String::with_capacity(text.len() + replacement.len());
    out.push_str(&text[..start]);
    out.push_str(replacement);
    let new_byte = out.len();
    out.push_str(&text[end..]);
    let pos = byte_to_char(&out, new_byte);
    (out, pos)
}

/// How a file's lines were terminated, so saving preserves what was loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    /// CRLF -- shown as "Windows (CRLF)", and the default for new documents.
    #[default]
    Crlf,
    /// LF -- shown as "Unix (LF)".
    Lf,
}

impl LineEnding {
    pub fn label(self) -> &'static str {
        match self {
            LineEnding::Crlf => "Windows (CRLF)",
            LineEnding::Lf => "Unix (LF)",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Crlf => "\r\n",
            LineEnding::Lf => "\n",
        }
    }

    /// Guess from file contents: any CR before an LF means CRLF.
    pub fn detect(text: &str) -> Self {
        if text.contains("\r\n") {
            LineEnding::Crlf
        } else if text.contains('\n') {
            LineEnding::Lf
        } else {
            LineEnding::default()
        }
    }
}

/// Normalise loaded text to LF, which is what the editor works in.
pub fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const B1: &str = "\u{2022}"; // level 1 marker
    const B2: &str = "\u{25E6}"; // level 2 marker
    const B3: &str = "\u{25AA}"; // level 3 marker

    #[test]
    fn plain_line_is_not_a_bullet() {
        let p = parse_line("just some text");
        assert_eq!(p.level, None);
        assert_eq!(p.text, "just some text");
    }

    #[test]
    fn indented_plain_text_is_not_a_bullet() {
        // Leading spaces alone must not be mistaken for a bullet.
        let p = parse_line("    indented but plain");
        assert_eq!(p.level, None);
        assert_eq!(p.text, "    indented but plain");
    }

    #[test]
    fn parses_bullet_levels() {
        assert_eq!(parse_line(&format!("{B1} top")).level, Some(0));
        assert_eq!(parse_line(&format!("    {B2} second")).level, Some(1));
        assert_eq!(parse_line(&format!("        {B3} third")).level, Some(2));
        assert_eq!(parse_line(&format!("    {B2} second")).text, "second");
    }

    #[test]
    fn parses_empty_bullet_with_and_without_trailing_space() {
        assert_eq!(parse_line(B1), ParsedLine { level: Some(0), text: "" });
        assert_eq!(
            parse_line(&format!("{B1} ")),
            ParsedLine { level: Some(0), text: "" }
        );
    }

    #[test]
    fn marker_without_following_space_is_plain_text() {
        let line = format!("{B1}notaspace");
        assert_eq!(parse_line(&line).level, None);
    }

    #[test]
    fn markers_cycle_every_three_levels() {
        assert_eq!(marker_for_level(0), '\u{2022}');
        assert_eq!(marker_for_level(1), '\u{25E6}');
        assert_eq!(marker_for_level(2), '\u{25AA}');
        assert_eq!(marker_for_level(3), '\u{2022}');
    }

    // --- indent / outdent --------------------------------------------------

    #[test]
    fn indent_nests_a_bullet_and_swaps_the_marker() {
        let got = indent_line(&format!("{B1} hello")).unwrap();
        assert_eq!(got, format!("    {B2} hello"));
    }

    #[test]
    fn indent_nests_several_levels_deep() {
        let mut line = format!("{B1} deep");
        for _ in 0..3 {
            line = indent_line(&line).unwrap();
        }
        assert_eq!(line, format!("            {B1} deep"));
        assert_eq!(parse_line(&line).level, Some(3));
    }

    #[test]
    fn outdent_is_the_inverse_of_indent() {
        let original = format!("    {B2} round trip");
        let there = indent_line(&original).unwrap();
        let back = outdent_line(&there).unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn indent_and_outdent_ignore_plain_lines() {
        assert_eq!(indent_line("plain"), None);
        assert_eq!(outdent_line("plain"), None);
    }

    #[test]
    fn outdent_stops_at_the_top_level() {
        assert_eq!(outdent_line(&format!("{B1} top")), None);
    }

    #[test]
    fn indent_stops_at_max_level() {
        let deep = format!("{}{} x", " ".repeat(MAX_LEVEL * INDENT_WIDTH), marker_for_level(MAX_LEVEL));
        assert_eq!(parse_line(&deep).level, Some(MAX_LEVEL));
        assert_eq!(indent_line(&deep), None);
    }

    // --- reindent over cursor and selection --------------------------------

    #[test]
    fn reindent_moves_the_cursor_with_the_text() {
        let text = format!("{B1} abc");
        // cursor at end of "abc" = 6 chars in ("• abc" is 5 chars... marker+space+abc)
        let cursor = text.chars().count();
        let (out, s, e) = reindent(&text, cursor, cursor, false).unwrap();
        assert_eq!(out, format!("    {B2} abc"));
        assert_eq!(s, e);
        // Four spaces were added before the cursor.
        assert_eq!(s, cursor + INDENT_WIDTH);
    }

    #[test]
    fn reindent_returns_none_when_no_bullet_is_in_range() {
        assert_eq!(reindent("plain text", 2, 2, false), None);
    }

    #[test]
    fn reindent_indents_every_bullet_in_a_selection() {
        let text = format!("{B1} one\n{B1} two\nplain");
        let (out, _, _) = reindent(&text, 0, text.chars().count(), false).unwrap();
        assert_eq!(out, format!("    {B2} one\n    {B2} two\nplain"));
    }

    #[test]
    fn reindent_leaves_surrounding_lines_alone() {
        let text = format!("before\n{B1} mid\nafter");
        let cursor = "before\n".chars().count() + 2;
        let (out, _, _) = reindent(&text, cursor, cursor, false).unwrap();
        assert_eq!(out, format!("before\n    {B2} mid\nafter"));
    }

    // --- new bullet hotkey -------------------------------------------------

    #[test]
    fn new_bullet_on_plain_text_opens_a_top_level_bullet() {
        let text = "some notes";
        let (out, pos) = insert_new_bullet(text, text.chars().count());
        assert_eq!(out, format!("some notes\n{B1} "));
        assert_eq!(pos, out.chars().count());
    }

    #[test]
    fn new_bullet_on_a_sub_bullet_stays_at_the_same_level() {
        let text = format!("    {B2} first");
        let (out, _) = insert_new_bullet(&text, text.chars().count());
        assert_eq!(out, format!("    {B2} first\n    {B2} "));
        // and the new line really parses back as level 1
        assert_eq!(parse_line(out.lines().last().unwrap()).level, Some(1));
    }

    #[test]
    fn new_bullet_reuses_a_blank_line_instead_of_pushing_one_down() {
        let text = "line\n";
        let (out, pos) = insert_new_bullet(text, text.chars().count());
        assert_eq!(out, format!("line\n{B1} "));
        assert_eq!(pos, out.chars().count());
    }

    #[test]
    fn new_bullet_splits_the_line_at_the_cursor() {
        let text = "abcdef";
        let (out, pos) = insert_new_bullet(text, 3);
        assert_eq!(out, format!("abc\n{B1} def"));
        assert_eq!(pos, "abc\n\u{2022} ".chars().count());
    }

    // --- enter on an empty bullet -----------------------------------------

    #[test]
    fn enter_clears_an_empty_bullet() {
        let text = format!("note\n{B1} ");
        let (out, pos) = enter_on_empty_bullet(&text, text.chars().count()).unwrap();
        assert_eq!(out, "note\n");
        assert_eq!(pos, out.chars().count());
    }

    #[test]
    fn enter_clears_an_empty_sub_bullet_including_its_indent() {
        let text = format!("note\n        {B3} ");
        let (out, _) = enter_on_empty_bullet(&text, text.chars().count()).unwrap();
        assert_eq!(out, "note\n");
    }

    #[test]
    fn enter_leaves_a_bullet_with_text_alone() {
        let text = format!("{B1} has words");
        assert_eq!(enter_on_empty_bullet(&text, text.chars().count()), None);
    }

    #[test]
    fn enter_leaves_plain_lines_alone() {
        assert_eq!(enter_on_empty_bullet("plain", 5), None);
    }

    // --- offsets around multi-byte markers ---------------------------------

    #[test]
    fn cursor_offsets_are_characters_not_bytes() {
        // "• x" is 3 chars but 5 bytes; getting this wrong would panic or
        // insert mid-character.
        let text = format!("{B1} x");
        assert_eq!(text.chars().count(), 3);
        assert_eq!(text.len(), 5);
        let (out, pos) = insert_at(&text, 3, "y");
        assert_eq!(out, format!("{B1} xy"));
        assert_eq!(pos, 4);
    }

    #[test]
    fn line_col_counts_characters() {
        let text = format!("{B1} abc\nsecond");
        assert_eq!(line_col(&text, 0), (1, 1));
        assert_eq!(line_col(&text, 5), (1, 6));
        assert_eq!(line_col(&text, 6), (2, 1));
    }

    #[test]
    fn replace_range_swaps_a_partial_phrase_in_place() {
        let text = format!("{B1} hello wor");
        let start = 2;
        let end = text.chars().count();
        let (out, pos) = replace_range(&text, start..end, "hello world");
        assert_eq!(out, format!("{B1} hello world"));
        assert_eq!(pos, out.chars().count());
    }

    #[test]
    fn replace_range_tolerates_an_out_of_range_end() {
        let (out, _) = replace_range("abc", 1..999, "X");
        assert_eq!(out, "aX");
    }

    // --- line endings ------------------------------------------------------

    #[test]
    fn detects_and_normalises_line_endings() {
        assert_eq!(LineEnding::detect("a\r\nb"), LineEnding::Crlf);
        assert_eq!(LineEnding::detect("a\nb"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("no newline"), LineEnding::Crlf);
        assert_eq!(normalize_newlines("a\r\nb\rc\nd"), "a\nb\nc\nd");
    }
}
