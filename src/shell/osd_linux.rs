//! The pop-ups on Linux. They come with the Fn keys; until then nothing shows.

use crate::color::Rgb;
use crate::hardware::system;

pub const KEYBOARD_GLYPH: char = '\u{E765}';
pub const TIMER_GLYPH: char = '\u{E916}';
pub const OFFICE_GLYPH: char = '\u{E821}';
pub const GAMING_GLYPH: char = '\u{E7FC}';
pub const TURBO_GLYPH: char = '\u{E945}';
pub const FAN_GLYPH: char = '\u{E9CA}';
pub const SETUP_GLYPH: char = '\u{E8F1}';

pub struct OsdWindow;

impl OsdWindow {
    pub fn new() -> OsdWindow {
        OsdWindow
    }

    pub fn show_brightness(&self, _fraction: f32) {}

    pub fn show_light_bar(&self, _on: bool) {}

    pub fn show_touchpad(&self, _enabled: bool) {}

    pub fn show_win_key(&self, _locked: bool) {}

    pub fn show_text(&self, _glyph: char, _caption: &str) {}
}

/// The accent fill for sliders: a lighter shade of the accent in dark mode, a darker one in light mode.
pub fn accent_fill(dark: bool) -> Rgb {
    let accent = system::accent_color().unwrap_or(Rgb::new(0, 120, 212));
    if dark { accent.lerp(Rgb::WHITE, 0.45) } else { accent.lerp(Rgb::BLACK, 0.15) }
}
