//! Turning whatever the microphone gives us into what Whisper wants.
//!
//! Whisper needs 16 kHz mono `f32`. Microphones hand us 44.1 or 48 kHz, often
//! stereo, in whatever sample format the driver prefers. This does the
//! downmix and rate conversion incrementally, keeping the leftover fraction
//! between calls so that successive audio callbacks join seamlessly.
//!
//! Linear interpolation is enough here: speech is band-limited well below
//! 8 kHz and Whisper's own front end is a mel spectrogram, so the artefacts a
//! longer filter would remove are not ones the model can hear.

/// The sample rate whisper.cpp expects.
pub const WHISPER_SAMPLE_RATE: u32 = 16_000;

/// Incremental downmix-and-resample to 16 kHz mono.
pub struct Resampler {
    channels: usize,
    /// Input samples consumed per output sample.
    step: f64,
    /// Fractional read position within `carry`.
    pos: f64,
    /// Mono input not yet consumed, carried between calls.
    carry: Vec<f32>,
}

impl Resampler {
    pub fn new(source_rate: u32, channels: u16) -> Self {
        Resampler {
            channels: channels.max(1) as usize,
            step: source_rate.max(1) as f64 / WHISPER_SAMPLE_RATE as f64,
            pos: 0.0,
            carry: Vec::new(),
        }
    }

    /// Feed one callback's worth of interleaved samples; get back 16 kHz mono.
    pub fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
        for frame in interleaved.chunks(self.channels) {
            let sum: f32 = frame.iter().sum();
            self.carry.push(sum / frame.len() as f32);
        }

        // Each output sample interpolates between carry[i] and carry[i+1], so
        // we can only emit while a full pair is available.
        let mut out = Vec::with_capacity(
            ((self.carry.len() as f64 - self.pos) / self.step).max(0.0) as usize + 1,
        );
        while self.pos + 1.0 < self.carry.len() as f64 {
            let i = self.pos.floor() as usize;
            let frac = (self.pos - i as f64) as f32;
            out.push(self.carry[i] * (1.0 - frac) + self.carry[i + 1] * frac);
            self.pos += self.step;
        }

        // Discard what we are past, keeping the sample the next interpolation
        // still needs.
        let consumed = self.pos.floor() as usize;
        if consumed > 0 {
            self.carry.drain(..consumed);
            self.pos -= consumed as f64;
        }
        out
    }
}

/// Root-mean-square level of a block, used for voice activity detection and
/// for the level meter in Settings.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| (*s as f64) * (*s as f64)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_16k_mono_through_at_the_same_rate() {
        let mut r = Resampler::new(16_000, 1);
        let input: Vec<f32> = (0..1600).map(|i| (i % 10) as f32 / 10.0).collect();
        let out = r.process(&input);
        // One sample is held back for the next interpolation.
        assert_eq!(out.len(), 1599);
    }

    #[test]
    fn decimates_48k_stereo_to_roughly_a_third() {
        let mut r = Resampler::new(48_000, 2);
        // 4800 frames of stereo = 100 ms, which is 1600 samples at 16 kHz.
        let input = vec![0.25f32; 4800 * 2];
        let out = r.process(&input);
        assert!(
            (out.len() as i64 - 1600).abs() <= 2,
            "expected about 1600 samples, got {}",
            out.len()
        );
    }

    #[test]
    fn downmixes_stereo_by_averaging_the_channels() {
        let mut r = Resampler::new(16_000, 2);
        // Left hard +1, right hard -1: the mono result is silence.
        let input: Vec<f32> = (0..200).flat_map(|_| [1.0f32, -1.0f32]).collect();
        let out = r.process(&input);
        assert!(out.iter().all(|s| s.abs() < 1e-6));
    }

    #[test]
    fn a_constant_signal_stays_constant() {
        let mut r = Resampler::new(44_100, 1);
        let out = r.process(&vec![0.5f32; 4410]);
        assert!(!out.is_empty());
        for s in out {
            assert!((s - 0.5).abs() < 1e-5, "interpolation drifted: {s}");
        }
    }

    #[test]
    fn successive_callbacks_join_without_gaps_or_drift() {
        // 1 second of 48 kHz stereo delivered in 100 chunks should yield
        // close to 16000 samples in total, with no accumulated drift.
        let mut r = Resampler::new(48_000, 2);
        let chunk = vec![0.1f32; 480 * 2];
        let total: usize = (0..100).map(|_| r.process(&chunk).len()).sum();
        assert!(
            (total as i64 - 16_000).abs() <= 4,
            "drifted over a second: {total}"
        );
    }

    #[test]
    fn handles_an_empty_callback() {
        let mut r = Resampler::new(48_000, 2);
        assert!(r.process(&[]).is_empty());
    }

    #[test]
    fn rms_measures_level() {
        assert_eq!(rms(&[]), 0.0);
        assert!(rms(&[0.0; 100]).abs() < 1e-9);
        assert!((rms(&[1.0, -1.0, 1.0, -1.0]) - 1.0).abs() < 1e-6);
        assert!(rms(&[0.1; 50]) < rms(&[0.5; 50]));
    }
}
