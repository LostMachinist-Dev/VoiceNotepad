//! Settings and where they live on disk.
//!
//! Everything the Settings dialog can change is in [`Config`], which is
//! serialised to JSON in the user's app-data folder. Unknown or corrupt files
//! fall back to defaults rather than stopping the app from starting -- a note
//! taker should never be blocked by its own preferences file.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Which theme to use, or to follow the operating system.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePref {
    pub const ALL: [ThemePref; 3] = [ThemePref::System, ThemePref::Light, ThemePref::Dark];

    pub fn label(self) -> &'static str {
        match self {
            ThemePref::System => "Follow system",
            ThemePref::Light => "Light",
            ThemePref::Dark => "Dark",
        }
    }
}

/// Whisper model size. These are the English-only models: faster and more
/// accurate than the multilingual ones for dictation, which is all this does.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum ModelSize {
    Tiny,
    Base,
    #[default]
    Small,
    Medium,
}

impl ModelSize {
    pub const ALL: [ModelSize; 4] = [
        ModelSize::Tiny,
        ModelSize::Base,
        ModelSize::Small,
        ModelSize::Medium,
    ];

    /// The ggml file name, which is also the name used on Hugging Face.
    pub fn file_name(self) -> &'static str {
        match self {
            ModelSize::Tiny => "ggml-tiny.en.bin",
            ModelSize::Base => "ggml-base.en.bin",
            ModelSize::Small => "ggml-small.en.bin",
            ModelSize::Medium => "ggml-medium.en.bin",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ModelSize::Tiny => "tiny.en",
            ModelSize::Base => "base.en",
            ModelSize::Small => "small.en",
            ModelSize::Medium => "medium.en",
        }
    }

    /// Rough download size, shown in Settings so the choice is informed.
    pub fn approx_size(self) -> &'static str {
        match self {
            ModelSize::Tiny => "75 MB",
            ModelSize::Base => "142 MB",
            ModelSize::Small => "466 MB",
            ModelSize::Medium => "1.5 GB",
        }
    }

    /// A one-line hint about the speed/accuracy trade-off.
    pub fn hint(self) -> &'static str {
        match self {
            ModelSize::Tiny => "fastest, least accurate",
            ModelSize::Base => "fast",
            ModelSize::Small => "recommended balance",
            ModelSize::Medium => "most accurate, needs a strong CPU",
        }
    }
}

/// One key combination, stored in settings as text like `Ctrl+Alt+Space`.
///
/// Kept free of any hotkey-library types so it can be parsed, displayed and
/// tested without the GUI half of the crate being built.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct HotkeySpec {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    /// Canonical key name: `A`..`Z`, `0`..`9`, `Space`, `F1`, `Enter`, ...
    pub key: String,
}

impl HotkeySpec {
    /// Parse `Ctrl+Alt+Space` and friends. Separators and case are forgiving;
    /// the last component is the key, everything before it a modifier.
    ///
    /// Returns `None` for an empty spec or an unknown modifier, so the caller
    /// can keep the previous binding rather than silently losing a hotkey.
    pub fn parse(text: &str) -> Option<Self> {
        let mut spec = HotkeySpec::default();
        let parts: Vec<&str> = text
            .split('+')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        let (key, mods) = parts.split_last()?;

        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => spec.ctrl = true,
                "alt" | "option" => spec.alt = true,
                "shift" => spec.shift = true,
                "win" | "super" | "meta" | "cmd" | "command" => spec.meta = true,
                _ => return None,
            }
        }

        spec.key = canonical_key(key)?;
        Some(spec)
    }

    /// True when no modifier is held. Such a binding would swallow a plain
    /// keystroke system-wide, so Settings refuses it.
    pub fn is_bare(&self) -> bool {
        !(self.ctrl || self.alt || self.shift || self.meta)
    }
}

impl fmt::Display for HotkeySpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            write!(f, "Ctrl+")?;
        }
        if self.alt {
            write!(f, "Alt+")?;
        }
        if self.shift {
            write!(f, "Shift+")?;
        }
        if self.meta {
            write!(f, "Win+")?;
        }
        write!(f, "{}", self.key)
    }
}

