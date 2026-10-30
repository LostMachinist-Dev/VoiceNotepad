//! The speech engine: one background thread that owns the microphone and the
//! recogniser, and talks to the window over channels.
//!
//! The recogniser is a *streaming* model. Audio goes in one fixed-size chunk
//! at a time and text comes back as it is recognised, with the model keeping
//! its own state between chunks. Nothing here cuts speech into phrases, and
//! nothing re-transcribes anything: text is emitted once and appended.
//!
//! That is the whole point of this design. The previous engine wrapped
//! Whisper, which only understands complete thirty-second clips, so it had to
//! detect silences, cut the speech into phrases and transcribe each phrase on
//! its own -- which is exactly why dictation came out as disconnected
//! fragments. A streaming model removes the need for any of it.

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use parakeet_rs::{ExecutionConfig, Nemotron, ParakeetEOU, ParakeetEOUHandle};

use crate::audio::{self, AudioError, SharedAudio};
use crate::config::{find_model, model_dir, SpeechModel};
use crate::model;
use crate::resample::rms;


/// How long to wait for the first audio before deciding the microphone is not
/// actually delivering anything. Some devices open successfully and then stay
/// silent, which is indistinguishable from a broken app unless we say so.
const AUDIO_TIMEOUT: Duration = Duration::from_secs(5);

/// Silent chunks pushed through on stop, to flush the model's tail out.
const FLUSH_CHUNKS: usize = 3;

/// What the UI asks of the engine.
#[derive(Debug, Clone)]
pub enum Command {
    /// Make sure the model is on disk, downloading it if needed.
    EnsureModel,
    StartListening,
    StopListening,
    /// Microphone name, or `None` for the system default.
    SetDevice(Option<String>),
    SetModel(SpeechModel),
    Shutdown,
}

