//! System-wide hotkeys.
//!
//! These have to work while another window has focus -- the whole point is to
//! narrate while testing a different GUI -- so they are registered with the OS
//! rather than handled as egui key events.

use std::sync::{Arc, Mutex};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::config::HotkeySpec;

/// What a hotkey press means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ToggleListening,
    NewBullet,
}

/// Which registration each press belongs to, shared with the listener thread.
#[derive(Default)]
struct Registered {
    toggle: Option<u32>,
    bullet: Option<u32>,
}

/// Holds the OS registrations. Dropping it releases them.
pub struct Hotkeys {
    manager: Option<GlobalHotKeyManager>,
    toggle: Option<HotKey>,
    bullet: Option<HotKey>,
    registered: Arc<Mutex<Registered>>,
    /// Presses the listener thread has seen and the window has not yet read.
    pending: Arc<Mutex<Vec<Action>>>,
    /// Why the last [`Hotkeys::apply`] did not fully work, if it did not.
    pub warning: Option<String>,
}

impl Hotkeys {
    /// `ctx` is how a press wakes the window.
    ///
    /// Presses arrive on a thread of the hotkey library's own, and the window
    /// only runs when something asks it to. Polling from the draw loop alone
    /// meant a press while the window sat idle was never noticed -- the whole
    /// point of a system-wide hotkey is that the window is idle.
    pub fn new(ctx: egui::Context) -> Self {
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
        let registered: Arc<Mutex<Registered>> = Arc::default();
        let pending: Arc<Mutex<Vec<Action>>> = Arc::default();

        if manager.is_some() {
            let registered = Arc::clone(&registered);
            let pending = Arc::clone(&pending);
            std::thread::Builder::new()
                .name("voice-notepad-hotkeys".into())
                .spawn(move || {
                    let receiver = GlobalHotKeyEvent::receiver();
                    while let Ok(event) = receiver.recv() {
                        // Act on press, not release, so holding the key does
                        // not double up.
                        if event.state != HotKeyState::Pressed {
                            continue;
                        }
                        let action = {
                            let ids = match registered.lock() {
                                Ok(ids) => ids,
                                Err(_) => break,
                            };
                            if Some(event.id) == ids.toggle {
                                Some(Action::ToggleListening)
                            } else if Some(event.id) == ids.bullet {
                                Some(Action::NewBullet)
                            } else {
                                None
                            }
                        };
                        if let Some(action) = action {
                            if let Ok(mut queue) = pending.lock() {
                                queue.push(action);
                            }
                            ctx.request_repaint();
                        }
                    }
                })
                .ok();
        }

        Hotkeys {
            manager,
            toggle: None,
            bullet: None,
            registered,
            pending,
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
        if let Ok(mut ids) = self.registered.lock() {
            ids.toggle = None;
            ids.bullet = None;
        }

        let mut problems: Vec<String> = Vec::new();

        for (spec, slot, what) in [
            (toggle, 0usize, "listening"),
            (bullet, 1usize, "new bullet"),
        ] {
            match to_hotkey(spec) {
                Some(hotkey) => match manager.register(hotkey) {
                    Ok(()) => {
                        if let Ok(mut ids) = self.registered.lock() {
                            if slot == 0 {
                                self.toggle = Some(hotkey);
                                ids.toggle = Some(hotkey.id());
                            } else {
                                self.bullet = Some(hotkey);
                                ids.bullet = Some(hotkey.id());
                            }
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
        match self.pending.lock() {
            Ok(mut queue) => std::mem::take(&mut *queue),
            Err(_) => Vec::new(),
        }
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
