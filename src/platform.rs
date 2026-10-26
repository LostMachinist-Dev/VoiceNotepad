//! The few things that genuinely differ per operating system.
//!
//! Keeping them here is what makes the rest of the code portable: Windows is
//! the only target that has to work today, but nothing outside this module
//! knows that.

/// Does the system ask for a dark UI? `None` means "could not tell", and the
/// caller should pick a default rather than guess.
pub fn system_prefers_dark() -> Option<bool> {
    imp::system_prefers_dark()
}

/// A reason the system-wide hotkeys may not reach us, if there is one.
///
/// Registering a hotkey can succeed and still never fire, which is a horrible
/// thing to leave someone to work out for themselves. Where we can tell in
/// advance, we say so.
pub fn hotkey_caveat() -> Option<HotkeyCaveat> {
    imp::hotkey_caveat()
}

/// The same warning at two lengths: one that fits the status bar, and the
/// whole explanation for Settings, where there is room for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyCaveat {
    pub short: String,
    pub detail: String,
}

#[cfg(windows)]
mod imp {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    /// Windows registers hotkeys with the OS itself; nothing to warn about.
    pub fn hotkey_caveat() -> Option<super::HotkeyCaveat> {
        None
    }

    /// Windows stores this as `AppsUseLightTheme`: 0 means dark.
    pub fn system_prefers_dark() -> Option<bool> {
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
            .ok()?;
        let light: u32 = key.get_value("AppsUseLightTheme").ok()?;
        Some(light == 0)
    }
}

#[cfg(not(windows))]
mod imp {
    /// On Linux, ask the desktop. GNOME's `color-scheme` is the modern answer;
    /// fall back to the legacy theme name containing "dark".
    pub fn system_prefers_dark() -> Option<bool> {
        if let Some(out) = gsettings("org.gnome.desktop.interface", "color-scheme") {
            if out.contains("prefer-dark") {
                return Some(true);
            }
            if out.contains("prefer-light") || out.contains("default") {
                return Some(false);
            }
        }
        let theme = gsettings("org.gnome.desktop.interface", "gtk-theme")?;
        Some(theme.to_lowercase().contains("dark"))
    }

    /// The hotkey library has only an X11 backend, and a Wayland compositor
    /// does not deliver keystrokes to an X11 grab. Registration still
    /// succeeds, so without this the hotkeys would quietly never fire and
    /// there would be nothing on screen to explain why.
    pub fn hotkey_caveat() -> Option<super::HotkeyCaveat> {
        let wayland = std::env::var("XDG_SESSION_TYPE")
            .map(|t| t.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false)
            || std::env::var_os("WAYLAND_DISPLAY").is_some();
        if !wayland {
            return None;
        }
        Some(super::HotkeyCaveat {
            short: "Wayland session: hotkeys work only in this window. See Settings."
                .to_string(),
            detail: "This is a Wayland session. Wayland does not let an \
                     application claim system-wide keys, so the hotkeys work \
                     while this window has focus but not while another \
                     program does. Log in to an Xorg session for them to work \
                     everywhere, or use the mic button instead."
                .to_string(),
        })
    }

    fn gsettings(schema: &str, key: &str) -> Option<String> {
        let out = std::process::Command::new("gsettings")
            .args(["get", schema, key])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_wayland_caveat_is_reported_only_where_it_applies() {
        let caveat = super::hotkey_caveat();
        if cfg!(windows) {
            assert!(caveat.is_none(), "Windows registers hotkeys with the OS");
        } else {
            // On Linux the answer depends on the session this runs in; what
            // must hold is that when there is a caveat it names the problem
            // and tells the reader what to do about it.
            if let Some(caveat) = caveat {
                assert!(caveat.detail.contains("Wayland"));
                assert!(caveat.detail.contains("Xorg"));
                // The short form has to fit the status bar.
                assert!(
                    caveat.short.len() <= 80,
                    "status-bar text is {} chars: {}",
                    caveat.short.len(),
                    caveat.short
                );
            }
        }
    }
}
