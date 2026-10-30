//! Recording crashes so they can be reported rather than guessed at.
//!
//! The window has no console behind it on Windows, so a panic that is not
//! written down somewhere leaves nothing at all to go on -- which is exactly
//! the position a microphone crash left us in once already.

use std::io::Write;

/// Install a panic hook that appends to the log file and still prints to
/// stderr for anyone running from a terminal.
pub fn install_panic_logger() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed");
        let entry = format!(
            "---\nVoice Notepad {} ({})\nthread: {}\n{}\n",
            env!("CARGO_PKG_VERSION"),
            build_target(),
            name,
            info
        );
        append(&entry);
        previous(info);
    }));
}

/// Append a line to the log, creating it if needed. Failures are ignored --
/// there is nowhere useful to report a logging failure to.
pub fn append(entry: &str) {
    let Some(path) = crate::config::log_file() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = file.write_all(entry.as_bytes());
    }
}

/// The target triple this was built for, for bug reports.
pub fn build_target() -> &'static str {
    option_env!("VOICE_NOTEPAD_TARGET").unwrap_or("unknown")
}

/// A one-line summary of the build, shown in Help > About.
pub fn version_line() -> String {
    format!(
        "Voice Notepad {} ({})",
        env!("CARGO_PKG_VERSION"),
        build_target()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_line_names_the_version_and_the_build() {
        let line = version_line();
        assert!(line.starts_with("Voice Notepad "));
        assert!(line.contains(env!("CARGO_PKG_VERSION")));
        // Whatever the target is, it must not be left blank in a bug report.
        assert!(!build_target().is_empty());
    }
}
