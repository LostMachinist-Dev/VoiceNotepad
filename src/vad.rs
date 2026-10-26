//! Voice activity detection.
//!
//! Decides when a phrase starts and stops, so the transcriber runs on speech
//! rather than on the gaps. It is deliberately simple -- a level gate above an
//! adaptive noise floor, with an onset count and a hangover -- because the
//! consequences of getting it slightly wrong are small: a phrase is committed
//! a moment early or late, never lost.
//!
//! It never creates bullets. Bullets are hotkey-only, by design, so a pause in
//! speech has no structural meaning here at all.

use crate::resample::rms;

/// 20 ms at 16 kHz. Short enough to react, long enough for a stable level.
pub const FRAME_SAMPLES: usize = 320;

/// Consecutive loud frames before we call it speech (40 ms). Rejects clicks.
const ONSET_FRAMES: usize = 2;

/// Consecutive quiet frames before we call the phrase over (900 ms). Long
/// enough to survive the pause between words and a short pause for thought,
/// short enough that the text still feels live.
const HANGOVER_FRAMES: usize = 45;

/// Absolute floor: below this, a room is silent no matter what the noise
/// tracker believes.
const MIN_SPEECH_RMS: f32 = 0.008;

/// How far above the noise floor counts as speech.
const FLOOR_MULTIPLE: f32 = 2.5;

/// What a frame did to the speech state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadEvent {
    /// Still quiet.
    Silence,
    /// This frame began a phrase.
    SpeechStarted,
    /// Mid-phrase.
    Speech,
    /// The hangover expired; the phrase is finished.
    SpeechEnded,
}

pub struct Vad {
    noise_floor: f32,
    initialised: bool,
    speech_run: usize,
    silence_run: usize,
    in_speech: bool,
    last_level: f32,
}

impl Default for Vad {
    fn default() -> Self {
        Self::new()
    }
}

impl Vad {
    pub fn new() -> Self {
        Vad {
            noise_floor: 0.0,
            initialised: false,
            speech_run: 0,
            silence_run: 0,
            in_speech: false,
            last_level: 0.0,
        }
    }

    /// Forget the current phrase but keep the learned noise floor.
    pub fn reset_phrase(&mut self) {
        self.speech_run = 0;
        self.silence_run = 0;
        self.in_speech = false;
    }

    pub fn in_speech(&self) -> bool {
        self.in_speech
    }

    /// Level of the most recent frame, for the meter in Settings.
    pub fn level(&self) -> f32 {
        self.last_level
    }