/// Normalise a key name: single letters and digits upper-case, named keys
/// title-cased against a known list.
fn canonical_key(key: &str) -> Option<String> {
    if key.is_empty() {
        return None;
    }
    let upper = key.to_ascii_uppercase();

    if upper.len() == 1 {
        let c = upper.chars().next()?;
        if c.is_ascii_alphanumeric() {
            return Some(upper);
        }
    }

    // Function keys.
    if let Some(n) = upper.strip_prefix('F') {
        if let Ok(n) = n.parse::<u8>() {
            if (1..=24).contains(&n) {
                return Some(format!("F{n}"));
            }
        }
    }

    const NAMED: [&str; 16] = [
        "Space", "Enter", "Tab", "Escape", "Backspace", "Delete", "Insert", "Home", "End",
        "PageUp", "PageDown", "Up", "Down", "Left", "Right", "Backquote",
    ];
    NAMED
        .iter()
        .find(|n| n.eq_ignore_ascii_case(key))
        .map(|n| (*n).to_string())
}

fn default_toggle_hotkey() -> String {
    "Ctrl+Alt+Space".to_string()
}

fn default_bullet_hotkey() -> String {
    "Ctrl+Alt+B".to_string()
}

fn default_zoom() -> f32 {
    1.0
}

fn default_word_wrap() -> bool {
    true
}

/// Everything persisted between runs.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub theme: ThemePref,
    pub model: ModelSize,
    #[serde(default = "default_toggle_hotkey")]
    pub toggle_hotkey: String,
    #[serde(default = "default_bullet_hotkey")]
    pub bullet_hotkey: String,
    /// Input device name, or `None` for the system default.
    pub input_device: Option<String>,
    #[serde(default = "default_word_wrap")]
    pub word_wrap: bool,
    #[serde(default = "default_zoom")]
    pub zoom: f32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: ThemePref::default(),
            model: ModelSize::default(),
            toggle_hotkey: default_toggle_hotkey(),
            bullet_hotkey: default_bullet_hotkey(),
            input_device: None,
            word_wrap: default_word_wrap(),
            zoom: default_zoom(),
        }
    }
}

impl Config {
    /// Read the config file, falling back to defaults if it is missing or
    /// unreadable. Never fails: bad settings must not stop the app starting.
    pub fn load() -> Self {
        let Some(path) = config_file() else {
            return Config::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Config::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    /// Write the config file, creating its folder if needed.
    pub fn save(&self) -> std::io::Result<()> {
        let path = config_file().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "could not locate an app data folder",
            )
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)
    }

    pub fn toggle_spec(&self) -> HotkeySpec {
        HotkeySpec::parse(&self.toggle_hotkey)
            .unwrap_or_else(|| HotkeySpec::parse(&default_toggle_hotkey()).expect("valid default"))
    }

    pub fn bullet_spec(&self) -> HotkeySpec {
        HotkeySpec::parse(&self.bullet_hotkey)
            .unwrap_or_else(|| HotkeySpec::parse(&default_bullet_hotkey()).expect("valid default"))
    }

    /// Clamp zoom to the range the View menu offers.
    pub fn clamped_zoom(&self) -> f32 {
        self.zoom.clamp(0.5, 3.0)
    }
}

/// `%APPDATA%\VoiceNotepad\config` on Windows, `~/.config/VoiceNotepad` on Linux.
pub fn config_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "VoiceNotepad")
        .map(|d| d.config_dir().to_path_buf())
}

pub fn config_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.json"))
}

/// Where downloaded Whisper models are kept.
pub fn model_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "VoiceNotepad")
        .map(|d| d.data_dir().join("models"))
}

/// A model shipped next to the exe takes precedence over the download folder,
/// so an installer can bundle one and the app will find it with no network.
pub fn bundled_model_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("models"))
}

