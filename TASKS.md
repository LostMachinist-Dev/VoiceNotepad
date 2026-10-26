# Voice Notepad - tasks

## Done
- Surveyed the box: Rust 1.99, mingw-w64, cmake, wine 10, libclang (runtime), 24 cores. No sudo.
- Proved the whole dependency graph cross-compiles to x86_64-pc-windows-gnu.
- Found and fixed the whisper-rs-sys MinGW blocker (ggml static libs built without
  the `lib` prefix). Patch lives in scripts/vendor-whisper-sys.py.
- Core text model (src/document.rs): bullet parsing, indent/outdent, new-bullet
  hotkey, Enter-on-empty-bullet, char-safe cursor maths. 29 unit tests.
- Markdown conversion (src/markdown.rs): display <-> markdown, both directions. 13 unit tests.

## Next
- config.rs, platform.rs, model.rs (download with progress)
- audio.rs (cpal capture + resample), stt.rs (whisper worker, VAD, live partials)
- hotkeys.rs (global hotkeys), theme.rs, ui.rs (Notepad look-alike shell)
- NSIS installer under wine
- README, HANDOFF.md, study guide

## Blocked
- Native Linux run needs `sudo apt install libasound2-dev` (ALSA headers for cpal).
  Does not affect the Windows build. Linux is a future goal anyway.
- No sudo on this box, so NSIS is fetched as a portable zip and run under wine
  rather than installed with apt.
