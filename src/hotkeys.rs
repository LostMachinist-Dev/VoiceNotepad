//! System-wide hotkeys.
//!
//! These have to work while another window has focus -- the whole point is to
//! narrate while testing a different GUI -- so they are registered with the OS
//! rather than handled as egui key events.

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::config::HotkeySpec;

/// What a hotkey press means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ToggleListening,
    NewBullet,
}

/// Holds the OS registrations. Dropping it releases them.
pub struct Hotkeys {
    manager: Option<GlobalHotKeyManager>,
    toggle: Option<HotKey>,
    bullet: Option<HotKey>,
    /// Why the last [`Hotkeys::apply`] did not fully work, if it did not.
    pub warning: Option<String>,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self::new()
    }
}

impl Hotkeys {
    pub fn new() -> Self {
        let (manager, warning) = match GlobalHotKeyManager::new() {
            Ok(manager) => (Some(manager), None),
            Err(err) => (
                None,
                Some(format!(
                    "System-wide hotkeys are unavailable ({err}). The mic button and the menus \
                     still work."
                )),
            ),
        };
        Hotkeys {
            manager,
            toggle: None,
            bullet: None,
            warning,
        }
    }

    /// True when the OS accepted at least the manager; false means hotkeys are
    /// simply not available in this session.
    pub fn available(&self) -> bool {
        self.manager.is_some()
    }

    /// Register `toggle` and `bullet`, replacing whatever was registered.
    ///
    /// A combination another program already owns cannot be registered; that
    /// is reported in [`Hotkeys::warning`] and the other binding still works,
    /// which is much better than failing both or crashing.
    pub fn apply(&mut self, toggle: &HotkeySpec, bullet: &HotkeySpec) {
        let Some(manager) = &self.manager else {
            return;
        };

        for old in [self.toggle.take(), self.bullet.take()].into_iter().flatten() {
            let _ = manager.unregister(old);
        }

        let mut problems: Vec<String> = Vec::new();

        for (spec, slot, what) in [
            (toggle, 0usize, "listening"),
            (bullet, 1usize, "new bullet"),
        ] {
            match to_hotkey(spec) {
                Some(hotkey) => match manager.register(hotkey) {
                    Ok(()) => {
                        if slot == 0 {
                            self.toggle = Some(hotkey);
                        } else {
                            self.bullet = Some(hotkey);
                        }
                    }
                    Err(err) => problems.push(format!(
                        "{spec} could not be registered for {what}; another program is probably \
                         using it ({err})."
                    )),
                },
                None => problems.push(format!("{spec} is not a key combination we can register.")),
            }
        }

        self.warning = if problems.is_empty() {
            None
        } else {
            Some(problems.join(" "))
        };
    }

    /// Everything pressed since the last call.
    pub fn poll(&self) -> Vec<Action> {
        let mut actions = Vec::new();
        if self.manager.is_none() {
            return actions;
        }
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            // Act on press, not release, so holding the key does not double up.
            if event.state != HotKeyState::Pressed {
                continue;
            }
            if Some(event.id) == self.toggle.map(|h| h.id()) {
                actions.push(Action::ToggleListening);
            } else if Some(event.id) == self.bullet.map(|h| h.id()) {
                actions.push(Action::NewBullet);
            }
        }
        actions
    }
}

/// Translate our stored spec into a registrable hotkey.
fn to_hotkey(spec: &HotkeySpec) -> Option<HotKey> {
    let mut mods = Modifiers::empty();
    if spec.ctrl {
        mods |= Modifiers::CONTROL;
    }
    if spec.alt {
        mods |= Modifiers::ALT;
    }
    if spec.shift {
        mods |= Modifiers::SHIFT;
    }
    if spec.meta {
        mods |= Modifiers::META;
    }
    let code = to_code(&spec.key)?;
    Some(HotKey::new(
        if mods.is_empty() { None } else { Some(mods) },
        code,
    ))
}

/// Map our canonical key names onto `keyboard_types::Code` spellings.
fn to_code(key: &str) -> Option<Code> {
    use std::str::FromStr;

    let name = match key {
        "Up" => "ArrowUp".to_string(),
        "Down" => "ArrowDown".to_string(),
        "Left" => "ArrowLeft".to_string(),
        "Right" => "ArrowRight".to_string(),
        k if k.len() == 1 && k.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) => {
            format!("Key{k}")
        }
        k if k.len() == 1 && k.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
            format!("Digit{k}")
        }
        // Space, Enter, Tab, F1.. and the rest already match.
        other => other.to_string(),
    };
    Code::from_str(&name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_default_bindings_to_real_key_codes() {
        assert_eq!(to_code("Space"), Some(Code::Space));
        assert_eq!(to_code("B"), Some(Code::KeyB));
    }

    #[test]
    fn maps_letters_digits_arrows_and_function_keys() {
        assert_eq!(to_code("A"), Some(Code::KeyA));
        assert_eq!(to_code("Z"), Some(Code::KeyZ));
        assert_eq!(to_code("1"), Some(Code::Digit1));
        assert_eq!(to_code("Up"), Some(Code::ArrowUp));
        assert_eq!(to_code("Right"), Some(Code::ArrowRight));
        assert_eq!(to_code("F9"), Some(Code::F9));
        assert_eq!(to_code("Enter"), Some(Code::Enter));
    }

    #[test]
    fn rejects_a_key_name_we_cannot_register() {
        assert_eq!(to_code("Wiggle"), None);
    }

    #[test]
    fn every_key_the_settings_parser_accepts_can_be_registered() {
        // The two halves must agree, or Settings would accept a binding that
        // then silently fails to register.
        let keys = [
            "A", "Q", "Z", "0", "9", "Space", "Enter", "Tab", "Escape", "Backspace", "Delete",
            "Insert", "Home", "End", "PageUp", "PageDown", "Up", "Down", "Left", "Right",
            "Backquote", "F1", "F12",
        ];
        for key in keys {
            let spec = HotkeySpec::parse(&format!("Ctrl+Alt+{key}"))
                .unwrap_or_else(|| panic!("settings rejected {key}"));
            assert!(
                to_hotkey(&spec).is_some(),
                "{key} parses but cannot be registered"
            );
        }
    }

    #[test]
    fn modifiers_are_carried_through() {
        let spec = HotkeySpec::parse("Ctrl+Alt+Space").unwrap();
        let hotkey = to_hotkey(&spec).unwrap();
        assert!(hotkey.mods.contains(Modifiers::CONTROL));
        assert!(hotkey.mods.contains(Modifiers::ALT));
        assert!(!hotkey.mods.contains(Modifiers::SHIFT));
    }

    #[test]
    fn the_two_default_hotkeys_are_different_registrations() {
        let toggle = to_hotkey(&HotkeySpec::parse("Ctrl+Alt+Space").unwrap()).unwrap();
        let bullet = to_hotkey(&HotkeySpec::parse("Ctrl+Alt+B").unwrap()).unwrap();
        assert_ne!(toggle.id(), bullet.id());
    }
}
