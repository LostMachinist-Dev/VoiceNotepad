//! Microphone capture.
//!
//! One cpal input stream, downmixed and resampled to 16 kHz mono on the audio
//! callback, pushed into a shared queue that the speech thread drains. The
//! stream is created when listening starts and dropped when it stops, which is
//! what keeps idle CPU at nothing: there is no callback running at all.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, Sample, SampleFormat};

use crate::resample::{Resampler, WHISPER_SAMPLE_RATE};

/// Cap on buffered audio. If the speech thread falls far behind -- a slow CPU
/// with a big model -- we drop the oldest audio rather than grow forever.
const MAX_BUFFERED_SECONDS: usize = 30;
const MAX_BUFFERED: usize = WHISPER_SAMPLE_RATE as usize * MAX_BUFFERED_SECONDS;

/// Audio queued for transcription, in 16 kHz mono.
pub type SharedAudio = Arc<Mutex<VecDeque<f32>>>;

pub fn shared_audio() -> SharedAudio {
    Arc::new(Mutex::new(VecDeque::new()))
}

/// Why capture could not start, in terms a person can act on.
#[derive(Debug, Clone)]
pub enum AudioError {
    NoDevice,
    NamedDeviceMissing(String),
    PermissionDenied,
    DeviceBusy,
    Unsupported(String),
    Other(String),
}

impl AudioError {
    /// A complete, plain-words message for the status bar or a dialog.
    pub fn user_message(&self) -> String {
        match self {
            AudioError::NoDevice => "No microphone was found. Plug one in, then press the \
                 mic button again."
                .into(),
            AudioError::NamedDeviceMissing(name) => format!(
                "The microphone \"{name}\" is not available any more. Pick a different one in \
                 Settings, or choose the system default."
            ),
            AudioError::PermissionDenied => "Windows blocked access to the microphone. Open \
                 Settings > Privacy & security > Microphone and allow desktop apps to use it."
                .into(),
            AudioError::DeviceBusy => "The microphone is in use by another program. Close that \
                 program and try again."
                .into(),
            AudioError::Unsupported(what) => format!(
                "The microphone does not offer a format this app can read ({what}). Try a \
                 different device in Settings."
            ),
            AudioError::Other(detail) => format!("The microphone could not be opened: {detail}"),
        }
    }
}

fn map_cpal(err: &cpal::Error) -> AudioError {
    match err.kind() {
        ErrorKind::PermissionDenied => AudioError::PermissionDenied,
        ErrorKind::DeviceBusy => AudioError::DeviceBusy,
        ErrorKind::DeviceNotAvailable => AudioError::NoDevice,
        ErrorKind::UnsupportedConfig => AudioError::Unsupported(err.to_string()),
        _ => AudioError::Other(err.to_string()),
    }
}

/// Names of the available input devices, for the Settings picker.
///
/// Never fails: an empty list is the honest answer when there is no audio
/// subsystem at all, and the UI shows that as "no microphones found".
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };
    let mut names: Vec<String> = devices.map(|d| d.to_string()).collect();
    names.sort();
    names.dedup();
    names
}

/// The name of the system default input device, if there is one.
pub fn default_input_device_name() -> Option<String> {
    cpal::default_host()
        .default_input_device()
        .map(|d| d.to_string())
}

/// Open the default input briefly and report what happened, in words.
///
/// Exists because "I pressed the mic and nothing happened" is otherwise
/// impossible to act on: this says whether a device opens, at what rate and
/// format, and whether any sound actually arrives.
pub fn check(preferred: Option<&str>, listen_for: std::time::Duration) -> String {
    let devices = list_input_devices();
    let default = default_input_device_name();
    let requested = preferred.unwrap_or("system default").to_string();

    // Detail is gathered first but printed after the verdict: the answer to
    // "is my microphone working" should not be below the fold.
    let mut detail = String::new();
    detail.push_str(&format!("requested: {requested}\n"));
    detail.push_str(&format!(
        "system default: {}\n",
        default.unwrap_or_else(|| "none".into())
    ));

    if devices.is_empty() {
        return format!(
            "NOT WORKING: no input devices were found at all.\n\n\
             Plug a microphone in, then try again.\n\n{detail}"
        );
    }

    let sink = shared_audio();
    let capture = match start(preferred, Arc::clone(&sink)) {
        Ok(capture) => capture,
        Err(err) => {
            return format!(
                "NOT WORKING: the microphone would not open.\n\n{}\n\n{detail}\n{}",
                err.user_message(),
                device_list(&devices)
            )
        }
    };
    detail.push_str(&format!(
        "opened: \"{}\" at {} Hz\n",
        capture.device_name, capture.source_rate
    ));

    std::thread::sleep(listen_for);
    let (count, peak) = match sink.lock() {
        Ok(queue) => (
            queue.len(),
            queue.iter().fold(0.0f32, |m, s| m.max(s.abs())),
        ),
        Err(_) => (0, 0.0),
    };
    drop(capture);

    let seconds = count as f32 / crate::resample::WHISPER_SAMPLE_RATE as f32;
    detail.push_str(&format!(
        "received: {count} samples ({seconds:.2}s of 16 kHz mono)\n"
    ));
    detail.push_str(&format!("loudest sound: {peak:.4}\n"));

    let verdict = if count == 0 {
        "NOT WORKING: the microphone opened but sent no sound at all.\n\n         Something else has probably taken it, or it is disabled in the          system's sound settings."
    } else if peak < 0.001 {
        "NOT WORKING: sound is arriving, but it is completely silent.\n\n         The input is muted, or the wrong device is selected. Try picking a          specific microphone above instead of the system default."
    } else if peak < 0.02 {
        "WORKING, BUT VERY QUIET.\n\n         Sound is getting through but it is faint. Turn the input volume up          in the system's sound settings, or move closer to the microphone,          or speech may not be picked up."
    } else {
        "WORKING. Sound is reaching Voice Notepad."
    };

    format!("{verdict}\n\n{detail}\n{}", device_list(&devices))
}

