//! Tidying what Whisper returns before it reaches the document.
//!
//! The spec is strict about this: no timestamps, no speaker labels, no segment
//! markers, anywhere. Whisper will happily emit `[BLANK_AUDIO]`, `(music)` and
//! -- if asked, or if a stray flag changes -- bracketed time ranges. Rather
//! than rely only on getting every parameter right, the text is scrubbed here
//! too, so nothing of that shape can reach the note.

use crate::resample::WHISPER_SAMPLE_RATE;

/// Whisper is unreliable on very short clips, so anything shorter than this is
/// padded with silence. 1.1 s leaves comfortable margin over the 1 s minimum.
const MIN_INFERENCE_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 11 / 10;

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

/// Clean one piece of transcribed text.
///
/// Drops anything in square brackets outright -- Whisper uses them only for
/// annotations -- drops parenthesised non-speech noises, and collapses
/// whitespace. Ordinary parenthetical speech is left alone.
pub fn clean_transcript(raw: &str) -> String {
    let no_brackets = strip_spans(raw, '[', ']', |_| true);
    let no_parens = strip_spans(&no_brackets, '(', ')', is_non_speech);

    let mut out = String::with_capacity(no_parens.len());
    for word in no_parens.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
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

/// Pad a short clip with silence so Whisper will accept it.
///
/// Returns a borrow when no padding is needed, so the common case copies
/// nothing.
pub fn pad_for_inference(samples: &[f32]) -> std::borrow::Cow<'_, [f32]> {
    if samples.len() >= MIN_INFERENCE_SAMPLES {
        return std::borrow::Cow::Borrowed(samples);
    }
    let mut padded = Vec::with_capacity(MIN_INFERENCE_SAMPLES);
    padded.extend_from_slice(samples);
    padded.resize(MIN_INFERENCE_SAMPLES, 0.0);
    std::borrow::Cow::Owned(padded)
}

/// Join a committed phrase onto what is already in the note.
///
/// Inserts a single separating space when the preceding character needs one,
/// so successive phrases read as prose rather than running together. Returns
/// the text to insert, which may be empty.
pub fn phrase_with_separator(preceding: Option<char>, phrase: &str) -> String {
    let phrase = phrase.trim();
    if phrase.is_empty() {
        return String::new();
    }
    match preceding {
        // Start of the document, start of a line, or just after a bullet
        // marker: no separator wanted.
        None => phrase.to_string(),
        Some(c) if c.is_whitespace() => phrase.to_string(),
        Some(_) => format!(" {phrase}"),
    }
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

    // --- padding -----------------------------------------------------------

    #[test]
    fn short_clips_are_padded_to_the_whisper_minimum() {
        let short = vec![0.1f32; 100];
        let padded = pad_for_inference(&short);
        assert_eq!(padded.len(), MIN_INFERENCE_SAMPLES);
        assert_eq!(padded[0], 0.1);
        assert_eq!(padded[MIN_INFERENCE_SAMPLES - 1], 0.0);
    }

    #[test]
    fn long_clips_are_passed_through_without_copying() {
        let long = vec![0.1f32; MIN_INFERENCE_SAMPLES * 2];
        let out = pad_for_inference(&long);
        assert_eq!(out.len(), long.len());
        assert!(matches!(out, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn an_empty_clip_pads_to_silence() {
        let out = pad_for_inference(&[]);
        assert_eq!(out.len(), MIN_INFERENCE_SAMPLES);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    // --- joining phrases ---------------------------------------------------

    #[test]
    fn a_phrase_after_a_word_gets_one_space() {
        assert_eq!(phrase_with_separator(Some('.'), "Next thought"), " Next thought");
        assert_eq!(phrase_with_separator(Some('d'), "and then"), " and then");
    }

    #[test]
    fn a_phrase_after_whitespace_or_at_the_start_gets_none() {
        assert_eq!(phrase_with_separator(None, "First"), "First");
        assert_eq!(phrase_with_separator(Some(' '), "First"), "First");
        assert_eq!(phrase_with_separator(Some('\n'), "First"), "First");
    }

    #[test]
    fn an_empty_phrase_inserts_nothing() {
        assert_eq!(phrase_with_separator(Some('.'), "   "), "");
        assert_eq!(phrase_with_separator(None, ""), "");
    }
}
