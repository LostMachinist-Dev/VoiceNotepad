//! The window: menu bar, text area, status bar, and the dialogs.
//!
//! Layout follows Notepad closely -- one window, a menu bar, a plain text
//! area, a status bar -- with exactly two additions at the right end of the
//! menu bar: a theme toggle and a mic button.
//!
//! The on-screen string is the document (see [`crate::document`]). Keyboard
//! handling runs *before* the text area is drawn, so Tab, Enter and the
//! clipboard shortcuts can be intercepted and given bullet-aware or
//! markdown-aware behaviour before egui's own handling sees them.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{Align, Key, Layout, Modifiers, Pos2, Rect, Sense, Stroke, Vec2};

use crate::config::{Config, HotkeySpec, SpeechModel, ThemePref};
use crate::document::{self, LineEnding};
use crate::hotkeys::{self, Hotkeys};
use crate::markdown;
use crate::model;
use crate::stt::{Command, Event, SpeechEngine};
use crate::theme::{self, Appearance, Palette};
use crate::transcript;

/// How long a status-bar message stays up.
const MESSAGE_LIFETIME: Duration = Duration::from_secs(4);

/// Typing within this window of the last keystroke joins the same undo step.
const UNDO_COALESCE: Duration = Duration::from_millis(600);

fn editor_id() -> egui::Id {
    egui::Id::unique("voice-notepad-editor")
}

/// One undo step.
#[derive(Clone)]
struct Snapshot {
    text: String,
    cursor: usize,
}

/// Where the model is up to.
enum ModelStatus {
    Idle,
    Downloading {
        received: u64,
        total: Option<u64>,
        file: String,
    },
    Loading,
    Ready,
    Failed(String),
}

/// An action that first needs the document saved or discarded.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    New,
    Open,
    Exit,
}

/// Which hotkey Settings is currently capturing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HotkeyField {
    Toggle,
    Bullet,
}

pub struct VoiceNotepad {
    config: Config,
    engine: SpeechEngine,
    hotkeys: Hotkeys,
    appearance: Appearance,
    /// The preference the current `appearance` was derived from, so we only
    /// re-apply the theme when something actually changed.
    applied_for: Option<(ThemePref, Appearance)>,

    text: String,
    path: Option<PathBuf>,
    dirty: bool,
    line_ending: LineEnding,

    cursor: usize,
    selection: (usize, usize),
    /// Set when we have moved the cursor ourselves; written into egui's state
    /// after the text area is drawn.
    pending_cursor: Option<(usize, usize)>,
    focus_editor: bool,

    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    text_at_frame_start: String,
    last_user_edit: Option<Instant>,

    listening: bool,
    model_status: ModelStatus,
    /// When dictated text was last appended, so a run of it collapses into
    /// one undo step rather than one per chunk.
    last_dictation: Option<Instant>,
    input_level: f32,

    show_settings: bool,
    show_about: bool,
    show_find: bool,
    find_query: String,
    find_match_case: bool,
    find_message: Option<String>,
    capture_hotkey: Option<HotkeyField>,
    hotkey_draft: (String, String),
    devices: Vec<String>,
    /// Result of Settings > Test microphone, filled in by a worker thread.
    /// On Windows there is no console to print a diagnostic to, so it has to
    /// be somewhere the user can actually see it.
    mic_report: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    mic_test_running: bool,
    draft: Config,

    /// Why the hotkeys may not reach us here, if the platform can tell.
    hotkey_caveat: Option<crate::platform::HotkeyCaveat>,
    message: Option<(String, Instant)>,
    /// The title last sent to the window manager, so it is only sent again
    /// when it has actually changed.
    last_title: String,
    error: Option<String>,
    confirm: Option<Pending>,
    closing: bool,
}

impl VoiceNotepad {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        let config = Config::load();
        let appearance = theme::resolve(config.theme);
        theme::apply(&cc.egui_ctx, appearance);

        let engine = SpeechEngine::spawn(
            config.model,
            config.input_device.clone(),
            cc.egui_ctx.clone(),
        );
        // Get the model on disk straight away, so the progress bar appears on
        // first run rather than the first time the mic button is pressed.
        engine.send(Command::EnsureModel);

        let mut hotkeys = Hotkeys::new(cc.egui_ctx.clone());
        hotkeys.apply(&config.toggle_spec(), &config.bullet_spec());
        // Either the registration failed, or it succeeded somewhere the keys
        // will never arrive from. Both are worth saying out loud.
        let hotkey_caveat = crate::platform::hotkey_caveat();
        let hotkey_warning = hotkeys
            .warning
            .clone()
            .or_else(|| hotkey_caveat.as_ref().map(|c| c.short.clone()));

        let devices = crate::audio::list_input_devices();

        let mut app = VoiceNotepad {
            draft: config.clone(),
            hotkey_draft: (config.toggle_hotkey.clone(), config.bullet_hotkey.clone()),
            config,
            engine,
            hotkeys,
            appearance,
            applied_for: None,
            text: String::new(),
            path: None,
            dirty: false,
            line_ending: LineEnding::default(),
            cursor: 0,
            selection: (0, 0),
            pending_cursor: None,
            focus_editor: true,
            undo: Vec::new(),
            redo: Vec::new(),
            text_at_frame_start: String::new(),
            last_user_edit: None,
            listening: false,
            model_status: ModelStatus::Idle,
            last_dictation: None,
            input_level: 0.0,
            show_settings: false,
            show_about: false,
            show_find: false,
            find_query: String::new(),
            find_match_case: false,
            find_message: None,
            capture_hotkey: None,
            devices,
            mic_report: std::sync::Arc::new(std::sync::Mutex::new(None)),
            mic_test_running: false,
            hotkey_caveat,
            message: hotkey_warning.clone().map(|w| (w, Instant::now())),
            last_title: String::new(),
            error: None,
            confirm: None,
            closing: false,
        };

