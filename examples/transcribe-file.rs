//! Transcribe a WAV file with the same code path live dictation uses.
//!
//! This exists because the microphone cannot be exercised in CI or on the
//! Linux build box, but everything after the microphone can: the voice
//! detector, the phrase chunking, the Whisper parameters and the transcript
//! cleanup are all shared with [`voice_notepad::stt`].
//!
//!     cargo run --example transcribe-file -- <model.bin> <audio.wav>
//!
//! The audio must be WAV; 16-bit or 32-bit float PCM, any sample rate, mono
//! or stereo.

use std::path::Path;

use voice_notepad::resample::{Resampler, WHISPER_SAMPLE_RATE};
use voice_notepad::stt::transcribe_clip;
use voice_notepad::vad::{Vad, VadEvent, FRAME_SAMPLES};
use whisper_rs::{WhisperContext, WhisperContextParameters};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: transcribe-file <model.bin> <audio.wav> [--whole]");
        std::process::exit(2);
    }
    let (model_path, audio_path) = (&args[1], &args[2]);

    let samples = match read_wav_as_16k_mono(Path::new(audio_path)) {
        Ok(samples) => samples,
        Err(err) => {
            eprintln!("could not read {audio_path}: {err}");
            std::process::exit(1);
        }
    };
    println!(
        "read {} ({:.1}s at {} Hz mono)",
        audio_path,
        samples.len() as f32 / WHISPER_SAMPLE_RATE as f32,
        WHISPER_SAMPLE_RATE
    );

    let load_started = std::time::Instant::now();
    let context =
        match WhisperContext::new_with_params(model_path, WhisperContextParameters::default()) {
            Ok(context) => context,
            Err(err) => {
                eprintln!("could not load {model_path}: {err}");
                std::process::exit(1);
            }
        };
    let load_time = load_started.elapsed();

    // Timed separately because creating a state allocates the KV cache and
    // compute buffers. The live path builds one and reuses it; doing it per
    // phrase would add this much to every single inference.
    let state_started = std::time::Instant::now();
    let mut state = match context.create_state() {
        Ok(state) => state,
        Err(err) => {
            eprintln!("could not create a recogniser state: {err}");
            std::process::exit(1);
        }
    };
    println!(
        "loaded {model_path} in {:.2}s (recogniser state: {:.2}s)",
        load_time.as_secs_f32(),
        state_started.elapsed().as_secs_f32()
    );

    let threads = std::thread::available_parallelism()
        .map(|n| n.get().min(8) as i32)
        .unwrap_or(4);

    // One clip, no splitting: the baseline to compare phrase-at-a-time against.
    let whole = args.iter().any(|a| a == "--whole");

    // Split into phrases exactly as the live path does, pre-roll included.
    // Without the pre-roll the attack of each phrase is lost, and the first
    // word or two comes out wrong -- which is the whole reason it exists.
    let phrases: Vec<Vec<f32>> = if whole {
        vec![samples.clone()]
    } else {
        let mut vad = Vad::new();
        let mut found: Vec<Vec<f32>> = Vec::new();
        let mut current: Vec<f32> = Vec::new();
        let mut preroll: std::collections::VecDeque<f32> = std::collections::VecDeque::new();
        const PREROLL_FRAMES: usize = 10;

        for frame in samples.chunks(FRAME_SAMPLES) {
            if frame.len() < FRAME_SAMPLES {
                break;
            }
            match vad.push_frame(frame) {
                VadEvent::Silence => {
                    preroll.extend(frame.iter().copied());
                    while preroll.len() > PREROLL_FRAMES * FRAME_SAMPLES {
                        preroll.pop_front();
                    }
                }
                VadEvent::SpeechStarted => {
                    current.clear();
                    current.extend(preroll.iter().copied());
                    preroll.clear();
                    current.extend_from_slice(frame);
                }
                VadEvent::Speech => current.extend_from_slice(frame),
                VadEvent::SpeechEnded => {
                    current.extend_from_slice(frame);
                    found.push(std::mem::take(&mut current));
                }
            }
        }
        if !current.is_empty() {
            found.push(current);
        }
        found
    };
    println!("voice detector found {} phrase(s)\n", phrases.len());

    let mut transcript = String::new();
    for (i, phrase) in phrases.iter().enumerate() {
        let started = std::time::Instant::now();
        match transcribe_clip(&mut state, phrase, false, threads) {
            Ok(text) => {
                println!(
                    "phrase {} ({:.1}s audio, {:.1}s to transcribe): {text}",
                    i + 1,
                    phrase.len() as f32 / WHISPER_SAMPLE_RATE as f32,
                    started.elapsed().as_secs_f32()
                );
                if !transcript.is_empty() {
                    transcript.push(' ');
                }
                transcript.push_str(&text);
            }
            Err(err) => println!("phrase {}: FAILED: {err}", i + 1),
        }
    }

    println!("\n--- transcript ---\n{transcript}");

    // The spec forbids these anywhere in the output, so say so out loud.
    let clean = !transcript.contains("-->")
        && !transcript.contains('[')
        && !transcript.contains(']');
    println!(
        "\nno timestamps, labels or segment markers: {}",
        if clean { "yes" } else { "NO -- BUG" }
    );
    if !clean {
        std::process::exit(1);
    }
}

/// A deliberately small WAV reader: enough for the formats a test clip comes
/// in, with no extra dependency pulled into the shipping binary.
fn read_wav_as_16k_mono(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }

    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);

    let mut pos = 12;
    let mut format_tag = 0u16;
    let mut channels = 1u16;
    let mut rate = WHISPER_SAMPLE_RATE;
    let mut bits = 16u16;
    let mut data: Option<(usize, usize)> = None;

    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32_at(pos + 4) as usize;
        let body = pos + 8;
        match id {
            b"fmt " if body + 16 <= bytes.len() => {
                format_tag = u16_at(body);
                channels = u16_at(body + 2);
                rate = u32_at(body + 4);
                bits = u16_at(body + 14);
            }
            b"data" => {
                data = Some((body, size.min(bytes.len().saturating_sub(body))));
                break;
            }
            _ => {}
        }
        pos = body + size + (size & 1); // chunks are word-aligned
    }

    let (start, len) = data.ok_or("no data chunk")?;
    let raw = &bytes[start..start + len];

    let interleaved: Vec<f32> = match (format_tag, bits) {
        (1, 16) => raw
            .chunks_exact(2)
            .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0)
            .collect(),
        (1, 32) => raw
            .chunks_exact(4)
            .map(|s| i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0)
            .collect(),
        (3, 32) => raw
            .chunks_exact(4)
            .map(|s| f32::from_le_bytes([s[0], s[1], s[2], s[3]]))
            .collect(),
        _ => {
            return Err(format!(
                "unsupported WAV format (tag {format_tag}, {bits} bits)"
            ))
        }
    };

    let mut resampler = Resampler::new(rate, channels);
    Ok(resampler.process(&interleaved))
}