/// What the engine tells the UI.
#[derive(Debug, Clone)]
pub enum Event {
    /// First-run download progress.
    ModelDownload(model::Progress),
    /// The model is present and being loaded into memory.
    ModelLoading,
    /// Ready to transcribe.
    ModelReady,
    /// Listening started or stopped. The engine is the authority on this, so
    /// a failure to open the microphone turns the mic button back to grey.
    Listening(bool),
    /// Recognised text, to append at the cursor. Already final -- a streaming
    /// model does not revise what it has emitted.
    Text(String),
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
    ///
    /// `ctx` is how the engine wakes the window. The UI only runs when
    /// something asks it to, so an event sent without a repaint request sits
    /// in the channel unread.
    pub fn spawn(model: SpeechModel, device: Option<String>, ctx: egui::Context) -> Self {
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let (event_tx, events) = std::sync::mpsc::channel();

        let thread = std::thread::Builder::new()
            .name("voice-notepad-speech".into())
            .spawn(move || {
                let mut worker = Worker::new(model, device, event_tx, ctx);
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
// the recogniser
// ---------------------------------------------------------------------------

/// The two streaming models behind one interface.
///
/// Public so the `transcribe-file` example can drive exactly the same code
/// the microphone does, which is how the speed and accuracy numbers in
/// HANDOFF.md were measured.
pub enum Recogniser {
    Nemotron(Box<Nemotron>),
    /// The handle is kept so the stream can be restarted: unlike Nemotron,
    /// the EOU model has no reset of its own.
    Eou {
        handle: Box<ParakeetEOUHandle>,
        stream: Box<ParakeetEOU>,
    },
}

impl Recogniser {
    /// Load `model` from a folder of ONNX files.
    pub fn load(model: SpeechModel, dir: &std::path::Path) -> Result<Self, String> {
        // ONNX Runtime allows exactly one environment per process, so it is
        // built once and shared.
        let config = ExecutionConfig::new().with_environment(
            parakeet_rs::ort::init()
                .build()
                .map_err(|e| format!("ONNX Runtime could not start: {e}"))?,
        );

        match model {
            SpeechModel::Nemotron => {
                let loaded = Nemotron::from_pretrained(dir, Some(config))
                    .map_err(|e| format!("{e}"))?;
                Ok(Recogniser::Nemotron(Box::new(loaded)))
            }
            SpeechModel::Eou => {
                let handle = ParakeetEOUHandle::from_pretrained(dir, Some(config))
                    .map_err(|e| format!("{e}"))?;
                let stream = ParakeetEOU::from_shared(&handle);
                Ok(Recogniser::Eou {
                    handle: Box::new(handle),
                    stream: Box::new(stream),
                })
            }
        }
    }

    /// How much audio this model wants at a time.
    pub fn chunk_samples(&self) -> usize {
        match self {
            Recogniser::Nemotron(model) => model.chunk_samples(),
            // The EOU model documents 160 ms chunks and has no accessor.
            Recogniser::Eou { .. } => 2560,
        }
    }

    /// Feed one chunk and get back whatever text it produced. The spacing of
    /// the result is meaningful; see [`crate::transcript::join_fragment`].
    pub fn transcribe_chunk(&mut self, chunk: &[f32]) -> Result<String, String> {
        let text = match self {
            Recogniser::Nemotron(model) => model.transcribe_chunk(chunk),
            // `reset_on_eou` is false deliberately: the crate's author reports
            // it behaving poorly in practice, and for dictation an
            // end-of-utterance is not a reason to forget context anyway.
            Recogniser::Eou { stream, .. } => stream.transcribe(chunk, false),
        };
        text.map_err(|e| format!("Transcription failed: {e}"))
    }

    /// Forget what has been said so far, for a fresh listening session.
    pub fn reset(&mut self) {
        match self {
            Recogniser::Nemotron(model) => model.reset(),
            Recogniser::Eou { handle, stream } => {
                **stream = ParakeetEOU::from_shared(handle);
            }
        }
    }
}

// ---------------------------------------------------------------------------

struct Worker {
    events: Sender<Event>,
    /// Used only to wake the window after sending an event.
    ctx: egui::Context,
    model: SpeechModel,
    device: Option<String>,

    recogniser: Option<Recogniser>,
    /// Set when the loaded recogniser no longer matches `model`.
    stale: bool,

    audio_queue: SharedAudio,
    capture: Option<audio::Capture>,
    /// Audio not yet making up a whole chunk.
    pending: Vec<f32>,

    last_audio: Instant,
    audio_warned: bool,

    /// Models with a download thread already running. Switching back and
    /// forth in Settings would otherwise start a second download of the same
    /// model, and both would write the same `.part` files.
    downloading: Arc<Mutex<Vec<SpeechModel>>>,
}

impl Worker {
    fn new(
        model: SpeechModel,
        device: Option<String>,
        events: Sender<Event>,
        ctx: egui::Context,
    ) -> Self {
        Worker {
            events,
            ctx,
            model,
            device,
            recogniser: None,
            stale: false,
            audio_queue: audio::shared_audio(),
            capture: None,
            pending: Vec::new(),
            last_audio: Instant::now(),
            audio_warned: false,
            downloading: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
        // Nothing reads the channel until the window runs again.
        self.ctx.request_repaint();
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
            Command::EnsureModel => self.ensure_model_files(),
            Command::StartListening => self.start_listening(),
            Command::StopListening => self.stop_listening(),
            Command::SetDevice(device) => {
                self.device = device;
                if self.capture.is_some() {
                    self.stop_listening();
                    self.start_listening();
                }
            }
            Command::SetModel(model) => {
                if model != self.model {
                    // A different model means a different recogniser, and
                    // only start_listening loads one -- which it skips
                    // entirely while the mic is already open. Switching
                    // mid-dictation therefore used to leave the microphone
                    // running with no recogniser behind it: the button stayed
                    // red, audio piled up in `pending` for as long as it was
                    // left, and no text ever arrived. Stop and start again
                    // around the swap, exactly as SetDevice does.
                    let was_listening = self.capture.is_some();
                    if was_listening {
                        // Before the old recogniser is dropped, so the tail
                        // of what was being said still comes out.
                        self.stop_listening();
                    }
                    self.model = model;
                    self.recogniser = None;
                    self.stale = true;
                    self.pending.clear();
                    self.ensure_model_files();
                    if was_listening {
                        // If the new model still has to download, this says
                        // so and turns the button off rather than pretending.
                        self.start_listening();
                    }
                }
            }
            Command::Shutdown => {}
        }
    }

    /// Download the model if it is not already on disk.
    fn ensure_model_files(&mut self) {
        if let Some(path) = find_model(self.model) {
            self.emit(Event::ModelDownload(model::Progress::Done(path)));
            return;
        }
        let Some(dir) = model_dir() else {
            self.emit(Event::Error(
                "Could not find a folder to keep the speech model in.".into(),
            ));
            return;
        };

        let wanted = self.model;
        // Already fetching this one: let the running thread finish.
        match self.downloading.lock() {
            Ok(mut running) if !running.contains(&wanted) => running.push(wanted),
            Ok(_) => return,
            Err(_) => return,
        }

        let events = self.events.clone();
        let ctx = self.ctx.clone();
        let running = Arc::clone(&self.downloading);
        // Downloading on its own thread keeps the engine responsive, so the
        // mic button and Settings still work while gigabytes come down.
        std::thread::Builder::new()
            .name("voice-notepad-download".into())
            .spawn(move || {
                let reporter = events.clone();
                let reporter_ctx = ctx.clone();
                let result = model::download(wanted, &dir, move |progress| {
                    let _ = reporter.send(Event::ModelDownload(progress));
                    reporter_ctx.request_repaint();
                });
                if let Ok(mut running) = running.lock() {
                    running.retain(|m| *m != wanted);
                }
                if let Err(message) = result {
                    let _ = events.send(Event::ModelDownload(model::Progress::Failed(
                        message.clone(),
                    )));
                    let _ = events.send(Event::Error(message));
                }
                ctx.request_repaint();
            })
            .ok();
    }

    /// Load the model into memory. Slow, so it happens here on the engine
    /// thread rather than on the UI thread.
    fn ensure_recogniser(&mut self) -> bool {
        if self.recogniser.is_some() && !self.stale {
            return true;
        }
        let Some(path) = find_model(self.model) else {
            self.emit(Event::Error(
                "The speech model has not finished downloading yet.".into(),
            ));
            return false;
        };

        self.emit(Event::ModelLoading);
        match Recogniser::load(self.model, &path) {
            Ok(recogniser) => {
                self.recogniser = Some(recogniser);
                self.stale = false;
                self.emit(Event::ModelReady);
                true
            }
            Err(err) => {
                self.emit(Event::Error(format!(
                    "The speech model at {} could not be loaded ({err}). Try choosing a \
                     different model in Settings.",
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
        if !self.ensure_recogniser() {
            self.emit(Event::Listening(false));
            return;
        }

        if let Ok(mut queue) = self.audio_queue.lock() {
            queue.clear();
        }
        self.pending.clear();
        if let Some(recogniser) = self.recogniser.as_mut() {
            recogniser.reset();
        }
        self.last_audio = Instant::now();
        self.audio_warned = false;

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
        if self.capture.take().is_none() {
            return;
        }
        // The stream is stopped; push silence through so the last few words
        // still held in the model's cache come out.
        self.flush();
        self.pending.clear();
        self.emit(Event::Listening(false));
    }

    /// Move audio out of the capture queue and feed it to the recogniser a
    /// chunk at a time.
    fn pump_audio(&mut self) {
        let incoming: Vec<f32> = match self.audio_queue.lock() {
            Ok(mut queue) => queue.drain(..).collect(),
            Err(_) => return,
        };

        if incoming.is_empty() {
            // Opening a device can succeed and then deliver nothing at all --
            // a muted input, or one the system has handed to something else.
            if !self.audio_warned && self.last_audio.elapsed() > AUDIO_TIMEOUT {
                self.audio_warned = true;
                let device = self
                    .capture
                    .as_ref()
                    .map(|c| c.device_name.clone())
                    .unwrap_or_else(|| "the microphone".to_string());
                self.emit(Event::Error(format!(
                    "No sound is arriving from \"{device}\". Check that it is not muted, \
                     that the system is set to use it, and that nothing else has taken \
                     it. You can pick a different microphone in Settings."
                )));
            }
            return;
        }

        self.last_audio = Instant::now();
        self.audio_warned = false;
        self.emit(Event::Level(rms(&incoming)));
        self.pending.extend_from_slice(&incoming);

        let chunk_samples = match self.recogniser.as_ref() {
            Some(recogniser) => recogniser.chunk_samples(),
            // Nothing to feed the audio to. Holding on to it would grow this
            // buffer for as long as the mic stayed open, so drop it.
            None => {
                self.pending.clear();
                return;
            }
        };

        let mut consumed = 0;
        while self.pending.len() - consumed >= chunk_samples {
            let start = consumed;
            consumed += chunk_samples;
            // Disjoint borrows: the recogniser and the buffer are separate
            // fields, so this needs no copy of the chunk.
            let text = match self.recogniser.as_mut() {
                Some(recogniser) => recogniser.transcribe_chunk(&self.pending[start..consumed]),
                None => break,
            };
            match text {
                Ok(text) => self.emit_text(&text),
                Err(message) => {
                    self.emit(Event::Error(message));
                    break;
                }
            }
        }
        if consumed > 0 {
            self.pending.drain(..consumed);
        }
    }

    /// Push silence through to get the tail of the last words out.
    fn flush(&mut self) {
        let Some(chunk_samples) = self.recogniser.as_ref().map(|r| r.chunk_samples()) else {
            return;
        };
        // Whatever is left over, padded out to a whole chunk.
        if !self.pending.is_empty() {
            let mut last = std::mem::take(&mut self.pending);
            last.resize(chunk_samples.max(last.len()), 0.0);
            for chunk in last.chunks(chunk_samples) {
                if chunk.len() < chunk_samples {
                    break;
                }
                if let Some(recogniser) = self.recogniser.as_mut() {
                    if let Ok(text) = recogniser.transcribe_chunk(chunk) {
                        self.emit_text(&text);
                    }
                }
            }
        }

        let silence = vec![0.0f32; chunk_samples];
        for _ in 0..FLUSH_CHUNKS {
            let text = match self.recogniser.as_mut() {
                Some(recogniser) => recogniser.transcribe_chunk(&silence),
                None => return,
            };
            if let Ok(text) = text {
                self.emit_text(&text);
            }
        }
    }

    /// Pass the fragment on exactly as the model emitted it.
    ///
    /// The spacing matters and must survive to the UI: a leading space is the
    /// only thing that distinguishes a new word from the rest of the current
    /// one. Annotations are stripped at the point of insertion, by
    /// `transcript::join_fragment`.
    fn emit_text(&self, raw: &str) {
        if !raw.is_empty() {
            self.emit(Event::Text(raw.to_string()));
        }
    }
}

/// Make sure ONNX Runtime can be found before anything tries to use it.
///
/// On Windows the runtime is a DLL loaded at startup rather than linked in,
/// because Microsoft ships no MinGW build to link against. It lives beside
/// the exe, and this points the loader at it. Elsewhere it is linked
/// statically and there is nothing to do.
pub fn prepare_runtime() {
    #[cfg(windows)]
    {
        if std::env::var_os("ORT_DYLIB_PATH").is_some() {
            return;
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let dll = dir.join("onnxruntime.dll");
                if dll.is_file() {
                    std::env::set_var("ORT_DYLIB_PATH", dll);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    /// A worker with no microphone and no recogniser loaded, which is the
    /// state a model switch leaves it in.
    fn idle_worker() -> (Worker, Receiver<Event>) {
        let (tx, rx) = channel();
        let worker = Worker::new(SpeechModel::Nemotron, None, tx, egui::Context::default());
        (worker, rx)
    }

    #[test]
    fn audio_arriving_with_no_recogniser_is_not_hoarded() {
        // Switching models while listening used to leave the microphone open
        // with `recogniser: None`. Every pass through pump_audio appended to
        // `pending` and then returned before consuming any of it, so the
        // buffer grew for as long as the mic was left on -- about 3.8 MB a
        // minute at 16 kHz, and never transcribed.
        let (mut worker, _events) = idle_worker();
        assert!(worker.recogniser.is_none());

        for _ in 0..50 {
            if let Ok(mut queue) = worker.audio_queue.lock() {
                queue.extend(std::iter::repeat_n(0.01f32, 16_000));
            }
            worker.pump_audio();
        }

        assert!(
            worker.pending.is_empty(),
            "held {} samples with nothing to transcribe them",
            worker.pending.len()
        );
    }

    #[test]
    fn switching_models_marks_the_recogniser_stale() {
        let (mut worker, _events) = idle_worker();
        worker.stale = false;

        worker.handle(Command::SetModel(SpeechModel::Eou));

        assert_eq!(worker.model, SpeechModel::Eou);
        assert!(worker.stale, "the loaded recogniser no longer matches");
        assert!(worker.recogniser.is_none());
        assert!(worker.pending.is_empty());
    }

    #[test]
    fn switching_to_the_model_already_in_use_changes_nothing() {
        let (mut worker, _events) = idle_worker();
        worker.stale = false;
        worker.pending.push(0.5);

        worker.handle(Command::SetModel(SpeechModel::Nemotron));

        assert!(!worker.stale);
        assert_eq!(worker.pending.len(), 1, "a no-op switch dropped audio");
    }

    #[test]
    fn a_second_download_of_the_same_model_is_not_started() {
        let (worker, _events) = idle_worker();
        // Stand in for a download thread that is already running.
        worker
            .downloading
            .lock()
            .expect("lock")
            .push(SpeechModel::Nemotron);

        let before = Arc::strong_count(&worker.downloading);
        let mut worker = worker;
        worker.ensure_model_files();

        // Nothing was spawned, so nothing took a second reference.
        assert_eq!(Arc::strong_count(&worker.downloading), before);
    }
}
