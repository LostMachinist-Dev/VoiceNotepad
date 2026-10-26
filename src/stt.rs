//! The speech engine: one background thread that owns the microphone and
//! whisper.cpp, and talks to the UI over channels.
//!
//! Why one thread owns both: cpal's `Stream` is not `Send` on Windows, and the
//! Whisper state is expensive to create. Keeping them together means the UI
//! never touches either, so the UI thread cannot be blocked by a model load or
//! an inference pass.
//!
//! How live text works. While a phrase is being spoken the whole phrase so far
//! is re-transcribed every [`PARTIAL_INTERVAL`] and sent as [`Event::Partial`].
//! The UI holds that text in a known range and swaps it for each better guess,
//! so the words visibly firm up as you keep talking. When the voice detector
//! says the phrase is over, one last pass is sent as [`Event::Final`] and the
//! text stops moving.

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

use crate::audio::{self, AudioError, SharedAudio};
use crate::config::{find_model, model_dir, ModelSize};
use crate::model;
use crate::resample::WHISPER_SAMPLE_RATE;
use crate::transcript::{clean_transcript, pad_for_inference};
use crate::vad::{Vad, VadEvent, FRAME_SAMPLES};

/// How often the in-progress phrase is re-transcribed.
const PARTIAL_INTERVAL: Duration = Duration::from_millis(700);

/// Don't bother transcribing less speech than this.
const MIN_PHRASE_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize / 2;

/// Whisper only looks at the first 30 s of a clip, so a phrase with no pause
/// in it has to be committed rather than silently lose its tail. 15 s also
/// bounds the cost of a partial, since each one re-transcribes the whole
/// phrase so far.
const MAX_PHRASE_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 15;

/// Audio kept before the voice detector fires, so phrase onsets are not
/// clipped. 10 frames is 200 ms.
const PREROLL_FRAMES: usize = 10;

/// Whisper's encoder window, in frames: 30 seconds at 50 frames per second.
const FULL_AUDIO_CTX: usize = 1500;

/// Never give the encoder less context than this, however short the phrase.
///
/// Measured on whisper.cpp's jfk.wav with small.en, per phrase: the full 1500
/// takes 2.0 s, 256 takes 0.3 s but starts repeating itself ("Ask not. Ask
/// not."), 512 takes 0.5 s and matches the full-context text exactly, and 768
/// takes 0.9 s for no further gain. Whisper's position embeddings are trained
/// on the full window, so cutting too close to the audio degrades it.
const MIN_AUDIO_CTX: usize = 512;

/// How much encoder context a clip of this length actually needs.
///
/// Whisper pads every clip to 30 seconds and, left alone, encodes all of it --
/// so a three second phrase spends most of its time encoding silence. Telling
/// it how much of the window is real is the single biggest speed win
/// available here, and it costs nothing in accuracy as long as the figure
/// covers the audio with room to spare.
///
/// Samples map to encoder frames at 320 samples each (160-sample hop, halved
/// again by the convolution stride).
fn audio_ctx_for(samples: usize) -> std::ffi::c_int {
    let frames = samples / 320;
    // Margin for the mel padding and the tail of the final window.
    let rounded = (frames + 96).div_ceil(64) * 64;
    rounded.clamp(MIN_AUDIO_CTX, FULL_AUDIO_CTX) as std::ffi::c_int
}

/// What the UI asks of the engine.
#[derive(Debug, Clone)]
pub enum Command {
    /// Make sure the model file is on disk, downloading it if needed.
    EnsureModel,
    StartListening,
    StopListening,
    /// Microphone name, or `None` for the system default.
    SetDevice(Option<String>),
    SetModel(ModelSize),
    Shutdown,
}

/// What the engine tells the UI.
#[derive(Debug, Clone)]
pub enum Event {
    /// First-run download progress.
    ModelDownload(model::Progress),
    /// The model file is present and being loaded into memory.
    ModelLoading,
    /// Ready to transcribe.
    ModelReady,
    /// Listening started or stopped. The engine is the authority on this, so
    /// a failure to open the microphone turns the mic button back to grey.
    Listening(bool),
    /// The phrase in progress, to replace whatever partial is on screen.
    Partial(String),
    /// The phrase is finished; this text is permanent.
    Final(String),
    /// Input level, for the meter in Settings.
    Level(f32),
    /// Something went wrong, in words fit to show the user.
    Error(String),
}

