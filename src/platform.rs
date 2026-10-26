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

#[cfg(windows)]
mod imp {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

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
