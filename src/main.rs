//! Voice Notepad.
//!
//! A single window that looks and behaves like Windows Notepad, with local
//! speech-to-text behind one mic button.

// No console window in a release build; keep it in debug so panics are visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use voice_notepad::ui::VoiceNotepad;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Untitled - Voice Notepad")
            .with_inner_size([920.0, 640.0])
            .with_min_inner_size([420.0, 280.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Voice Notepad",
        options,
        Box::new(|cc| Ok(Box::new(VoiceNotepad::new(cc)))),
    )
}
