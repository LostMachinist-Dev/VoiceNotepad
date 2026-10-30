//! Transcribe a WAV file with the same code path live dictation uses.
//!
//! This exists because the microphone cannot be driven in CI, but everything
//! after it can: the chunking, the recogniser and the fragment joining are
//! all shared with the live path. It is also how the numbers in HANDOFF.md
//! were measured.
//!
//!     cargo run --release --example transcribe-file -- <model-dir> <audio.wav>
//!
//! The audio may be any sample rate, mono or stereo, 16-bit or 32-bit float.

use std::path::Path;

use voice_notepad::config::SpeechModel;
use voice_notepad::resample::{Resampler, WHISPER_SAMPLE_RATE};
use voice_notepad::stt::Recogniser;
use voice_notepad::transcript::join_fragment;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: transcribe-file <model-dir> <audio.wav>");
        eprintln!("  the model folder name decides which model is loaded:");
        for model in SpeechModel::ALL {
            eprintln!("    {:<24} {}", model.dir_name(), model.label());
        }
        std::process::exit(2);
    }
    let (model_dir, audio_path) = (&args[1], &args[2]);

    // Which model this is, from the folder name, falling back to the default.
    let model = SpeechModel::ALL
        .into_iter()
        .find(|m| model_dir.contains(m.dir_name()))
        .unwrap_or_default();

    let samples = match read_wav_as_16k_mono(Path::new(audio_path)) {
        Ok(samples) => samples,
        Err(err) => {
            eprintln!("could not read {audio_path}: {err}");
            std::process::exit(1);
        }
    };
    let duration = samples.len() as f32 / WHISPER_SAMPLE_RATE as f32;
    println!("clip: {audio_path} ({duration:.1}s)");

    let load_started = std::time::Instant::now();
    let mut recogniser = match Recogniser::load(model, Path::new(model_dir)) {
        Ok(recogniser) => recogniser,
        Err(err) => {
            eprintln!("could not load {model_dir}: {err}");
            std::process::exit(1);
        }
    };
    let chunk_samples = recogniser.chunk_samples();
    println!(
        "model: {} loaded in {:.1}s, {} ms chunks\n",
        model.label(),
        load_started.elapsed().as_secs_f32(),
        chunk_samples * 1000 / WHISPER_SAMPLE_RATE as usize
    );

    // --- stream it, exactly as the live path does -------------------------
    let mut transcript = String::new();
    let mut worst = 0.0f32;
    let mut total = 0.0f32;
    let mut chunks = 0usize;

    let started = std::time::Instant::now();
    let mut feed = |recogniser: &mut Recogniser, chunk: &[f32], transcript: &mut String| {
        let t = std::time::Instant::now();
        let text = recogniser.transcribe_chunk(chunk);
        let took = t.elapsed().as_secs_f32();
        total += took;
        worst = worst.max(took);
        chunks += 1;
        if let Ok(text) = text {
            let piece = join_fragment(transcript.chars().last(), &text);
            transcript.push_str(&piece);
        }
    };

    for chunk in samples.chunks(chunk_samples) {
        let mut block = chunk.to_vec();
        block.resize(chunk_samples, 0.0);
        feed(&mut recogniser, &block, &mut transcript);
    }
    // Flush the tail out of the model's cache.
    let silence = vec![0.0f32; chunk_samples];
    for _ in 0..3 {
        feed(&mut recogniser, &silence, &mut transcript);
    }
    let elapsed = started.elapsed().as_secs_f32();

    println!("--- transcript ---\n{}", transcript.trim());

    let chunk_ms = chunk_samples as f32 * 1000.0 / WHISPER_SAMPLE_RATE as f32;
    println!("\n--- speed ---");
    println!("  {chunks} chunks of {chunk_ms:.0} ms");
    println!("  slowest chunk: {:.0} ms", worst * 1000.0);
    println!("  average chunk: {:.0} ms", total / chunks as f32 * 1000.0);
    println!("  real-time factor: {:.2}x", duration / elapsed);
    if worst * 1000.0 < chunk_ms {
        println!("  -> keeps up with live speech on this machine.");
    } else {
        println!("  -> TOO SLOW for live here: a chunk takes longer than it covers.");
    }

    // The spec forbids these anywhere in the output.
    let clean = !transcript.contains("-->") && !transcript.contains('[');
    println!(
        "\nno timestamps, labels or segment markers: {}",
        if clean { "yes" } else { "NO -- BUG" }
    );
    if !clean {
        std::process::exit(1);
    }
}

/// A deliberately small WAV reader: enough for a test clip, with no extra
/// dependency pulled into the shipping binary.
fn read_wav_as_16k_mono(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }

    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);

    let mut pos = 12;
    let (mut tag, mut channels, mut rate, mut bits) = (0u16, 1u16, WHISPER_SAMPLE_RATE, 16u16);
    let mut data: Option<(usize, usize)> = None;

    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32_at(pos + 4) as usize;
        let body = pos + 8;
        match id {
            b"fmt " if body + 16 <= bytes.len() => {
                tag = u16_at(body);
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

    let interleaved: Vec<f32> = match (tag, bits) {
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
        _ => return Err(format!("unsupported WAV format (tag {tag}, {bits} bits)")),
    };

    let mut resampler = Resampler::new(rate, channels);
    Ok(resampler.process(&interleaved))
}