/// Handle to the engine thread.
pub struct SpeechEngine {
    commands: Sender<Command>,
    events: Receiver<Event>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SpeechEngine {
    /// Start the engine thread. It idles until told to do something.
    pub fn spawn(model: ModelSize, device: Option<String>) -> Self {
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let (event_tx, events) = std::sync::mpsc::channel();

        let thread = std::thread::Builder::new()
            .name("voice-notepad-speech".into())
            .spawn(move || {
                let mut worker = Worker::new(model, device, event_tx);
                worker.run(command_rx);
            })
            .ok();

        SpeechEngine {
            commands: command_tx,
            events,
            thread,
        }
    }

    pub fn send(&self, command: Command) {
        // A dead engine thread is reported through events; dropping a command
        // here must not panic the UI.
        let _ = self.commands.send(command);
    }

    /// Drain everything the engine has said since the last frame.
    pub fn drain(&self) -> Vec<Event> {
        let mut out = Vec::new();
        loop {
            match self.events.try_recv() {
                Ok(event) => out.push(event),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }
}

impl Drop for SpeechEngine {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// ---------------------------------------------------------------------------

struct Worker {
    events: Sender<Event>,
    model_size: ModelSize,
    device: Option<String>,

    context: Option<WhisperContext>,
    /// Reused across inferences. Creating one allocates close to 200 MB of
    /// KV cache and compute buffers, so doing it per phrase -- which is what
    /// this used to do -- cost more than the inference itself on a slower
    /// machine.
    state: Option<WhisperState>,
    /// Set when the model on disk no longer matches `model_size`.
    context_stale: bool,

    audio_queue: SharedAudio,
    capture: Option<audio::Capture>,

    vad: Vad,
    /// Speech collected for the phrase in progress.
    phrase: Vec<f32>,
    /// Rolling pre-onset audio, so a phrase does not start clipped.
    preroll: std::collections::VecDeque<f32>,
    /// Samples not yet whole frames.
    frame_fill: Vec<f32>,

    last_partial: Instant,
    partial_sent: bool,
    threads: i32,
}

impl Worker {
    fn new(model_size: ModelSize, device: Option<String>, events: Sender<Event>) -> Self {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get().min(8) as i32)
            .unwrap_or(4)
            .max(1);

        Worker {
            events,
            model_size,
            device,
            context: None,
            state: None,
            context_stale: false,
            audio_queue: audio::shared_audio(),
            capture: None,
            vad: Vad::new(),
            phrase: Vec::new(),
            preroll: std::collections::VecDeque::new(),
            frame_fill: Vec::new(),
            last_partial: Instant::now(),
            partial_sent: false,
            threads,
        }
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn run(&mut self, commands: Receiver<Command>) {
        loop {
            // Handle everything pending, then do a slice of audio work.
            loop {
                match commands.try_recv() {
                    Ok(Command::Shutdown) | Err(TryRecvError::Disconnected) => {
                        self.stop_listening();
                        return;
                    }
                    Ok(command) => self.handle(command),
                    Err(TryRecvError::Empty) => break,
                }
            }

            if self.capture.is_some() {
                self.pump_audio();
                std::thread::sleep(Duration::from_millis(10));
            } else {
                // Not listening: block so the thread costs nothing at all.
                match commands.recv() {
                    Ok(Command::Shutdown) | Err(_) => {
                        self.stop_listening();
                        return;
                    }
                    Ok(command) => self.handle(command),
                }
            }
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::EnsureModel => self.ensure_model_file(),
            Command::StartListening => self.start_listening(),
            Command::StopListening => self.stop_listening(),
            Command::SetDevice(device) => {
                self.device = device;
                if self.capture.is_some() {
                    // Re-open on the new device.
                    self.stop_listening();
                    self.start_listening();
                }
            }
            Command::SetModel(size) => {
                if size != self.model_size {
                    self.model_size = size;
                    self.context = None;
                    self.state = None;
                    self.context_stale = true;
                    self.ensure_model_file();
                }
            }
            Command::Shutdown => {}
        }
    }

    /// Download the model if it is not already on disk.
    fn ensure_model_file(&mut self) {
        if find_model(self.model_size).is_some() {
            self.emit(Event::ModelDownload(model::Progress::Done(
                find_model(self.model_size).unwrap_or_default(),
            )));
            return;
        }
        let Some(dir) = model_dir() else {
            self.emit(Event::Error(
                "Could not find a folder to keep the speech model in.".into(),
            ));
            return;
        };

        let events = self.events.clone();
        let size = self.model_size;
        // Downloading on its own thread keeps the engine responsive, so the
        // mic button and Settings still work while a 466 MB file comes down.
        std::thread::Builder::new()
            .name("voice-notepad-download".into())
            .spawn(move || {
                let reporter = events.clone();
                let result = model::download(size, &dir, move |progress| {
                    let _ = reporter.send(Event::ModelDownload(progress));
                });
                if let Err(message) = result {
                    let _ = events.send(Event::ModelDownload(model::Progress::Failed(
                        message.clone(),
                    )));
                    let _ = events.send(Event::Error(message));
                }
            })
            .ok();
    }

    /// Load the model into memory. Slow (a second or two), so it happens here
    /// on the engine thread rather than on the UI thread.
    fn ensure_context(&mut self) -> bool {
        if self.context.is_some() && self.state.is_some() && !self.context_stale {
            return true;
        }
        let Some(path) = find_model(self.model_size) else {
            self.emit(Event::Error(
                "The speech model has not finished downloading yet.".into(),
            ));
            return false;
        };

        self.emit(Event::ModelLoading);
        let params = WhisperContextParameters::default();
        match WhisperContext::new_with_params(&path, params) {
            Ok(context) => {
                // Build the state once, here, where the wait is already
                // expected rather than in the middle of a phrase.
                match context.create_state() {
                    Ok(state) => {
                        self.state = Some(state);
                        self.context = Some(context);
                        self.context_stale = false;
                        self.emit(Event::ModelReady);
                        true
                    }
                    Err(err) => {
                        self.emit(Event::Error(format!(
                            "The speech recogniser could not be started: {err}"
                        )));
                        false
                    }
                }
            }
            Err(err) => {
                self.emit(Event::Error(format!(
                    "The speech model at {} could not be loaded ({err}). Try choosing a \
                     different model size in Settings.",
                    path.display()
                )));
                false
            }
        }
    }

    fn start_listening(&mut self) {
        if self.capture.is_some() {
            return;
        }
        if !self.ensure_context() {
            self.emit(Event::Listening(false));
            return;
        }

        if let Ok(mut queue) = self.audio_queue.lock() {
            queue.clear();
        }
        self.vad = Vad::new();
        self.phrase.clear();
        self.preroll.clear();
        self.frame_fill.clear();
        self.partial_sent = false;

        match audio::start(self.device.as_deref(), Arc::clone(&self.audio_queue)) {
            Ok(capture) => {
                self.capture = Some(capture);
                self.emit(Event::Listening(true));
            }
            Err(AudioError::NamedDeviceMissing(name)) => {
                // The chosen device vanished. Say so, then try the default so
                // dictation still works.
                self.emit(Event::Error(
                    AudioError::NamedDeviceMissing(name).user_message(),
                ));
                match audio::start(None, Arc::clone(&self.audio_queue)) {
                    Ok(capture) => {
                        self.capture = Some(capture);
                        self.emit(Event::Listening(true));
                    }
                    Err(err) => {
                        self.emit(Event::Error(err.user_message()));
                        self.emit(Event::Listening(false));
                    }
                }
            }
            Err(err) => {
                self.emit(Event::Error(err.user_message()));
                self.emit(Event::Listening(false));
            }
        }
    }

    fn stop_listening(&mut self) {
        let was_listening = self.capture.take().is_some();
        if !was_listening {
            return;
        }
        // Dropping the capture stops the callback; commit whatever was said.
        if self.phrase.len() >= MIN_PHRASE_SAMPLES {
            self.commit_phrase();
        } else {
            self.phrase.clear();
            self.partial_sent = false;
            // Clear any partial still showing on screen.
            self.emit(Event::Final(String::new()));
        }
        self.emit(Event::Listening(false));
    }

    /// Move audio out of the capture queue, run the detector, and transcribe.
    fn pump_audio(&mut self) {
        let incoming: Vec<f32> = match self.audio_queue.lock() {
            Ok(mut queue) => queue.drain(..).collect(),
            Err(_) => return,
        };
        if !incoming.is_empty() {
            self.frame_fill.extend_from_slice(&incoming);
        }

        let mut consumed = 0;
        while self.frame_fill.len() - consumed >= FRAME_SAMPLES {
            let frame_start = consumed;
            consumed += FRAME_SAMPLES;
            let frame: &[f32] = &self.frame_fill[frame_start..consumed];

            match self.vad.push_frame(frame) {
                VadEvent::Silence => {
                    self.preroll.extend(frame.iter().copied());
                    let cap = PREROLL_FRAMES * FRAME_SAMPLES;
                    while self.preroll.len() > cap {
                        self.preroll.pop_front();
                    }
                }
                VadEvent::SpeechStarted => {
                    self.phrase.clear();
                    self.phrase.extend(self.preroll.iter().copied());
                    self.preroll.clear();
                    self.phrase.extend_from_slice(frame);
                    self.last_partial = Instant::now();
                }
                VadEvent::Speech => self.phrase.extend_from_slice(frame),
                VadEvent::SpeechEnded => {
                    self.phrase.extend_from_slice(frame);
                    // Drop what we have consumed before a slow inference, so
                    // the queue does not look stale afterwards.
                    self.frame_fill.drain(..consumed);
                    consumed = 0;
                    self.commit_phrase();
                }
            }
        }
        if consumed > 0 {
            self.frame_fill.drain(..consumed);
        }

        self.emit(Event::Level(self.vad.level()));

        if self.vad.in_speech() && self.phrase.len() >= MAX_PHRASE_SAMPLES {
            // A phrase with no pause in it; commit and carry on in a new one.
            self.commit_phrase();
            self.vad.reset_phrase();
            return;
        }

        if self.vad.in_speech()
            && self.phrase.len() >= MIN_PHRASE_SAMPLES
            && self.last_partial.elapsed() >= PARTIAL_INTERVAL
        {
            if let Some(text) = self.transcribe(true) {
                self.partial_sent = true;
                self.emit(Event::Partial(text));
            }
            // Timed from when the pass *finished*, not when it started. On a
            // slow machine an inference can outlast the interval, and timing
            // from the start would leave no gap at all: partials would run
            // back to back, the audio queue would not be drained, and the
            // window would feel stuck.
            self.last_partial = Instant::now();
        }
    }

    /// Transcribe the phrase and send it as permanent text.
    fn commit_phrase(&mut self) {
        if self.phrase.len() < MIN_PHRASE_SAMPLES {
            if self.partial_sent {
                // Retract a partial that turned out to be nothing.
                self.emit(Event::Final(String::new()));
                self.partial_sent = false;
            }
            self.phrase.clear();
            return;
        }
        let text = self.transcribe(false).unwrap_or_default();
        self.emit(Event::Final(text));
        self.partial_sent = false;
        self.phrase.clear();
    }

    /// Run whisper.cpp over the current phrase.
    ///
    /// `partial` asks for one segment and a cheaper search, because the result
    /// is about to be replaced anyway.
    fn transcribe(&mut self, partial: bool) -> Option<String> {
        let threads = self.threads;
        let phrase = std::mem::take(&mut self.phrase);
        let result = match self.state.as_mut() {
            Some(state) => transcribe_clip(state, &phrase, partial, threads),
            None => Err("The speech recogniser is not loaded.".to_string()),
        };
        self.phrase = phrase;

        match result {
            Ok(text) => Some(text),
            Err(message) => {
                self.emit(Event::Error(message));
                None
            }
        }
    }
}

/// Transcribe one clip of 16 kHz mono audio.
///
/// This is the single place Whisper is configured, so the `transcribe-file`
/// example exercises exactly what live dictation does. Every parameter that
/// could produce a timestamp, a speaker label or a segment marker is turned
/// off here, and [`clean_transcript`] scrubs the result as well.
///
/// Each phrase is decoded on its own, with no carried-over prompt: feeding
/// the previous text back as context measured no better on the test clip and
/// leaks a `CString` per call in whisper-rs 0.16.
pub fn transcribe_clip(
    state: &mut WhisperState,
    samples: &[f32],
    partial: bool,
    threads: i32,
) -> Result<String, String> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(threads);
    params.set_language(Some("en"));
    params.set_translate(false);
    params.set_no_context(true);
    // Everything below is what keeps timestamps and markers out.
    params.set_no_timestamps(true);
    params.set_token_timestamps(false);
    params.set_print_timestamps(false);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_tdrz_enable(false);
    params.set_suppress_blank(true);
    params.set_single_segment(partial);
    if partial {
        params.set_temperature(0.0);
        params.set_temperature_inc(0.0);
    }

    // See audio_ctx_for: do not make the encoder chew through the silence
    // Whisper pads every clip with.
    let audio = pad_for_inference(samples);
    params.set_audio_ctx(audio_ctx_for(audio.len()));

    state
        .full(params, &audio)
        .map_err(|err| format!("Transcription failed: {err}"))?;

    let mut text = String::new();
    for i in 0..state.full_n_segments() {
        let Some(segment) = state.get_segment(i) else {
            continue;
        };
        if let Ok(piece) = segment.to_str_lossy() {
            text.push_str(&piece);
            text.push(' ');
        }
    }

    Ok(clean_transcript(&text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resample::WHISPER_SAMPLE_RATE;

    fn seconds(s: f32) -> usize {
        (WHISPER_SAMPLE_RATE as f32 * s) as usize
    }

    #[test]
    fn short_phrases_get_the_minimum_encoder_context() {
        // Below the floor the model starts repeating itself; see MIN_AUDIO_CTX.
        assert_eq!(audio_ctx_for(seconds(1.0)), MIN_AUDIO_CTX as i32);
        assert_eq!(audio_ctx_for(seconds(3.0)), MIN_AUDIO_CTX as i32);
        assert_eq!(audio_ctx_for(0), MIN_AUDIO_CTX as i32);
    }

    #[test]
    fn long_phrases_get_more_context_in_proportion() {
        let ten = audio_ctx_for(seconds(10.0));
        let twenty = audio_ctx_for(seconds(20.0));
        assert!(ten > MIN_AUDIO_CTX as i32, "10s should need more than the floor");
        assert!(twenty > ten, "20s should need more context than 10s");
    }

    #[test]
    fn encoder_context_never_exceeds_the_models_window() {
        // Whisper asserts on anything above its trained window.
        assert_eq!(audio_ctx_for(seconds(30.0)), FULL_AUDIO_CTX as i32);
        assert_eq!(audio_ctx_for(seconds(120.0)), FULL_AUDIO_CTX as i32);
    }

    #[test]
    fn context_always_covers_the_audio_with_room_to_spare() {
        // The whole point: too small and the tail of the phrase is cut off.
        for secs in [1.0f32, 2.5, 5.0, 8.0, 12.0, 15.0, 25.0] {
            let samples = seconds(secs);
            let needed = (samples / 320) as i32;
            let given = audio_ctx_for(samples);
            assert!(
                given >= needed + 64,
                "{secs}s needs {needed} frames but was given only {given}"
            );
        }
    }

    #[test]
    fn a_phrase_at_the_hard_commit_limit_still_fits() {
        // MAX_PHRASE_SAMPLES is the longest clip the live path can produce.
        let given = audio_ctx_for(MAX_PHRASE_SAMPLES);
        let needed = (MAX_PHRASE_SAMPLES / 320) as i32;
        assert!(given >= needed + 64);
        assert!(given <= FULL_AUDIO_CTX as i32);
    }
}