    /// Feed one [`FRAME_SAMPLES`]-sized frame.
    pub fn push_frame(&mut self, frame: &[f32]) -> VadEvent {
        let level = rms(frame);
        self.last_level = level;

        // Noise floor: drop to a new quiet level at once, rise only very
        // slowly. That way a hissy room is learned quickly and a long vowel
        // is never mistaken for the background.
        if !self.initialised {
            self.noise_floor = level;
            self.initialised = true;
        } else if level < self.noise_floor {
            self.noise_floor = level;
        } else {
            self.noise_floor += (level - self.noise_floor) * 0.0005;
        }

        let threshold = (self.noise_floor * FLOOR_MULTIPLE).max(MIN_SPEECH_RMS);
        let loud = level > threshold;

        if self.in_speech {
            if loud {
                self.silence_run = 0;
            } else {
                self.silence_run += 1;
            }
            if self.silence_run >= HANGOVER_FRAMES {
                self.in_speech = false;
                self.speech_run = 0;
                return VadEvent::SpeechEnded;
            }
            VadEvent::Speech
        } else {
            if loud {
                self.speech_run += 1;
            } else {
                self.speech_run = 0;
            }
            if self.speech_run >= ONSET_FRAMES {
                self.in_speech = true;
                self.silence_run = 0;
                return VadEvent::SpeechStarted;
            }
            VadEvent::Silence
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(level: f32) -> Vec<f32> {
        // Alternating sign gives an RMS equal to `level`.
        (0..FRAME_SAMPLES)
            .map(|i| if i % 2 == 0 { level } else { -level })
            .collect()
    }

    fn feed(vad: &mut Vad, level: f32, count: usize) -> Vec<VadEvent> {
        let f = frame(level);
        (0..count).map(|_| vad.push_frame(&f)).collect()
    }

    #[test]
    fn a_quiet_room_is_never_speech() {
        let mut vad = Vad::new();
        let events = feed(&mut vad, 0.0005, 200);
        assert!(events.iter().all(|e| *e == VadEvent::Silence));
        assert!(!vad.in_speech());
    }

    #[test]
    fn speech_starts_after_the_onset_count() {
        let mut vad = Vad::new();
        feed(&mut vad, 0.0005, 50); // learn the floor
        let events = feed(&mut vad, 0.2, 5);
        assert_eq!(events[0], VadEvent::Silence); // first loud frame only arms it
        assert_eq!(events[1], VadEvent::SpeechStarted);
        assert_eq!(events[2], VadEvent::Speech);
        assert!(vad.in_speech());
    }

    #[test]
    fn a_single_click_does_not_start_speech() {
        let mut vad = Vad::new();
        feed(&mut vad, 0.0005, 50);
        assert_eq!(vad.push_frame(&frame(0.9)), VadEvent::Silence);
        assert_eq!(vad.push_frame(&frame(0.0005)), VadEvent::Silence);
        assert!(!vad.in_speech());
    }

    #[test]
    fn a_phrase_ends_after_the_hangover() {
        let mut vad = Vad::new();
        feed(&mut vad, 0.0005, 50);
        feed(&mut vad, 0.2, 10);
        assert!(vad.in_speech());

        let events = feed(&mut vad, 0.0005, HANGOVER_FRAMES);
        assert_eq!(*events.last().unwrap(), VadEvent::SpeechEnded);
        assert!(!vad.in_speech());
        // Everything before the last frame was still inside the phrase.
        assert!(events[..events.len() - 1]
            .iter()
            .all(|e| *e == VadEvent::Speech));
    }

    #[test]
    fn short_gaps_between_words_do_not_end_the_phrase() {
        let mut vad = Vad::new();
        feed(&mut vad, 0.0005, 50);
        feed(&mut vad, 0.2, 10);
        // 600 ms of quiet is a pause between words, not the end of a phrase.
        let events = feed(&mut vad, 0.0005, 30);
        assert!(events.iter().all(|e| *e == VadEvent::Speech));
        assert!(vad.in_speech());
        // and speaking again keeps it going
        feed(&mut vad, 0.2, 5);
        assert!(vad.in_speech());
    }

    #[test]
    fn steady_background_hiss_is_learned_and_ignored() {
        let mut vad = Vad::new();
        let events = feed(&mut vad, 0.02, 300);
        assert!(
            events.iter().all(|e| *e == VadEvent::Silence),
            "constant hiss was mistaken for speech"
        );
    }

    #[test]
    fn speech_still_registers_over_background_hiss() {
        let mut vad = Vad::new();
        feed(&mut vad, 0.02, 300);
        let events = feed(&mut vad, 0.3, 5);
        assert!(events.contains(&VadEvent::SpeechStarted));
    }

    #[test]
    fn reset_clears_the_phrase_but_keeps_the_floor() {
        let mut vad = Vad::new();
        feed(&mut vad, 0.02, 300);
        feed(&mut vad, 0.3, 5);
        assert!(vad.in_speech());
        vad.reset_phrase();
        assert!(!vad.in_speech());
        // The learned floor still suppresses the hiss.
        let events = feed(&mut vad, 0.02, 50);
        assert!(events.iter().all(|e| *e == VadEvent::Silence));
    }

    #[test]
    fn reports_the_level_of_the_last_frame() {
        let mut vad = Vad::new();
        vad.push_frame(&frame(0.25));
        assert!((vad.level() - 0.25).abs() < 1e-5);
    }
}
