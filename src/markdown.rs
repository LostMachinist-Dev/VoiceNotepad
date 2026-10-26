//! Conversion between what is on screen and the markdown that leaves the app.
//!
//! Copy, Copy All and Save all go through [`to_markdown`]: display markers
//! become `-`, indentation stays four spaces per level, and everything else is
//! passed through untouched. No timestamps, no segment markers, nothing else
//! added -- the output is meant to paste straight into an LLM prompt.

use crate::document::{bullet_prefix, parse_line, INDENT_WIDTH, MAX_LEVEL};

/// Render display text as markdown.
///
/// Bullet lines become `- text`, indented four spaces per nesting level.
/// Non-bullet lines are copied verbatim, so an ordinary note stays plain text.
pub fn to_markdown(display: &str) -> String {
    let mut out = String::with_capacity(display.len());
    for (i, line) in display.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let parsed = parse_line(line);
        match parsed.level {
            Some(level) => {
                for _ in 0..level * INDENT_WIDTH {
                    out.push(' ');
                }
                out.push('-');
                // Keep an empty bullet as a bare "-" rather than "- ", so a
                // half-finished line does not leave trailing whitespace.
                if !parsed.text.is_empty() {
                    out.push(' ');
                    out.push_str(parsed.text);
                }
            }
            None => out.push_str(line),
        }
    }
    out
}

/// Read markdown bullets back into display markers.
///
/// Used when opening a `.md` file so a note written by this app round-trips.
/// Lines that are not list items are left exactly as they are.
pub fn from_markdown(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    for (i, line) in markdown.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match parse_list_item(line) {
            Some((level, text)) => {
                out.push_str(&bullet_prefix(level));
                out.push_str(text);
            }
            None => out.push_str(line),
        }
    }
    out
}

/// Recognise `-`, `*` or `+` list items and their nesting level.
fn parse_list_item(line: &str) -> Option<(usize, &str)> {
    let spaces = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[spaces..];
    let marker = rest.chars().next()?;
    if !matches!(marker, '-' | '*' | '+') {
        return None;
    }
    let after = &rest[1..];
    // "- text", or a bare "-" for an empty bullet.
    let text = if after.is_empty() {
        ""
    } else {
        after.strip_prefix(' ')?
    };
    Some(((spaces / INDENT_WIDTH).min(MAX_LEVEL), text))
}

#[cfg(test)]
mod tests {
    use super::*;

    const B1: &str = "\u{2022}";
    const B2: &str = "\u{25E6}";
    const B3: &str = "\u{25AA}";

    #[test]
    fn plain_text_passes_through_unchanged() {
        let text = "just a note\nsecond line";
        assert_eq!(to_markdown(text), text);
    }

    #[test]
    fn top_level_bullet_becomes_a_dash() {
        assert_eq!(to_markdown(&format!("{B1} hello")), "- hello");
    }

    #[test]
    fn sublists_get_four_spaces_per_level() {
        let display = format!("{B1} one\n    {B2} two\n        {B3} three");
        assert_eq!(to_markdown(&display), "- one\n    - two\n        - three");
    }

    #[test]
    fn mixes_bullets_and_plain_lines() {
        let display = format!("Intro line\n{B1} point\n    {B2} detail\nOutro");
        assert_eq!(
            to_markdown(&display),
            "Intro line\n- point\n    - detail\nOutro"
        );
    }

    #[test]
    fn adds_no_timestamps_or_extra_formatting() {
        let display = format!("{B1} spoken words here");
        let md = to_markdown(&display);
        assert_eq!(md, "- spoken words here");
        assert!(!md.contains('['));
        assert!(!md.contains(':'));
    }

    #[test]
    fn empty_bullet_has_no_trailing_space() {
        assert_eq!(to_markdown(&format!("{B1} ")), "-");
        assert_eq!(to_markdown(B1), "-");
    }

    #[test]
    fn preserves_blank_lines_and_a_trailing_newline() {
        let display = format!("{B1} a\n\n{B1} b\n");
        assert_eq!(to_markdown(&display), "- a\n\n- b\n");
    }

    #[test]
    fn empty_document_converts_to_empty_string() {
        assert_eq!(to_markdown(""), "");
    }

    #[test]
    fn indented_plain_text_keeps_its_spaces() {
        // Not a bullet, so indentation must survive verbatim.
        assert_eq!(to_markdown("    code-ish line"), "    code-ish line");
    }

    // --- reading markdown back in -----------------------------------------

    #[test]
    fn markdown_round_trips_through_the_display_form() {
        let display = format!("Intro\n{B1} one\n    {B2} two\n        {B3} three\nOutro");
        let md = to_markdown(&display);
        assert_eq!(from_markdown(&md), display);
    }

    #[test]
    fn from_markdown_accepts_star_and_plus_bullets() {
        assert_eq!(from_markdown("* one"), format!("{B1} one"));
        assert_eq!(from_markdown("+ one"), format!("{B1} one"));
    }

    #[test]
    fn from_markdown_ignores_non_list_lines() {
        assert_eq!(from_markdown("# Heading"), "# Heading");
        assert_eq!(from_markdown("a - b"), "a - b");
        // A dash with no space after it is not a list item.
        assert_eq!(from_markdown("-5 degrees"), "-5 degrees");
    }

    #[test]
    fn from_markdown_handles_a_bare_dash() {
        assert_eq!(from_markdown("-"), format!("{B1} "));
    }
}