        if let Some(path) = open {
            app.load_path(&path);
        }
        app
    }

    // --- document helpers -------------------------------------------------

    fn file_label(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "Untitled".to_string())
    }

    fn title(&self) -> String {
        format!(
            "{}{} - Voice Notepad",
            if self.dirty { "*" } else { "" },
            self.file_label()
        )
    }

    fn snapshot(&mut self) {
        self.undo.push(Snapshot {
            text: self.text.clone(),
            cursor: self.cursor,
        });
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Apply a programmatic edit: record undo, set text, move the cursor.
    fn edit(&mut self, new_text: String, cursor: usize) {
        self.snapshot();
        self.text = new_text;
        self.set_cursor(cursor);
        self.dirty = true;
    }

    fn set_cursor(&mut self, at: usize) {
        self.cursor = at;
        self.selection = (at, at);
        self.pending_cursor = Some((at, at));
    }

    fn set_selection(&mut self, start: usize, end: usize) {
        self.cursor = end;
        self.selection = (start, end);
        self.pending_cursor = Some((start, end));
    }

    fn note(&mut self, message: impl Into<String>) {
        self.message = Some((message.into(), Instant::now()));
    }

    fn slice_chars(&self, start: usize, end: usize) -> String {
        self.text
            .chars()
            .skip(start)
            .take(end.saturating_sub(start))
            .collect()
    }

    fn char_before(&self, index: usize) -> Option<char> {
        if index == 0 {
            return None;
        }
        self.text.chars().nth(index - 1)
    }

    // --- file operations ---------------------------------------------------

    fn do_new(&mut self) {
        self.text.clear();
        self.path = None;
        self.dirty = false;
        self.line_ending = LineEnding::default();
        self.undo.clear();
        self.redo.clear();
        self.last_dictation = None;
        self.set_cursor(0);
        self.note("New note");
    }

    fn do_open(&mut self) {
        let picked = rfd::FileDialog::new()
            .add_filter("Text and markdown", &["txt", "md", "markdown"])
            .add_filter("All files", &["*"])
            .pick_file();
        let Some(path) = picked else { return };
        self.load_path(&path);
    }

    /// Read a file into the document. Used by File > Open and by a path given
    /// on the command line.
    pub fn load_path(&mut self, path: &std::path::Path) {
        match std::fs::read(path) {
            Ok(bytes) => {
                let raw = String::from_utf8_lossy(&bytes).to_string();
                self.line_ending = LineEnding::detect(&raw);
                let normalised = document::normalize_newlines(&raw);
                // Only a markdown file is read back as bullets; a .txt file
                // must survive untouched, dashes and all.
                let is_markdown = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
                    .unwrap_or(false);
                self.text = if is_markdown {
                    markdown::from_markdown(&normalised)
                } else {
                    normalised
                };
                self.path = Some(path.to_path_buf());
                self.dirty = false;
                self.undo.clear();
                self.redo.clear();
                self.last_dictation = None;
                self.set_cursor(0);
                self.note(format!("Opened {}", self.file_label()));
            }
            Err(err) => {
                self.error = Some(format!("Could not open {}: {err}", path.display()));
            }
        }
    }

    /// Save, asking for a path if there is not one yet. Returns true on success.
    fn do_save(&mut self) -> bool {
        match self.path.clone() {
            Some(path) => self.write_to(&path),
            None => self.do_save_as(),
        }
    }

    fn do_save_as(&mut self) -> bool {
        let picked = rfd::FileDialog::new()
            .set_file_name(format!("{}.md", self.file_label().trim_end_matches(".md")))
            .add_filter("Markdown", &["md"])
            .add_filter("Text", &["txt"])
            .save_file();
        let Some(path) = picked else { return false };
        if self.write_to(&path) {
            self.path = Some(path);
            true
        } else {
            false
        }
    }

    fn write_to(&mut self, path: &std::path::Path) -> bool {
        // Saving writes markdown: that is the whole point of the format rules.
        let md = markdown::to_markdown(&self.text);
        let out = match self.line_ending {
            LineEnding::Lf => md,
            LineEnding::Crlf => md.replace('\n', "\r\n"),
        };
        match std::fs::write(path, out) {
            Ok(()) => {
                self.dirty = false;
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.note(format!("Saved {name}"));
                true
            }
            Err(err) => {
                self.error = Some(format!("Could not save to {}: {err}", path.display()));
                false
            }
        }
    }

    /// Run `action`, prompting first if there are unsaved changes.
    fn guarded(&mut self, action: Pending) {
        if self.dirty {
            self.confirm = Some(action);
        } else {
            self.perform(action);
        }
    }

    fn perform(&mut self, action: Pending) {
        match action {
            Pending::New => self.do_new(),
            Pending::Open => self.do_open(),
            Pending::Exit => self.closing = true,
        }
    }

    // --- clipboard ---------------------------------------------------------

    fn selected_text(&self) -> Option<String> {
        let (start, end) = (
            self.selection.0.min(self.selection.1),
            self.selection.0.max(self.selection.1),
        );
        if start == end {
            return None;
        }
        Some(self.slice_chars(start, end))
    }

    fn copy_selection(&mut self, ctx: &egui::Context) {
        match self.selected_text() {
            Some(selected) => {
                ctx.copy_text(markdown::to_markdown(&selected));
                self.note("Copied as markdown");
            }
            None => self.copy_all(ctx),
        }
    }

    fn copy_all(&mut self, ctx: &egui::Context) {
        ctx.copy_text(markdown::to_markdown(&self.text));
        self.note("Copied the whole note as markdown");
    }

    fn cut_selection(&mut self, ctx: &egui::Context) {
        let Some(selected) = self.selected_text() else {
            return;
        };
        ctx.copy_text(markdown::to_markdown(&selected));
        let (start, end) = (
            self.selection.0.min(self.selection.1),
            self.selection.0.max(self.selection.1),
        );
        let (text, cursor) = document::replace_range(&self.text, start..end, "");
        self.edit(text, cursor);
        self.note("Cut as markdown");
    }

    fn paste(&mut self) {
        let contents = arboard::Clipboard::new().and_then(|mut c| c.get_text());
        match contents {
            Ok(pasted) => {
                let pasted = document::normalize_newlines(&pasted);
                let (start, end) = (
                    self.selection.0.min(self.selection.1),
                    self.selection.0.max(self.selection.1),
                );
                let (text, cursor) = document::replace_range(&self.text, start..end, &pasted);
                self.edit(text, cursor);
            }
            Err(err) => self.error = Some(format!("Nothing could be pasted: {err}")),
        }
    }

    fn delete_selection(&mut self) {
        let (start, end) = (
            self.selection.0.min(self.selection.1),
            self.selection.0.max(self.selection.1),
        );
        if start == end {
            return;
        }
        let (text, cursor) = document::replace_range(&self.text, start..end, "");
        self.edit(text, cursor);
    }

    fn select_all(&mut self) {
        self.set_selection(0, self.text.chars().count());
    }

    // --- undo / redo -------------------------------------------------------

    fn do_undo(&mut self) {
        let Some(previous) = self.undo.pop() else {
            return;
        };
        self.redo.push(Snapshot {
            text: self.text.clone(),
            cursor: self.cursor,
        });
        self.text = previous.text;
        self.set_cursor(previous.cursor.min(self.text.chars().count()));
        self.dirty = true;
        self.last_dictation = None;
    }

    fn do_redo(&mut self) {
        let Some(next) = self.redo.pop() else {
            return;
        };
        self.undo.push(Snapshot {
            text: self.text.clone(),
            cursor: self.cursor,
        });
        self.text = next.text;
        self.set_cursor(next.cursor.min(self.text.chars().count()));
        self.dirty = true;
        self.last_dictation = None;
    }

    /// Record an undo step for text the user typed, coalescing a run of
    /// keystrokes into one step the way an editor should.
    fn note_typing(&mut self, before: String, cursor_before: usize) {
        let now = Instant::now();
        let fresh = self
            .last_user_edit
            .map(|t| now.duration_since(t) > UNDO_COALESCE)
            .unwrap_or(true);
        if fresh {
            self.undo.push(Snapshot {
                text: before,
                cursor: cursor_before,
            });
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        self.last_user_edit = Some(now);
        self.dirty = true;
        // Typing interrupts dictation, so the next burst starts a new step.
        self.last_dictation = None;
    }

    // --- speech ------------------------------------------------------------

    fn toggle_listening(&mut self) {
        if self.listening {
            self.engine.send(Command::StopListening);
        } else {
            self.engine.send(Command::StartListening);
        }
    }

    fn new_bullet(&mut self) {
        let (text, cursor) = document::insert_new_bullet(&self.text, self.cursor);
        self.edit(text, cursor);
        // Dictation that follows belongs to a new undo step.
        self.last_dictation = None;
    }

    /// Append recognised text at the cursor.
    ///
    /// A streaming recogniser emits text once and never revises it, so there
    /// is nothing to hold open and replace: each piece simply goes in where
    /// the cursor is. The old engine needed a whole mechanism for swapping an
    /// in-progress phrase in place; this is what is left of it.
    fn append_dictation(&mut self, text: &str) {
        let insertion = transcript::join_fragment(self.char_before(self.cursor), text);
        if insertion.is_empty() {
            return;
        }

        // One undo step per burst of speech, not per chunk of audio.
        let now = Instant::now();
        let fresh = self
            .last_dictation
            .map(|t| now.duration_since(t) > UNDO_COALESCE)
            .unwrap_or(true);
        if fresh {
            self.snapshot();
        }
        self.last_dictation = Some(now);

        let (text, cursor) = document::insert_at(&self.text, self.cursor, &insertion);
        self.text = text;
        self.set_cursor(cursor);
        self.dirty = true;
    }

    fn handle_engine_events(&mut self) {
        for event in self.engine.drain() {
            match event {
                Event::ModelDownload(progress) => match progress {
                    model::Progress::Connecting => {
                        self.model_status = ModelStatus::Downloading {
                            received: 0,
                            total: None,
                            file: "working out the size".into(),
                        }
                    }
                    model::Progress::Downloading {
                        received,
                        total,
                        file,
                    } => {
                        self.model_status = ModelStatus::Downloading {
                            received,
                            total,
                            file,
                        }
                    }
                    model::Progress::Done(_) => {
                        let was_downloading =
                            matches!(self.model_status, ModelStatus::Downloading { .. });
                        self.model_status = ModelStatus::Ready;
                        if was_downloading {
                            self.note("The speech model is ready");
                        }
                    }
                    model::Progress::Failed(message) => {
                        self.model_status = ModelStatus::Failed(message)
                    }
                },
                Event::ModelLoading => self.model_status = ModelStatus::Loading,
                Event::ModelReady => self.model_status = ModelStatus::Ready,
                Event::Listening(on) => {
                    self.listening = on;
                    if !on {
                        self.input_level = 0.0;
                    }
                }
                Event::Text(text) => self.append_dictation(&text),
                Event::Level(level) => self.input_level = level,
                Event::Error(message) => self.error = Some(message),
            }
        }
    }

    // --- keyboard ----------------------------------------------------------

    fn read_cursor(&mut self, ctx: &egui::Context) {
        if let Some(state) = TextEditState::load(ctx, editor_id()) {
            if let Some(range) = state.cursor.char_range() {
                let sorted = range.as_sorted_char_range();
                self.selection = (sorted.start.0, sorted.end.0);
                self.cursor = range.primary.index.0;
            }
        }
        let len = self.text.chars().count();
        self.cursor = self.cursor.min(len);
        self.selection.0 = self.selection.0.min(len);
        self.selection.1 = self.selection.1.min(len);
    }

    fn write_cursor(&mut self, ctx: &egui::Context) {
        let Some((start, end)) = self.pending_cursor.take() else {
            return;
        };
        let len = self.text.chars().count();
        let range = CCursorRange::two(
            CCursor::new(start.min(len)),
            CCursor::new(end.min(len)),
        );
        if let Some(mut state) = TextEditState::load(ctx, editor_id()) {
            state.cursor.set_char_range(Some(range));
            state.store(ctx, editor_id());
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        // While Settings is capturing a chord, the keyboard belongs to it.
        if self.capture_hotkey.is_some() {
            return;
        }

        // Tab and Enter belong to the document unless a dialog is holding the
        // keyboard. Testing egui's focus instead was fragile: one Tab that
        // slipped through moved focus off the text area and every later
        // keystroke went nowhere.
        let editor_has_keys = !self.show_find && !self.show_settings;

        // Enter on an empty bullet clears the marker rather than adding a
        // line. Decided before consuming, so an ordinary Enter still works.
        let clears_bullet = editor_has_keys
            && self.selection.0 == self.selection.1
            && document::enter_on_empty_bullet(&self.text, self.cursor).is_some();

        enum Act {
            New,
            Open,
            Save,
            SaveAs,
            Undo,
            Redo,
            Copy,
            CopyAll,
            Cut,
            Paste,
            SelectAll,
            Find,
            ZoomIn,
            ZoomOut,
            ZoomReset,
            Indent,
            Outdent,
            ClearBullet,
        }
        let mut acts: Vec<Act> = Vec::new();

        ctx.input_mut(|i| {
            // Most specific modifier combinations first: egui ignores extra
            // Shift and Alt when matching.
            if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::S) {
                acts.push(Act::SaveAs);
            }
            if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::C) {
                acts.push(Act::CopyAll);
            }
            if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) {
                acts.push(Act::Redo);
            }
            if i.consume_key(Modifiers::COMMAND, Key::N) {
                acts.push(Act::New);
            }
            if i.consume_key(Modifiers::COMMAND, Key::O) {
                acts.push(Act::Open);
            }
            if i.consume_key(Modifiers::COMMAND, Key::S) {
                acts.push(Act::Save);
            }
            if i.consume_key(Modifiers::COMMAND, Key::Z) {
                acts.push(Act::Undo);
            }
            if i.consume_key(Modifiers::COMMAND, Key::Y) {
                acts.push(Act::Redo);
            }
            if i.consume_key(Modifiers::COMMAND, Key::F) {
                acts.push(Act::Find);
            }
            if i.consume_key(Modifiers::COMMAND, Key::A) {
                acts.push(Act::SelectAll);
            }
            // Copy and cut go through markdown conversion, so egui must not
            // handle them itself.
            if i.consume_key(Modifiers::COMMAND, Key::C) {
                acts.push(Act::Copy);
            }
            if i.consume_key(Modifiers::COMMAND, Key::X) {
                acts.push(Act::Cut);
            }
            if i.consume_key(Modifiers::COMMAND, Key::V) {
                acts.push(Act::Paste);
            }
            if i.consume_key(Modifiers::COMMAND, Key::Plus)
                || i.consume_key(Modifiers::COMMAND, Key::Equals)
            {
                acts.push(Act::ZoomIn);
            }
            if i.consume_key(Modifiers::COMMAND, Key::Minus) {
                acts.push(Act::ZoomOut);
            }
            if i.consume_key(Modifiers::COMMAND, Key::Num0) {
                acts.push(Act::ZoomReset);
            }

            if editor_has_keys {
                if i.consume_key(Modifiers::SHIFT, Key::Tab) {
                    acts.push(Act::Outdent);
                } else if i.consume_key(Modifiers::NONE, Key::Tab) {
                    acts.push(Act::Indent);
                }
                if clears_bullet && i.consume_key(Modifiers::NONE, Key::Enter) {
                    acts.push(Act::ClearBullet);
                }
            }
        });

        for act in acts {
            match act {
                Act::New => self.guarded(Pending::New),
                Act::Open => self.guarded(Pending::Open),
                Act::Save => {
                    self.do_save();
                }
                Act::SaveAs => {
                    self.do_save_as();
                }
                Act::Undo => self.do_undo(),
                Act::Redo => self.do_redo(),
                Act::Copy => self.copy_selection(ctx),
                Act::CopyAll => self.copy_all(ctx),
                Act::Cut => self.cut_selection(ctx),
                Act::Paste => self.paste(),
                Act::SelectAll => self.select_all(),
                Act::Find => {
                    self.show_find = true;
                    self.find_message = None;
                }
                Act::ZoomIn => self.zoom_by(0.1),
                Act::ZoomOut => self.zoom_by(-0.1),
                Act::ZoomReset => {
                    self.config.zoom = 1.0;
                    let _ = self.config.save();
                }
                Act::Indent => {
                    self.reindent(false);
                    self.focus_editor = true;
                }
                Act::Outdent => {
                    self.reindent(true);
                    self.focus_editor = true;
                }
                Act::ClearBullet => {
                    if let Some((text, cursor)) =
                        document::enter_on_empty_bullet(&self.text, self.cursor)
                    {
                        self.edit(text, cursor);
                    }
                }
            }
        }
    }

    fn reindent(&mut self, outdent: bool) {
        let (start, end) = self.selection;
        match document::reindent(&self.text, start, end, outdent) {
            Some((text, new_start, new_end)) => {
                self.snapshot();
                self.text = text;
                if new_start == new_end {
                    self.set_cursor(new_start);
                } else {
                    self.set_selection(new_start, new_end);
                }
                self.dirty = true;
            }
            None if !outdent => {
                // Not on a bullet: Notepad inserts a tab.
                let (lo, hi) = (start.min(end), start.max(end));
                let (text, cursor) = document::replace_range(&self.text, lo..hi, "\t");
                self.edit(text, cursor);
            }
            None => {}
        }
    }

    fn zoom_by(&mut self, delta: f32) {
        self.config.zoom = (self.config.clamped_zoom() + delta).clamp(0.5, 3.0);
        let _ = self.config.save();
    }

    fn find_next(&mut self) {
        if self.find_query.is_empty() {
            return;
        }
        let haystack: Vec<char> = self.text.chars().collect();
        let needle: Vec<char> = self.find_query.chars().collect();

        let eq = |a: char, b: char| {
            if self.find_match_case {
                a == b
            } else {
                a.eq_ignore_ascii_case(&b) || a.to_lowercase().eq(b.to_lowercase())
            }
        };
        let matches_at = |start: usize| {
            needle
                .iter()
                .enumerate()
                .all(|(i, c)| haystack.get(start + i).is_some_and(|h| eq(*h, *c)))
        };

        let from = self.selection.0.max(self.selection.1);
        let limit = haystack.len().saturating_sub(needle.len()) + 1;
        let found = (from..limit)
            .find(|s| matches_at(*s))
            // Wrap around, which is what Notepad's Find does.
            .or_else(|| (0..from.min(limit)).find(|s| matches_at(*s)));

        match found {
            Some(start) => {
                self.set_selection(start, start + needle.len());
                self.focus_editor = true;
                self.find_message = None;
            }
            None => self.find_message = Some(format!("Cannot find \"{}\"", self.find_query)),
        }
    }
}