/// The device list, last, because it is long and rarely the answer.
fn device_list(devices: &[String]) -> String {
    let mut out = format!("all {} input device(s) seen:\n", devices.len());
    for name in devices {
        out.push_str(&format!("  - {name}\n"));
    }
    out
}

/// A live capture. Dropping it stops the stream and the callback.
pub struct Capture {
    // Held only to keep the stream alive; cpal stops it on drop.
    _stream: cpal::Stream,
    pub device_name: String,
    pub source_rate: u32,
}

/// Open `preferred` (or the system default) and start pushing 16 kHz mono
/// audio into `sink`.
pub fn start(preferred: Option<&str>, sink: SharedAudio) -> Result<Capture, AudioError> {
    let host = cpal::default_host();

    let device = match preferred {
        Some(name) => {
            let found = host
                .input_devices()
                .ok()
                .and_then(|mut ds| ds.find(|d| d.to_string() == name));
            match found {
                Some(d) => d,
                // Fall back to the default rather than refusing to listen, but
                // say so, because the user picked this device on purpose.
                None => return Err(AudioError::NamedDeviceMissing(name.to_string())),
            }
        }
        None => host.default_input_device().ok_or(AudioError::NoDevice)?,
    };

    let device_name = device.to_string();
    let supported = device.default_input_config().map_err(|e| map_cpal(&e))?;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.clone().into();
    let source_rate = config.sample_rate;
    let channels = config.channels;

    let error_sink = Arc::clone(&sink);
    let err_fn = move |err: cpal::Error| {
        // A dead stream must not look like silence forever; clearing the queue
        // makes the speech thread see the end of the utterance.
        if matches!(
            err.kind(),
            ErrorKind::DeviceNotAvailable | ErrorKind::StreamInvalidated
        ) {
            if let Ok(mut q) = error_sink.lock() {
                q.clear();
            }
        }
        eprintln!("audio stream error: {err}");
    };

    let stream = match sample_format {
        SampleFormat::F32 => build::<f32>(&device, &config, source_rate, channels, sink, err_fn),
        SampleFormat::I16 => build::<i16>(&device, &config, source_rate, channels, sink, err_fn),
        SampleFormat::I32 => build::<i32>(&device, &config, source_rate, channels, sink, err_fn),
        SampleFormat::I8 => build::<i8>(&device, &config, source_rate, channels, sink, err_fn),
        SampleFormat::U8 => build::<u8>(&device, &config, source_rate, channels, sink, err_fn),
        other => return Err(AudioError::Unsupported(format!("{other}"))),
    }?;

    stream.play().map_err(|e| map_cpal(&e))?;

    Ok(Capture {
        _stream: stream,
        device_name,
        source_rate,
    })
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    source_rate: u32,
    channels: u16,
    sink: SharedAudio,
    err_fn: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, AudioError>
where
    T: Sample + cpal::SizedSample,
    f32: FromSample<T>,
{
    let mut resampler = Resampler::new(source_rate, channels);
    let mut scratch: Vec<f32> = Vec::new();

    device
        .build_input_stream(
            config.clone(),
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                scratch.clear();
                scratch.extend(data.iter().map(|s| f32::from_sample(*s)));
                let mono = resampler.process(&scratch);
                if mono.is_empty() {
                    return;
                }
                // try_lock, not lock: the audio callback must never block on
                // the speech thread. Dropping a callback's worth of audio is
                // better than an underrun.
                if let Ok(mut queue) = sink.try_lock() {
                    queue.extend(mono);
                    if queue.len() > MAX_BUFFERED {
                        let excess = queue.len() - MAX_BUFFERED;
                        queue.drain(..excess);
                    }
                }
            },
            err_fn,
            None,
        )
        .map_err(|e| map_cpal(&e))
}
