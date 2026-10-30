//! Voice Notepad.
//!
//! A single window that looks and behaves like Windows Notepad, with local
//! speech-to-text behind one mic button.
//!
//!     VoiceNotepad [file]

// No console window in a release build; keep it in debug so panics are visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use voice_notepad::diagnostics;
use voice_notepad::ui::VoiceNotepad;

fn main() -> eframe::Result<()> {
    // Before anything else: a panic with no console behind it leaves nothing
    // to go on otherwise.
    diagnostics::install_panic_logger();
    // On Windows ONNX Runtime is a DLL beside the exe rather than linked in.
    voice_notepad::stt::prepare_runtime();

    if std::env::args_os().any(|a| a == "--version" || a == "-V") {
        println!("{}", diagnostics::version_line());
        return Ok(());
    }

    // Like Notepad: a path on the command line is opened at startup. This is
    // also what lets the desktop entry be a handler for text files.
    let open: Option<PathBuf> = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .filter(|p| !p.to_string_lossy().starts_with('-'));

    if std::env::args_os().any(|a| a == "--help" || a == "-h") {
        println!("{}\n", diagnostics::version_line());
        println!("usage: VoiceNotepad [file]\n");
        println!("A Notepad look-alike with local speech to text.");
        println!("Ctrl+Alt+Space starts and stops listening; Ctrl+` makes a bullet.");
        println!("\noptions:");
        println!("  --check-mic     test the microphone and report what happened");
        println!("  -V, --version   print the version and exit");
        println!("  -h, --help      print this and exit");
        return Ok(());
    }

    // A diagnostic for "I pressed the mic and nothing happened". Writes to
    // the log as well as the terminal, because on Windows this is a windowed
    // program with no console attached to print to.
    if std::env::args_os().any(|a| a == "--check-mic") {
        let config = voice_notepad::config::Config::load();
        let report = voice_notepad::audio::check(
            config.input_device.as_deref(),
            std::time::Duration::from_secs(3),
        );
        let full = format!("{}\n{}", diagnostics::version_line(), report);
        println!("{full}");
        diagnostics::append(&format!("---\n{full}"));
        if let Some(path) = voice_notepad::config::log_file() {
            println!("(also written to {})", path.display());
        }
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