// ---------------------------------------------------------------------------
// drawing
// ---------------------------------------------------------------------------

impl eframe::App for VoiceNotepad {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // eframe 0.37 hands us the root Ui; panels go inside it, while
        // windows and input still go through the Context.
        let ctx = ui.ctx().clone();
        let ctx = &ctx;

        // Theme first, so everything drawn this frame agrees.
        let wanted = theme::resolve(self.config.theme);
        if self.applied_for != Some((self.config.theme, wanted)) {
            self.appearance = wanted;
            theme::apply(ctx, wanted);
            self.applied_for = Some((self.config.theme, wanted));
        }
        let palette = theme::palette(self.appearance);

        self.read_cursor(ctx);
        // Reuse the buffer rather than allocating a copy of the document
        // every frame; this runs on every repaint.
        self.text_at_frame_start.clear();
        self.text_at_frame_start.push_str(&self.text);
        let cursor_at_frame_start = self.cursor;

        self.handle_engine_events();

        for action in self.hotkeys.poll() {
            match action {
                hotkeys::Action::ToggleListening => self.toggle_listening(),
                hotkeys::Action::NewBullet => self.new_bullet(),
            }
        }

        self.handle_shortcuts(ctx);

        // Only when it changes. Sending a viewport command marks the viewport
        // dirty, so doing this every frame is a repaint loop that never
        // settles -- and with it the app never goes idle.
        let title = self.title();
        if self.last_title != title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }

        self.menu_bar(ui, &palette);
        self.status_bar(ui, &palette);
        self.editor(ui, &palette, cursor_at_frame_start);

        self.settings_window(ctx);
        self.mic_report_window(ctx);
        self.about_window(ctx);
        self.find_window(ctx);
        self.model_window(ctx);
        self.error_window(ctx);
        self.confirm_window(ctx);

        self.write_cursor(ctx);

        // Window close: ask about unsaved work first.
        if ctx.input(|i| i.viewport().close_requested()) && !self.closing {
            if self.dirty {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.confirm = Some(Pending::Exit);
            } else {
                self.closing = true;
            }
        }
        if self.closing {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        if let Some((_, at)) = &self.message {
            if at.elapsed() > MESSAGE_LIFETIME {
                self.message = None;
            }
        }

        // Only repaint continuously while there is something to animate.
        if self.listening || matches!(self.model_status, ModelStatus::Downloading { .. }) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

impl VoiceNotepad {
    fn menu_bar(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        egui::Panel::top("menu")
            .frame(
                egui::Frame::NONE
                    .fill(palette.chrome)
                    .inner_margin(egui::Margin::symmetric(2, 1)),
            )
            .show(ui, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    self.file_menu(ui);
                    self.edit_menu(ui, ctx);
                    self.view_menu(ui);
                    self.help_menu(ui);

                    // Theme toggle and mic button, at the right end.
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let mic = mic_button(ui, self.listening, palette);
                        if mic.clicked() {
                            self.toggle_listening();
                        }
                        if mic.secondary_clicked() {
                            self.open_settings();
                        }
                        mic.on_hover_text(if self.listening {
                            format!(
                                "Listening. {} to stop. Right-click for Settings.",
                                self.config.toggle_hotkey
                            )
                        } else {
                            format!(
                                "Paused. {} to start. Right-click for Settings.",
                                self.config.toggle_hotkey
                            )
                        });

                        let theme = theme_button(ui, self.appearance, palette);
                        if theme.clicked() {
                            // The toggle flips to the other appearance, which
                            // means committing to an explicit preference.
                            self.config.theme = match self.appearance.flipped() {
                                Appearance::Light => ThemePref::Light,
                                Appearance::Dark => ThemePref::Dark,
                            };
                            let _ = self.config.save();
                        }
                        theme.on_hover_text(match self.appearance {
                            Appearance::Light => "Switch to dark",
                            Appearance::Dark => "Switch to light",
                        });
                    });
                });
            });
    }

    fn open_settings(&mut self) {
        self.draft = self.config.clone();
        self.hotkey_draft = (
            self.config.toggle_hotkey.clone(),
            self.config.bullet_hotkey.clone(),
        );
        self.devices = crate::audio::list_input_devices();
        self.capture_hotkey = None;
        self.show_settings = true;
    }

    fn file_menu(&mut self, ui: &mut egui::Ui) {
        egui::menu::MenuButton::new("File").ui(ui, |ui| {
            if menu_item(ui, "New", "Ctrl+N") {
                self.guarded(Pending::New);
            }
            if menu_item(ui, "Open...", "Ctrl+O") {
                self.guarded(Pending::Open);
            }
            if menu_item(ui, "Save", "Ctrl+S") {
                self.do_save();
            }
            if menu_item(ui, "Save As...", "Ctrl+Shift+S") {
                self.do_save_as();
            }
            ui.separator();
            if menu_item(ui, "Exit", "") {
                self.guarded(Pending::Exit);
            }
        });
    }

    fn edit_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::menu::MenuButton::new("Edit").ui(ui, |ui| {
            if ui
                .add_enabled(!self.undo.is_empty(), menu_button("Undo", "Ctrl+Z"))
                .clicked()
            {
                self.do_undo();
            }
            if ui
                .add_enabled(!self.redo.is_empty(), menu_button("Redo", "Ctrl+Y"))
                .clicked()
            {
                self.do_redo();
            }
            ui.separator();
            let has_selection = self.selection.0 != self.selection.1;
            if ui
                .add_enabled(has_selection, menu_button("Cut", "Ctrl+X"))
                .clicked()
            {
                self.cut_selection(ctx);
            }
            if ui
                .add_enabled(has_selection, menu_button("Copy", "Ctrl+C"))
                .clicked()
            {
                self.copy_selection(ctx);
            }
            if menu_item(ui, "Paste", "Ctrl+V") {
                self.paste();
            }
            if ui
                .add_enabled(has_selection, menu_button("Delete", "Del"))
                .clicked()
            {
                self.delete_selection();
            }
            ui.separator();
            if menu_item(ui, "Copy All as Markdown", "Ctrl+Shift+C") {
                self.copy_all(ctx);
            }
            ui.separator();
            if menu_item(ui, "Select All", "Ctrl+A") {
                self.select_all();
            }
            if menu_item(ui, "Find...", "Ctrl+F") {
                self.show_find = true;
                self.find_message = None;
            }
        });
    }

    fn view_menu(&mut self, ui: &mut egui::Ui) {
        egui::menu::MenuButton::new("View").ui(ui, |ui| {
            if menu_item(ui, "Zoom In", "Ctrl++") {
                self.zoom_by(0.1);
            }
            if menu_item(ui, "Zoom Out", "Ctrl+-") {
                self.zoom_by(-0.1);
            }
            if menu_item(ui, "Restore Default Zoom", "Ctrl+0") {
                self.config.zoom = 1.0;
                let _ = self.config.save();
            }
            ui.separator();
            if ui
                .checkbox(&mut self.config.word_wrap, "Word Wrap")
                .changed()
            {
                let _ = self.config.save();
            }
            ui.separator();
            if ui.button("Settings...").clicked() {
                self.open_settings();
                ui.close();
            }
        });
    }

    fn help_menu(&mut self, ui: &mut egui::Ui) {
        egui::menu::MenuButton::new("Help").ui(ui, |ui| {
            if menu_item(ui, "About Voice Notepad", "") {
                self.show_about = true;
            }
        });
    }

    /// Everything someone would be asked for in a bug report, in one place
    /// and copyable in one click.
    fn about_details(&self) -> String {
        let model = self.config.model;
        let model_state = match crate::config::find_model(model) {
            Some(path) => format!("{} ({})", model.label(), path.display()),
            None => format!("{} (not downloaded yet)", model.label()),
        };
        let mut details = format!(
            "{}\nspeech model: {}\nmicrophone: {}\ntheme: {}\nhotkeys: {} / {}\n",
            crate::diagnostics::version_line(),
            model_state,
            self.config
                .input_device
                .clone()
                .unwrap_or_else(|| "system default".into()),
            self.config.theme.label(),
            self.config.toggle_hotkey,
            self.config.bullet_hotkey,
        );
        if let Some(path) = crate::config::config_file() {
            details.push_str(&format!("settings: {}\n", path.display()));
        }
        if let Some(path) = crate::config::log_file() {
            details.push_str(&format!("log: {}\n", path.display()));
        }
        if let Some(caveat) = &self.hotkey_caveat {
            details.push_str(&format!("note: {}\n", caveat.detail));
        }
        details
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let mut open = true;
        let mut close = false;
        let details = self.about_details();

        egui::Window::new("About Voice Notepad")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(440.0)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new("Voice Notepad")
                        .heading()
                        .strong(),
                );
                ui.label(
                    egui::RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                        .size(14.0),
                );
                ui.add_space(6.0);
                ui.label(
                    "A Notepad look-alike that types what you say. The speech \
                     recognition runs on this computer; nothing is sent anywhere.",
                );
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);

                egui::ScrollArea::vertical()
                    .max_height(170.0)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(details.clone())
                                .monospace()
                                .size(11.0),
                        );
                    });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button("Copy details")
                        .on_hover_text("For a bug report")
                        .clicked()
                    {
                        ui.ctx().copy_text(details.clone());
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                });
            });

        if close || !open {
            self.show_about = false;
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::NONE
                    .fill(palette.chrome)
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Left: whatever just happened.
                    if let Some((message, _)) = &self.message {
                        ui.colored_label(palette.faint_ink, message.clone());
                    } else if let ModelStatus::Failed(why) = &self.model_status {
                        ui.colored_label(palette.recording, "Speech model unavailable")
                            .on_hover_text(why.clone());
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // Right: the Notepad fields, then the recording dot.
                        if self.listening {
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                            ui.painter().circle_filled(
                                rect.center(),
                                4.0,
                                palette.recording,
                            );
                        }
                        let (line, col) = document::line_col(&self.text, self.cursor);
                        for field in [
                            "UTF-8".to_string(),
                            self.line_ending.label().to_string(),
                            format!("{}%", (self.config.clamped_zoom() * 100.0).round() as i32),
                            format!("Ln {line}, Col {col}"),
                        ] {
                            ui.colored_label(palette.faint_ink, field);
                            ui.add_space(6.0);
                        }
                    });
                });
            });
    }

    fn editor(&mut self, ui: &mut egui::Ui, palette: &Palette, cursor_at_frame_start: usize) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(palette.page))
            .show(ui, |ui| {
                let font = egui::FontId::monospace(theme::editor_font_size(
                    self.config.clamped_zoom(),
                ));

                let scroll = if self.config.word_wrap {
                    egui::ScrollArea::vertical()
                } else {
                    egui::ScrollArea::both()
                };

                scroll
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let output = egui::TextEdit::multiline(&mut self.text)
                            .id(editor_id())
                            .font(font)
                            // Inside a horizontal scroll area the available
                            // width is unbounded, so an infinite desired width
                            // is what turns word wrap off.
                            .desired_width(f32::INFINITY)
                            .desired_rows(20)
                            .frame(egui::Frame::NONE)
                            // Tab indents; it must not tab away to another
                            // widget and strand the keyboard somewhere else.
                            .lock_focus(true)
                            .margin(egui::Margin::symmetric(8, 6))
                            .show(ui);

                        if output.response.changed() {
                            let before = std::mem::take(&mut self.text_at_frame_start);
                            self.note_typing(before, cursor_at_frame_start);
                        }
                        if let Some(range) = output.cursor_range {
                            let sorted = range.as_sorted_char_range();
                            self.selection = (sorted.start.0, sorted.end.0);
                            self.cursor = range.primary.index.0;
                        }
                        if self.focus_editor {
                            output.response.request_focus();
                            self.focus_editor = false;
                        }
                    });
            });
    }

    // --- dialogs -----------------------------------------------------------

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let mut apply = false;
        let mut close = false;

        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(380.0)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.add_space(2.0);
                ui.label(egui::RichText::new("Appearance").strong());
                ui.horizontal(|ui| {
                    for pref in ThemePref::ALL {
                        ui.radio_value(&mut self.draft.theme, pref, pref.label());
                    }
                });

                ui.add_space(8.0);
                ui.label(egui::RichText::new("Speech model").strong());
                egui::ComboBox::from_id_salt("model")
                    .selected_text(format!(
                        "{} ({})",
                        self.draft.model.label(),
                        self.draft.model.approx_size()
                    ))
                    .show_ui(ui, |ui| {
                        for model in SpeechModel::ALL {
                            ui.selectable_value(
                                &mut self.draft.model,
                                model,
                                format!(
                                    "{} - {}, {}",
                                    model.label(),
                                    model.hint(),
                                    model.approx_size()
                                ),
                            );
                        }
                    });
                if self.draft.model == SpeechModel::Eou {
                    ui.small(
                        "This model writes no punctuation and no capital letters.",
                    );
                }
                if self.draft.model != self.config.model {
                    ui.small("This model will be downloaded when you press Save.");
                }

                ui.add_space(8.0);
                ui.label(egui::RichText::new("Microphone").strong());
                let selected = self
                    .draft
                    .input_device
                    .clone()
                    .unwrap_or_else(|| "System default".to_string());
                egui::ComboBox::from_id_salt("device")
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.draft.input_device,
                            None,
                            "System default",
                        );
                        for name in &self.devices {
                            ui.selectable_value(
                                &mut self.draft.input_device,
                                Some(name.clone()),
                                name,
                            );
                        }
                    });
                if self.devices.is_empty() {
                    ui.small("No microphones were found.");
                }
                ui.horizontal(|ui| {
                    let busy = self.mic_test_running;
                    let enabled = !busy && !self.listening;
                    let button = ui.add_enabled(
                        enabled,
                        egui::Button::new(if busy {
                            "Testing..."
                        } else {
                            "Test microphone"
                        })
                        .small(),
                    );
                    if self.listening {
                        button.on_hover_text("Stop listening first");
                    } else if button.clicked() {
                        self.start_mic_test(ui.ctx().clone());
                    }
                });
                if self.listening {
                    ui.add(
                        egui::ProgressBar::new((self.input_level * 6.0).min(1.0))
                            .desired_height(6.0)
                            .text("input level"),
                    );
                }

                ui.add_space(8.0);
                ui.label(egui::RichText::new("Hotkeys").strong());
                ui.small("These work even when another program has focus.");
                self.hotkey_row(ui, HotkeyField::Toggle, "Start/stop listening");
                self.hotkey_row(ui, HotkeyField::Bullet, "New bullet");
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // A mis-click while rebinding otherwise leaves no way
                        // back except editing the settings file by hand.
                        if ui
                            .small_button("Reset to defaults")
                            .on_hover_text("Ctrl+Alt+Space and Ctrl+`")
                            .clicked()
                        {
                            let defaults = Config::default();
                            self.hotkey_draft =
                                (defaults.toggle_hotkey, defaults.bullet_hotkey);
                            self.capture_hotkey = None;
                        }
                    });
                });
                if !self.hotkeys.available() {
                    ui.small("System-wide hotkeys are unavailable in this session.");
                }
                if let Some(caveat) = &self.hotkey_caveat {
                    ui.add_space(2.0);
                    ui.small(
                        egui::RichText::new(caveat.detail.clone())
                            .color(theme::palette(self.appearance).recording),
                    );
                }

                ui.add_space(12.0);
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        apply = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });

        if apply {
            self.apply_settings();
            close = true;
        }
        if close || !open {
            self.show_settings = false;
            self.capture_hotkey = None;
        }
    }

    fn hotkey_row(&mut self, ui: &mut egui::Ui, field: HotkeyField, label: &str) {
        ui.horizontal(|ui| {
            ui.label(label);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let capturing = self.capture_hotkey == Some(field);
                let current = match field {
                    HotkeyField::Toggle => self.hotkey_draft.0.clone(),
                    HotkeyField::Bullet => self.hotkey_draft.1.clone(),
                };
                let text = if capturing {
                    "press keys...".to_string()
                } else {
                    current
                };
                if ui.button(text).clicked() {
                    self.capture_hotkey = if capturing { None } else { Some(field) };
                }
            });
        });

        if self.capture_hotkey == Some(field) {
            if let Some(spec) = capture_chord(ui.ctx()) {
                if spec.is_bare() {
                    self.note("A system-wide hotkey needs at least one of Ctrl, Alt or Shift");
                } else {
                    match field {
                        HotkeyField::Toggle => self.hotkey_draft.0 = spec.to_string(),
                        HotkeyField::Bullet => self.hotkey_draft.1 = spec.to_string(),
                    }
                    self.capture_hotkey = None;
                }
            }
        }
    }

    /// Open the chosen microphone for a few seconds and report what happened.
    /// Runs off the UI thread, because it deliberately sits and listens.
    fn start_mic_test(&mut self, ctx: egui::Context) {
        self.mic_test_running = true;
        if let Ok(mut slot) = self.mic_report.lock() {
            *slot = None;
        }
        let device = self.draft.input_device.clone();
        let slot = std::sync::Arc::clone(&self.mic_report);
        std::thread::Builder::new()
            .name("voice-notepad-mic-test".into())
            .spawn(move || {
                let report = crate::audio::check(
                    device.as_deref(),
                    std::time::Duration::from_secs(3),
                );
                crate::diagnostics::append(&format!("---\n{report}"));
                if let Ok(mut slot) = slot.lock() {
                    *slot = Some(report);
                }
                ctx.request_repaint();
            })
            .ok();
    }

    fn mic_report_window(&mut self, ctx: &egui::Context) {
        let report = match self.mic_report.lock() {
            Ok(slot) => slot.clone(),
            Err(_) => None,
        };
        let Some(report) = report else { return };
        self.mic_test_running = false;

        let mut open = true;
        let mut close = false;
        egui::Window::new("Microphone test")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    ui.label(egui::RichText::new(report.clone()).monospace().size(11.0));
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(report.clone());
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                });
            });
        if close || !open {
            if let Ok(mut slot) = self.mic_report.lock() {
                *slot = None;
            }
        }
    }

    fn apply_settings(&mut self) {
        let model_changed = self.draft.model != self.config.model;
        let device_changed = self.draft.input_device != self.config.input_device;

        self.config.theme = self.draft.theme;
        self.config.model = self.draft.model;
        self.config.input_device = self.draft.input_device.clone();
        self.config.toggle_hotkey = self.hotkey_draft.0.clone();
        self.config.bullet_hotkey = self.hotkey_draft.1.clone();

        self.hotkeys
            .apply(&self.config.toggle_spec(), &self.config.bullet_spec());
        if let Some(warning) = self.hotkeys.warning.clone() {
            self.note(warning);
        }

        if device_changed {
            self.engine
                .send(Command::SetDevice(self.config.input_device.clone()));
        }
        if model_changed {
            self.engine.send(Command::SetModel(self.config.model));
        }

        match self.config.save() {
            Ok(()) => self.note("Settings saved"),
            Err(err) => self.error = Some(format!("Settings could not be saved: {err}")),
        }
    }

    fn find_window(&mut self, ctx: &egui::Context) {
        if !self.show_find {
            return;
        }
        let mut open = true;
        let mut close = false;
        egui::Window::new("Find")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(320.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Find what:");
                    let field = ui.text_edit_singleline(&mut self.find_query);
                    if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        self.find_next();
                    }
                });
                ui.checkbox(&mut self.find_match_case, "Match case");
                if let Some(message) = &self.find_message {
                    ui.colored_label(theme::palette(self.appearance).recording, message.clone());
                }
                ui.horizontal(|ui| {
                    if ui.button("Find Next").clicked() {
                        self.find_next();
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        if close || !open {
            self.show_find = false;
        }
    }

    fn model_window(&mut self, ctx: &egui::Context) {
        let (received, total, file) = match &self.model_status {
            ModelStatus::Downloading {
                received,
                total,
                file,
            } => (*received, *total, file.clone()),
            ModelStatus::Loading => {
                egui::Window::new("Preparing speech")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new());
                            ui.label("Loading the speech model...");
                        });
                    });
                return;
            }
            _ => return,
        };

        egui::Window::new("Downloading speech model")
            .collapsible(false)
            .resizable(false)
            .default_width(380.0)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!(
                    "Getting the {} model ({}). This happens once; after that \
                     everything runs offline on this computer.",
                    self.config.model.label(),
                    self.config.model.approx_size()
                ));
                ui.add_space(6.0);
                let progress = model::Progress::Downloading {
                    received,
                    total,
                    file: file.clone(),
                };
                match progress.fraction() {
                    Some(fraction) => {
                        ui.add(
                            egui::ProgressBar::new(fraction)
                                .show_percentage()
                                .desired_height(14.0),
                        );
                    }
                    None => {
                        ui.add(egui::ProgressBar::new(0.0).animate(true).desired_height(14.0));
                    }
                }
                ui.add_space(4.0);
                ui.small(match total {
                    Some(total) => format!(
                        "{} of {}",
                        model::format_bytes(received),
                        model::format_bytes(total)
                    ),
                    None => model::format_bytes(received),
                });
                ui.add_space(2.0);
                ui.small(format!("file: {file}"));
                ui.add_space(4.0);
                ui.small("You can keep typing while this finishes.");
            });
    }

    fn error_window(&mut self, ctx: &egui::Context) {
        let Some(message) = self.error.clone() else {
            return;
        };
        let mut open = true;
        let mut close = false;
        egui::Window::new("Voice Notepad")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(400.0)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(message);
                ui.add_space(8.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("OK").clicked() {
                        close = true;
                    }
                });
            });
        if close || !open {
            self.error = None;
        }
    }

    fn confirm_window(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.confirm else {
            return;
        };
        let mut decided: Option<bool> = None; // Some(true) = go ahead
        let mut cancel = false;

        egui::Window::new("Voice Notepad")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!(
                    "Do you want to save changes to {}?",
                    self.file_label()
                ));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        decided = Some(self.do_save());
                    }
                    if ui.button("Don't Save").clicked() {
                        decided = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if cancel {
            self.confirm = None;
        } else if let Some(go_ahead) = decided {
            self.confirm = None;
            if go_ahead {
                // Saving cleared the dirty flag, so this will not re-prompt.
                self.dirty = false;
                self.perform(pending);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// small widgets
// ---------------------------------------------------------------------------

/// A menu entry with its shortcut shown on the right, like Notepad's.
fn menu_button(label: &str, shortcut: &str) -> egui::Button<'static> {
    let mut button = egui::Button::new(label.to_owned());
    if !shortcut.is_empty() {
        button = button.shortcut_text(shortcut.to_owned());
    }
    button
}

fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> bool {
    let clicked = ui.add(menu_button(label, shortcut)).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// Draw the mic: grey when paused, red when listening.
///
/// Drawn rather than drawn from a font, because a microphone glyph is not
/// something we can count on being present in the system's fonts.
fn mic_button(ui: &mut egui::Ui, listening: bool, palette: &Palette) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(26.0, 22.0), Sense::click());
    let painter = ui.painter().clone();

    if response.hovered() {
        painter.rect_filled(rect, 3.0, ui.visuals().widgets.hovered.bg_fill);
    }

    let colour = if listening {
        palette.recording
    } else {
        palette.idle_mic
    };
    let centre = rect.center();

    // Capsule head.
    let head = Rect::from_center_size(
        Pos2::new(centre.x, centre.y - 3.0),
        Vec2::new(7.0, 11.0),
    );
    painter.rect_filled(head, egui::CornerRadius::same(4), colour);

    // Stem and base, which read as a microphone at this size.
    let stroke = Stroke::new(1.6, colour);
    painter.line_segment(
        [
            Pos2::new(centre.x, centre.y + 4.0),
            Pos2::new(centre.x, centre.y + 7.5),
        ],
        stroke,
    );
    painter.line_segment(
        [
            Pos2::new(centre.x - 4.5, centre.y + 7.5),
            Pos2::new(centre.x + 4.5, centre.y + 7.5),
        ],
        stroke,
    );

    response
}

/// Sun in light mode, moon in dark: press it to go the other way.
fn theme_button(ui: &mut egui::Ui, appearance: Appearance, palette: &Palette) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(26.0, 22.0), Sense::click());
    let painter = ui.painter().clone();

    if response.hovered() {
        painter.rect_filled(rect, 3.0, ui.visuals().widgets.hovered.bg_fill);
    }

    let colour = palette.ink;
    let centre = rect.center();

    match appearance {
        Appearance::Light => {
            painter.circle_filled(centre, 4.0, colour);
            let stroke = Stroke::new(1.3, colour);
            for i in 0..8 {
                let angle = i as f32 * std::f32::consts::TAU / 8.0;
                let (sin, cos) = angle.sin_cos();
                painter.line_segment(
                    [
                        centre + Vec2::new(cos, sin) * 6.0,
                        centre + Vec2::new(cos, sin) * 8.5,
                    ],
                    stroke,
                );
            }
        }
        Appearance::Dark => {
            // A crescent: a disc with a bite taken out in the chrome colour.
            painter.circle_filled(centre, 7.0, colour);
            painter.circle_filled(
                centre + Vec2::new(3.2, -2.4),
                6.2,
                palette.chrome,
            );
        }
    }

    response
}