/// Look for `size` next to the exe first, then in the download folder.
pub fn find_model(size: ModelSize) -> Option<PathBuf> {
    let name = size.file_name();
    for dir in [bundled_model_dir(), model_dir()].into_iter().flatten() {
        let candidate = dir.join(name);
        if is_plausible_model(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// A ggml model is tens of megabytes at least; anything smaller is a truncated
/// or failed download and should be treated as absent.
pub fn is_plausible_model(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 10 * 1024 * 1024)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_documented_hotkeys() {
        let c = Config::default();
        assert_eq!(c.toggle_hotkey, "Ctrl+Alt+Space");
        assert_eq!(c.bullet_hotkey, "Ctrl+Alt+B");
        assert_eq!(c.model, ModelSize::Small);
        assert_eq!(c.theme, ThemePref::System);
    }

    #[test]
    fn parses_the_default_hotkeys() {
        let s = HotkeySpec::parse("Ctrl+Alt+Space").unwrap();
        assert!(s.ctrl && s.alt && !s.shift && !s.meta);
        assert_eq!(s.key, "Space");

        let b = HotkeySpec::parse("Ctrl+Alt+B").unwrap();
        assert_eq!(b.key, "B");
    }

    #[test]
    fn hotkey_parsing_is_case_and_space_insensitive() {
        let a = HotkeySpec::parse("ctrl + alt + space").unwrap();
        let b = HotkeySpec::parse("CTRL+ALT+SPACE").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.to_string(), "Ctrl+Alt+Space");
    }

    #[test]
    fn hotkeys_round_trip_through_display() {
        for text in ["Ctrl+Alt+Space", "Ctrl+Alt+B", "Ctrl+Shift+F9", "Win+D", "Alt+Up"] {
            let spec = HotkeySpec::parse(text).unwrap();
            assert_eq!(spec.to_string(), text, "round trip failed for {text}");
            assert_eq!(HotkeySpec::parse(&spec.to_string()).unwrap(), spec);
        }
    }

    #[test]
    fn rejects_unknown_modifiers_and_keys() {
        assert!(HotkeySpec::parse("Hyper+X").is_none());
        assert!(HotkeySpec::parse("Ctrl+Alt+Wiggle").is_none());
        assert!(HotkeySpec::parse("").is_none());
        assert!(HotkeySpec::parse("+").is_none());
        assert!(HotkeySpec::parse("Ctrl+F99").is_none());
    }

    #[test]
    fn detects_a_binding_with_no_modifier() {
        assert!(HotkeySpec::parse("Space").unwrap().is_bare());
        assert!(!HotkeySpec::parse("Ctrl+Space").unwrap().is_bare());
    }

    #[test]
    fn config_survives_a_round_trip_through_json() {
        let mut c = Config::default();
        c.model = ModelSize::Medium;
        c.theme = ThemePref::Dark;
        c.zoom = 1.5;
        c.input_device = Some("Headset Mic".into());
        let json = serde_json::to_string(&c).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.model, ModelSize::Medium);
        assert_eq!(back.theme, ThemePref::Dark);
        assert_eq!(back.zoom, 1.5);
        assert_eq!(back.input_device.as_deref(), Some("Headset Mic"));
    }

    #[test]
    fn partial_config_file_fills_in_defaults() {
        // An older or hand-edited file must not lose the other settings.
        let c: Config = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(c.theme, ThemePref::Dark);
        assert_eq!(c.toggle_hotkey, "Ctrl+Alt+Space");
        assert_eq!(c.model, ModelSize::Small);
    }

    #[test]
    fn model_file_names_are_the_english_ggml_ones() {
        assert_eq!(ModelSize::Small.file_name(), "ggml-small.en.bin");
        assert_eq!(ModelSize::Tiny.file_name(), "ggml-tiny.en.bin");
        for size in ModelSize::ALL {
            assert!(size.file_name().starts_with("ggml-"));
            assert!(size.file_name().ends_with(".en.bin"));
        }
    }

    #[test]
    fn zoom_is_clamped_to_the_menu_range() {
        let mut c = Config::default();
        c.zoom = 99.0;
        assert_eq!(c.clamped_zoom(), 3.0);
        c.zoom = 0.01;
        assert_eq!(c.clamped_zoom(), 0.5);
    }

    #[test]
    fn a_broken_hotkey_string_falls_back_to_the_default() {
        let mut c = Config::default();
        c.toggle_hotkey = "nonsense!!".into();
        assert_eq!(c.toggle_spec().to_string(), "Ctrl+Alt+Space");
    }
}
