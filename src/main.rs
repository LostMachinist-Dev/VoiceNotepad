//! Voice Notepad.
//!
//! A single window that looks and behaves like Windows Notepad, with local
//! speech-to-text behind one mic button.
//!
//!     VoiceNotepad [file]

// No console window in a release build; keep it in debug so panics are visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use voice_notepad::ui::VoiceNotepad;

fn main() -> eframe::Result<()> {
    // Like Notepad: a path on the command line is opened at startup. This is
    // also what lets the desktop entry be a handler for text files.
    let open: Option<PathBuf> = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .filter(|p| p.as_os_str() != "--help" && p.as_os_str() != "-h");

    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!("Voice Notepad {}\n", env!("CARGO_PKG_VERSION"));
        println!("usage: VoiceNotepad [file]\n");
        println!("A Notepad look-alike with local speech to text.");
        println!("Ctrl+Alt+Space starts and stops listening; Ctrl+Alt+B makes a bullet.");
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Untitled - Voice Notepad")
            .with_app_id("voice-notepad")
            .with_inner_size([920.0, 640.0])
            .with_min_inner_size([420.0, 280.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Voice Notepad",
        options,
        Box::new(move |cc| Ok(Box::new(VoiceNotepad::new(cc, open)))),
    )
}