/// Read one key combination from the input queue, for rebinding hotkeys.
///
/// Key presses are taken out of the queue so they do not also reach the
/// document while the user is choosing a binding.
fn capture_chord(ctx: &egui::Context) -> Option<HotkeySpec> {
    ctx.input_mut(|input| {
        let modifiers = input.modifiers;
        let mut pressed: Option<Key> = None;

        input.events.retain(|event| {
            if let egui::Event::Key {
                key,
                pressed: true,
                ..
            } = event
            {
                if pressed.is_none() {
                    pressed = Some(*key);
                }
                return false;
            }
            true
        });

        let key = pressed?;
        let name = canonical_key_name(key)?;
        let mut parts = Vec::new();
        if modifiers.ctrl {
            parts.push("Ctrl");
        }
        if modifiers.alt {
            parts.push("Alt");
        }
        if modifiers.shift {
            parts.push("Shift");
        }
        if modifiers.mac_cmd || modifiers.command && !modifiers.ctrl {
            parts.push("Win");
        }
        parts.push(&name);
        HotkeySpec::parse(&parts.join("+"))
    })
}

/// egui's key names mostly match ours; these are the exceptions.
fn canonical_key_name(key: Key) -> Option<String> {
    let name = key.name();
    let mapped = match name {
        "Backtick" => "Backquote",
        // Modifier keys alone are not a binding.
        "Copy" | "Cut" | "Paste" => return None,
        other => other,
    };
    Some(mapped.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_and_backtick_names_match_the_settings_parser() {
        assert_eq!(canonical_key_name(Key::ArrowUp).as_deref(), Some("Up"));
        assert_eq!(
            canonical_key_name(Key::Backtick).as_deref(),
            Some("Backquote")
        );
        assert_eq!(canonical_key_name(Key::Space).as_deref(), Some("Space"));
        assert_eq!(canonical_key_name(Key::Num1).as_deref(), Some("1"));
        assert_eq!(canonical_key_name(Key::B).as_deref(), Some("B"));
    }

    #[test]
    fn every_captured_key_name_parses_as_a_hotkey() {
        // Settings captures a key, formats it, and parses it back. If those
        // disagree the user cannot rebind that key at all.
        for key in [
            Key::A,
            Key::Z,
            Key::Num0,
            Key::Space,
            Key::Enter,
            Key::Tab,
            Key::Escape,
            Key::Backspace,
            Key::Delete,
            Key::Insert,
            Key::Home,
            Key::End,
            Key::PageUp,
            Key::PageDown,
            Key::ArrowUp,
            Key::ArrowDown,
            Key::ArrowLeft,
            Key::ArrowRight,
            Key::Backtick,
            Key::F1,
            Key::F12,
        ] {
            let name = canonical_key_name(key)
                .unwrap_or_else(|| panic!("{key:?} produced no name"));
            assert!(
                HotkeySpec::parse(&format!("Ctrl+Alt+{name}")).is_some(),
                "{key:?} -> {name} does not parse back"
            );
        }
    }

    #[test]
    fn clipboard_keys_are_not_offered_as_bindings() {
        assert_eq!(canonical_key_name(Key::Copy), None);
    }
}
