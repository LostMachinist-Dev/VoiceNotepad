//! Making egui look like Notepad, in light and dark.
//!
//! The colours are taken from Windows 11's Notepad: a near-white chrome with a
//! pure white page in light mode, and #202020 chrome over a #272727 page in
//! dark. The accent is the Windows selection blue.

use egui::{Color32, CornerRadius, Stroke, Visuals};

use crate::config::ThemePref;
use crate::platform;

/// The theme actually in use, after resolving "follow system".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

impl Appearance {
    pub fn flipped(self) -> Self {
        match self {
            Appearance::Light => Appearance::Dark,
            Appearance::Dark => Appearance::Light,
        }
    }
}

/// Work out which theme to show. "Follow system" falls back to light when the
/// system cannot be asked.
pub fn resolve(pref: ThemePref) -> Appearance {
    match pref {
        ThemePref::Light => Appearance::Light,
        ThemePref::Dark => Appearance::Dark,
        ThemePref::System => match platform::system_prefers_dark() {
            Some(true) => Appearance::Dark,
            Some(false) => Appearance::Light,
            None => Appearance::Light,
        },
    }
}

/// The handful of colours the UI needs by name.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Menu bar and status bar background.
    pub chrome: Color32,
    /// The text area background -- the "page".
    pub page: Color32,
    /// Body text.
    pub ink: Color32,
    /// Dimmer text, for the status bar.
    pub faint_ink: Color32,
    /// Hairlines between the chrome and the page.
    pub divider: Color32,
    /// Selection fill.
    pub accent: Color32,
    /// The mic button when listening, and the status bar dot.
    pub recording: Color32,
    /// The mic button when paused.
    pub idle_mic: Color32,
}

pub fn palette(appearance: Appearance) -> Palette {
    match appearance {
        Appearance::Light => Palette {
            chrome: Color32::from_rgb(0xF3, 0xF3, 0xF3),
            page: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            ink: Color32::from_rgb(0x1A, 0x1A, 0x1A),
            faint_ink: Color32::from_rgb(0x5A, 0x5A, 0x5A),
            divider: Color32::from_rgb(0xD7, 0xD7, 0xD7),
            accent: Color32::from_rgb(0x00, 0x78, 0xD4),
            recording: Color32::from_rgb(0xD1, 0x34, 0x3B),
            idle_mic: Color32::from_rgb(0x60, 0x60, 0x60),
        },
        Appearance::Dark => Palette {
            chrome: Color32::from_rgb(0x20, 0x20, 0x20),
            page: Color32::from_rgb(0x27, 0x27, 0x27),
            ink: Color32::from_rgb(0xF2, 0xF2, 0xF2),
            faint_ink: Color32::from_rgb(0xA8, 0xA8, 0xA8),
            divider: Color32::from_rgb(0x38, 0x38, 0x38),
            accent: Color32::from_rgb(0x4C, 0xA0, 0xE0),
            recording: Color32::from_rgb(0xE8, 0x4A, 0x4A),
            idle_mic: Color32::from_rgb(0xB0, 0xB0, 0xB0),
        },
    }
}

/// Apply the theme to the whole context.
pub fn apply(ctx: &egui::Context, appearance: Appearance) {
    let p = palette(appearance);

    let mut visuals = match appearance {
        Appearance::Light => Visuals::light(),
        Appearance::Dark => Visuals::dark(),
    };

    visuals.panel_fill = p.chrome;
    visuals.window_fill = p.chrome;
    visuals.extreme_bg_color = p.page; // TextEdit background
    visuals.faint_bg_color = p.chrome;
    visuals.override_text_color = Some(p.ink);
    visuals.selection.bg_fill = p.accent.linear_multiply(0.45);
    visuals.selection.stroke = Stroke::new(1.0, p.ink);
    visuals.window_stroke = Stroke::new(1.0, p.divider);

    // Notepad's chrome is flat: square corners, no drop shadows on the bar,
    // and buttons that only show themselves on hover.
    let radius = CornerRadius::same(2);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = radius;
    }
    visuals.widgets.noninteractive.bg_fill = p.chrome;
    visuals.widgets.noninteractive.weak_bg_fill = p.chrome;
    visuals.widgets.inactive.bg_fill = p.chrome;
    visuals.widgets.inactive.weak_bg_fill = p.chrome;
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.hovered.bg_fill = hover_tint(appearance);
    visuals.widgets.hovered.weak_bg_fill = hover_tint(appearance);
    visuals.widgets.hovered.bg_stroke = Stroke::NONE;
    visuals.widgets.active.bg_fill = press_tint(appearance);
    visuals.widgets.active.weak_bg_fill = press_tint(appearance);

    ctx.set_visuals(visuals);

    ctx.all_styles_mut(|style| {
        style.spacing.button_padding = egui::vec2(8.0, 3.0);
        style.spacing.menu_margin = egui::Margin::symmetric(0, 4);
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        // The chrome stays a fixed size regardless of the document's zoom,
        // which is how Notepad behaves.
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(13.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(13.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(11.5));
    });
}

fn hover_tint(appearance: Appearance) -> Color32 {
    match appearance {
        Appearance::Light => Color32::from_rgb(0xE5, 0xE5, 0xE5),
        Appearance::Dark => Color32::from_rgb(0x2F, 0x2F, 0x2F),
    }
}

fn press_tint(appearance: Appearance) -> Color32 {
    match appearance {
        Appearance::Light => Color32::from_rgb(0xD6, 0xD6, 0xD6),
        Appearance::Dark => Color32::from_rgb(0x3A, 0x3A, 0x3A),
    }
}

/// The document font size for a zoom factor. Notepad's zoom scales the page,
/// not the menus.
pub fn editor_font_size(zoom: f32) -> f32 {
    (14.0 * zoom).clamp(7.0, 42.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_preferences_ignore_the_system() {
        assert_eq!(resolve(ThemePref::Light), Appearance::Light);
        assert_eq!(resolve(ThemePref::Dark), Appearance::Dark);
    }

    #[test]
    fn the_toggle_flips_between_the_two() {
        assert_eq!(Appearance::Light.flipped(), Appearance::Dark);
        assert_eq!(Appearance::Dark.flipped(), Appearance::Light);
    }

    #[test]
    fn the_page_is_lighter_than_the_chrome_in_light_mode() {
        let p = palette(Appearance::Light);
        assert!(p.page.r() > p.chrome.r());
        // and darker in dark mode's case, the page is slightly lifted
        let d = palette(Appearance::Dark);
        assert!(d.page.r() > d.chrome.r());
    }

    #[test]
    fn zoom_scales_the_document_font_within_sane_bounds() {
        assert_eq!(editor_font_size(1.0), 14.0);
        assert_eq!(editor_font_size(2.0), 28.0);
        assert_eq!(editor_font_size(0.01), 7.0);
        assert_eq!(editor_font_size(100.0), 42.0);
    }
}
