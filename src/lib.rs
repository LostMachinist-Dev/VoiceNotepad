//! Voice Notepad: a Windows Notepad look-alike with local speech-to-text.
//!
//! The logic lives in a library rather than in `main.rs` so it can be unit
//! tested on any platform, including the Linux box this is cross-compiled
//! from. The modules that need a GUI, a microphone or whisper.cpp sit behind
//! the `app` feature, which is on by default; `--no-default-features` leaves
//! the text, settings, resampling, VAD and transcript logic -- the part where
//! the behaviour the spec pins down actually lives.

pub mod config;
pub mod diagnostics;
pub mod document;
pub mod markdown;
pub mod model;
pub mod platform;
pub mod resample;
pub mod transcript;

#[cfg(feature = "app")]
pub mod audio;
#[cfg(feature = "app")]
pub mod hotkeys;
#[cfg(feature = "app")]
pub mod stt;
#[cfg(feature = "app")]
pub mod theme;
#[cfg(feature = "app")]
pub mod ui;
