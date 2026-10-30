//! Tidying what Whisper returns before it reaches the document.
//!
//! The spec is strict about this: no timestamps, no speaker labels, no segment
//! markers, anywhere. ASR models emit annotations like `[BLANK_AUDIO]` or
//! `(music)` given half a chance, so the text is scrubbed on the way out and
//! nothing of that shape can reach the note.

/// Words that, alone in parentheses, are Whisper describing a sound rather
/// than transcribing speech.
const NON_SPEECH_WORDS: [&str; 20] = [
    "applause",
    "background",
    "blank",
    "blank_audio",
    "breathing",
    "buzzing",
    "clicking",
    "coughing",
    "crosstalk",
    "inaudible",
    "laughing",
    "laughter",
    "laughs",
    "music",
    "noise",
    "pause",
    "sighs",
    "silence",
    "static",
    "typing",
];

/// Clean one piece of transcribed text, collapsing all whitespace.
///
/// For text that arrives as a complete utterance. Streaming fragments must
/// use [`join_fragment`] instead, which preserves the spacing that tells
/// words apart from the middles of words.
pub fn clean_transcript(raw: &str) -> String {
    let stripped = strip_annotations(raw);
    let mut out = String::with_capacity(stripped.len());
    for word in stripped.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// Strip annotations but keep the spacing exactly as the model emitted it.
fn strip_annotations(raw: &str) -> String {
    let no_brackets = strip_spans(raw, '[', ']', |_| true);
    strip_spans(&no_brackets, '(', ')', is_non_speech)
}

/// Work out what to insert for one streaming fragment.
///
/// A streaming recogniser emits sub-word pieces, and the leading space on a
/// piece is the only thing distinguishing the start of a new word from the
/// middle of the current one: "polit" + "ical" is one word, "that" + " is"
/// is two. Trimming each piece and re-joining them with spaces -- which is
/// what the old whole-utterance logic did -- produced "polit ical" and
/// "fun ctions".
///
/// So the model's own spacing is kept, with one exception: at the start of a
/// line, or straight after a bullet marker, a leading space would show up as
/// an indent, so it is dropped.
pub fn join_fragment(preceding: Option<char>, fragment: &str) -> String {
    // Newlines and tabs from the model would break the document's own
    // structure, so they become ordinary spaces.
    let cleaned: String = strip_annotations(fragment)
        .chars()
        .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
        .collect();

    if cleaned.trim().is_empty() {
        return String::new();
    }
    match preceding {
        // Start of the document, start of a line, or just after a bullet
        // marker: a leading space would read as an indent.
        None => cleaned.trim_start().to_string(),
        Some(c) if c.is_whitespace() => cleaned.trim_start().to_string(),
        // Mid-line: whatever spacing the model chose is the right spacing.
        Some(_) => cleaned,
    }
}

/// Remove `open`..`close` spans for which `discard` returns true.
///
/// An unterminated opener is kept verbatim, so a stray bracket in dictation
/// cannot swallow the rest of the phrase.
fn strip_spans(text: &str, open: char, close: char, discard: impl Fn(&str) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(start) = rest.find(open) {
        let after_open = &rest[start + open.len_utf8()..];
        let Some(end) = after_open.find(close) else {
            break; // no closer: keep the remainder as-is
        };
        let inner = &after_open[..end];
        out.push_str(&rest[..start]);
        if !discard(inner) {
            out.push(open);
            out.push_str(inner);
            out.push(close);
        }
        rest = &after_open[end + close.len_utf8()..];
    }
    out.push_str(rest);
    out
}

/// Is this parenthesised text a sound rather than words?
fn is_non_speech(inner: &str) -> bool {
    let words: Vec<&str> = inner.split_whitespace().collect();
    if words.is_empty() || words.len() > 3 {
        return false;
    }
    words.iter().all(|w| {
        let w = w
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
            .to_ascii_lowercase();
        NON_SPEECH_WORDS.contains(&w.as_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_the_leading_space_whisper_always_adds() {
        assert_eq!(clean_transcript(" Hello there."), "Hello there.");
    }

    #[test]
    fn removes_blank_audio_annotations() {
        assert_eq!(clean_transcript("[BLANK_AUDIO]"), "");
        assert_eq!(clean_transcript(" [BLANK_AUDIO] "), "");
        assert_eq!(clean_transcript("[ Silence ] testing"), "testing");
    }

    #[test]
    fn removes_anything_that_looks_like_a_timestamp() {
        let raw = "[00:00:00.000 --> 00:00:02.000]  Testing the microphone.";
        assert_eq!(clean_transcript(raw), "Testing the microphone.");
    }

    #[test]
    fn output_never_contains_timestamps_or_segment_markers() {
        let raw = "[00:00:01.000 --> 00:00:03.000] first [BLANK_AUDIO] second (music) third";
        let cleaned = clean_transcript(raw);
        assert_eq!(cleaned, "first second third");
        assert!(!cleaned.contains("-->"));
        assert!(!cleaned.contains('['));
        assert!(!cleaned.contains(']'));
    }

    #[test]
    fn removes_parenthesised_sound_effects() {
        assert_eq!(clean_transcript("(music) let us begin"), "let us begin");
        assert_eq!(clean_transcript("hello (laughter)"), "hello");
        assert_eq!(clean_transcript("(background noise) ok"), "ok");
    }

    #[test]
    fn keeps_parentheses_that_are_part_of_the_dictation() {
        assert_eq!(
            clean_transcript("the result (see note three) was good"),
            "the result (see note three) was good"
        );
        assert_eq!(clean_transcript("press (A) to continue"), "press (A) to continue");
    }

    #[test]
    fn collapses_runs_of_whitespace_and_newlines() {
        assert_eq!(clean_transcript("one   two\n\nthree\t four"), "one two three four");
    }

    #[test]
    fn an_unclosed_bracket_does_not_eat_the_phrase() {
        assert_eq!(clean_transcript("testing [ unfinished"), "testing [ unfinished");
        assert_eq!(clean_transcript("a [b] c [d"), "a c [d");
    }

    #[test]
    fn empty_and_whitespace_only_input_give_empty_output() {
        assert_eq!(clean_transcript(""), "");
        assert_eq!(clean_transcript("   \n  "), "");
    }

    #[test]
    fn keeps_ordinary_punctuation_and_case() {
        assert_eq!(
            clean_transcript(" The quick brown fox, and so on."),
            "The quick brown fox, and so on."
        );
    }

    // --- joining streaming fragments ---------------------------------------

    #[test]
    fn a_fragment_continuing_a_word_gets_no_space() {
        // This is the bug that produced "polit ical" and "fun ctions".
        assert_eq!(join_fragment(Some('t'), "ical"), "ical");
        assert_eq!(join_fragment(Some('n'), "ctions"), "ctions");
    }

    #[test]
    fn the_models_own_leading_space_starts_a_new_word() {
        assert_eq!(join_fragment(Some('t'), " is"), " is");
        assert_eq!(join_fragment(Some('.'), " That"), " That");
    }

    #[test]
    fn a_leading_space_is_dropped_at_the_start_of_a_line() {
        assert_eq!(join_fragment(None, " First"), "First");
        assert_eq!(join_fragment(Some('\n'), " First"), "First");
        // Straight after a bullet marker, which ends in a space.
        assert_eq!(join_fragment(Some(' '), " First"), "First");
    }

    #[test]
    fn whitespace_only_fragments_insert_nothing() {
        assert_eq!(join_fragment(Some('a'), " "), "");
        assert_eq!(join_fragment(Some('a'), ""), "");
    }

    #[test]
    fn newlines_from_the_model_become_spaces() {
        // A stray newline would break the document's bullet structure.
        assert_eq!(join_fragment(Some('a'), "\nnext"), " next");
        assert!(!join_fragment(Some('a'), "one\ntwo").contains('\n'));
    }

    #[test]
    fn annotations_are_stripped_from_fragments_too() {
        assert_eq!(join_fragment(Some('a'), " [BLANK_AUDIO]"), "");
        assert_eq!(join_fragment(Some('a'), " word (music)"), " word ");
    }

    #[test]
    fn rebuilding_a_sentence_from_fragments_reads_correctly() {
        // Exactly the shape the recogniser emits.
        let fragments = ["A", " polit", "ical", " star", " is", " more", " reluct", "ant", "."];
        let mut out = String::new();
        for fragment in fragments {
            let piece = join_fragment(out.chars().last(), fragment);
            out.push_str(&piece);
        }
        assert_eq!(out, "A political star is more reluctant.");
    }
}
