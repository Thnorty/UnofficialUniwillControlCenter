//! The pop-ups on Linux: KDE Plasma shows them, through its pop-up service (`org.kde.osdService`), so they look like
//! its own volume and brightness ones. Other desktops have no such service, and nothing shows there.

use std::sync::mpsc::{self, Sender};

use crate::color::Rgb;
use crate::hardware::system;

// The same glyphs as on Windows, so the callers stay the same; here they choose a Breeze icon.
pub const KEYBOARD_GLYPH: char = '\u{E765}';
pub const TOUCHPAD_GLYPH: char = '\u{EFA5}';
pub const LOCK_GLYPH: char = '\u{E72E}';
pub const UNLOCK_GLYPH: char = '\u{E785}';
pub const LIGHT_GLYPH: char = '\u{EA80}';
pub const TIMER_GLYPH: char = '\u{E916}';
pub const OFFICE_GLYPH: char = '\u{E821}';
pub const GAMING_GLYPH: char = '\u{E7FC}';
pub const TURBO_GLYPH: char = '\u{E945}';
pub const FAN_GLYPH: char = '\u{E9CA}';
pub const SETUP_GLYPH: char = '\u{E8F1}';

/// One pop-up, as Plasma's pop-up service is asked for it.
#[derive(Debug, PartialEq)]
enum Request {
    /// Plasma's own keyboard brightness pop-up, with its bar.
    KeyboardBrightness(i32),
    /// A pop-up with an icon from the icon theme and a line of text.
    Text { icon: &'static str, text: String },
}

fn brightness_request(fraction: f32) -> Request {
    Request::KeyboardBrightness((fraction.clamp(0.0, 1.0) * 100.0).round() as i32)
}

fn text_request(glyph: char, caption: &str) -> Request {
    let icon = match glyph {
        KEYBOARD_GLYPH => "input-keyboard",
        TOUCHPAD_GLYPH => "input-touchpad",
        LOCK_GLYPH => "lock",
        UNLOCK_GLYPH => "object-unlocked",
        LIGHT_GLYPH => "brightness-high",
        TIMER_GLYPH => "chronometer",
        OFFICE_GLYPH => "battery-profile-powersave",
        GAMING_GLYPH => "battery-profile-balanced",
        TURBO_GLYPH => "battery-profile-performance",
        FAN_GLYPH => "temperature-cold",
        SETUP_GLYPH => "view-list-icons",
        _ => "dialog-information",
    };
    Request::Text { icon, text: caption.to_string() }
}

/// Sends the pop-ups to Plasma from a thread of its own, in order, so the UI never waits for the session bus.
pub struct OsdWindow {
    requests: Option<Sender<Request>>,
}

impl OsdWindow {
    pub fn new() -> OsdWindow {
        let (requests, received) = mpsc::channel::<Request>();
        let started = std::thread::Builder::new().name("Pop-ups".into()).spawn(move || {
            let mut connection = None;
            for request in received {
                if connection.is_none() {
                    connection = zbus::blocking::Connection::session().ok();
                }
                let Some(bus) = &connection else { continue };
                if let Err(zbus::Error::InputOutput(_)) = send(bus, &request) {
                    connection = None; // the session bus went away: connect again for the next one
                }
            }
        });
        OsdWindow { requests: started.ok().map(|_| requests) }
    }

    fn request(&self, request: Request) {
        if let Some(requests) = &self.requests {
            let _ = requests.send(request);
        }
    }

    pub fn show_brightness(&self, fraction: f32) {
        self.request(brightness_request(fraction));
    }

    pub fn show_light_bar(&self, on: bool) {
        self.show_text(LIGHT_GLYPH, if on { "Light bar on" } else { "Light bar off" });
    }

    pub fn show_touchpad(&self, enabled: bool) {
        self.show_text(TOUCHPAD_GLYPH, if enabled { "Touchpad on" } else { "Touchpad off" });
    }

    pub fn show_win_key(&self, locked: bool) {
        if locked {
            self.show_text(LOCK_GLYPH, "Windows key locked")
        } else {
            self.show_text(UNLOCK_GLYPH, "Windows key unlocked")
        }
    }

    pub fn show_text(&self, glyph: char, caption: &str) {
        self.request(text_request(glyph, caption));
    }
}

/// Asks Plasma for one pop-up. Without Plasma the call fails, and nothing shows.
fn send(bus: &zbus::blocking::Connection, request: &Request) -> zbus::Result<()> {
    const SERVICE: &str = "org.kde.plasmashell";
    const PATH: &str = "/org/kde/osdService";
    const INTERFACE: &str = "org.kde.osdService";
    match request {
        Request::KeyboardBrightness(percent) => {
            bus.call_method(Some(SERVICE), PATH, Some(INTERFACE), "keyboardBrightnessChanged", &(*percent,))?;
        }
        Request::Text { icon, text } => {
            bus.call_method(Some(SERVICE), PATH, Some(INTERFACE), "showText", &(*icon, text.as_str()))?;
        }
    }
    Ok(())
}

/// The accent fill for sliders: a lighter shade of the accent in dark mode, a darker one in light mode.
pub fn accent_fill(dark: bool) -> Rgb {
    let accent = system::accent_color().unwrap_or(Rgb::new(0, 120, 212));
    if dark { accent.lerp(Rgb::WHITE, 0.45) } else { accent.lerp(Rgb::BLACK, 0.15) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(icon: &'static str, text: &str) -> Request {
        Request::Text { icon, text: text.to_string() }
    }

    #[test]
    fn keyboard_brightness_uses_plasmas_own_pop_up() {
        assert_eq!(brightness_request(0.6), Request::KeyboardBrightness(60));
        assert_eq!(brightness_request(0.0), Request::KeyboardBrightness(0));
        assert_eq!(brightness_request(1.4), Request::KeyboardBrightness(100));
    }

    #[test]
    fn the_windows_key_shows_a_padlock() {
        assert_eq!(text_request(LOCK_GLYPH, "Windows key locked"), text("lock", "Windows key locked"));
        assert_eq!(text_request(UNLOCK_GLYPH, "Windows key unlocked"), text("object-unlocked", "Windows key unlocked"));
    }

    #[test]
    fn each_mode_has_its_power_profile_icon() {
        assert_eq!(text_request(OFFICE_GLYPH, "Office mode"), text("battery-profile-powersave", "Office mode"));
        assert_eq!(text_request(GAMING_GLYPH, "Gaming mode"), text("battery-profile-balanced", "Gaming mode"));
        assert_eq!(text_request(TURBO_GLYPH, "Turbo mode"), text("battery-profile-performance", "Turbo mode"));
    }

    #[test]
    fn the_other_pop_ups_have_breeze_icons() {
        assert_eq!(text_request(TIMER_GLYPH, "Time's up"), text("chronometer", "Time's up"));
        assert_eq!(text_request(LIGHT_GLYPH, "Light bar on"), text("brightness-high", "Light bar on"));
        assert_eq!(text_request(FAN_GLYPH, "Fan Boost on"), text("temperature-cold", "Fan Boost on"));
        assert_eq!(text_request(SETUP_GLYPH, "Back to your setup"), text("view-list-icons", "Back to your setup"));
        assert_eq!(text_request(KEYBOARD_GLYPH, "Caps Lock on"), text("input-keyboard", "Caps Lock on"));
        assert_eq!(text_request(TOUCHPAD_GLYPH, "Touchpad off"), text("input-touchpad", "Touchpad off"));
    }

    #[test]
    fn an_unknown_glyph_still_shows_its_text() {
        assert_eq!(text_request('x', "Something"), text("dialog-information", "Something"));
    }
}
