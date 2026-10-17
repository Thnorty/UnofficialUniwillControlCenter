//! The pop-ups on Linux, drawn by the desktop so they look like its own volume and brightness ones. KDE Plasma shows
//! them through its pop-up service (`org.kde.osdService`). GNOME keeps its pop-ups to its own components, so the
//! package brings a small GNOME Shell extension (`packaging/linux/gnome-shell-extension`) that shows them for the app.
//! Elsewhere, or before GNOME has loaded the extension (at the next sign-in), nothing shows.

use std::sync::mpsc::{self, Sender};

use crate::color::Rgb;
use crate::hardware::system;

// The same glyphs as on Windows, so the callers stay the same; here they choose the desktop's icon.
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

/// Which desktop draws the pop-ups.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Desktop {
    Plasma,
    Gnome,
}

/// The desktop from `XDG_CURRENT_DESKTOP` ("ubuntu:GNOME", "KDE"); any other is asked as Plasma, and fails quietly.
fn desktop_from(current: &str) -> Desktop {
    if current.split(':').any(|name| name.eq_ignore_ascii_case("GNOME")) { Desktop::Gnome } else { Desktop::Plasma }
}

/// One pop-up, before it's put in the desktop's terms.
#[derive(Debug, PartialEq)]
enum Request {
    /// The keyboard brightness, in percent, with its bar.
    KeyboardBrightness(i32),
    /// A pop-up with an icon and a line of text.
    Text { glyph: char, text: String },
}

fn brightness_request(fraction: f32) -> Request {
    Request::KeyboardBrightness((fraction.clamp(0.0, 1.0) * 100.0).round() as i32)
}

fn text_request(glyph: char, caption: &str) -> Request {
    Request::Text { glyph, text: caption.to_string() }
}

/// The Breeze icon for a pop-up on Plasma.
fn plasma_icon(glyph: char) -> &'static str {
    match glyph {
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
    }
}

/// The Adwaita symbolic icon for a pop-up on GNOME, as its own pop-ups use.
fn gnome_icon(glyph: char) -> &'static str {
    match glyph {
        KEYBOARD_GLYPH => "input-keyboard-symbolic",
        TOUCHPAD_GLYPH => "input-touchpad-symbolic",
        LOCK_GLYPH => "changes-prevent-symbolic",
        UNLOCK_GLYPH => "changes-allow-symbolic",
        LIGHT_GLYPH => "display-brightness-symbolic",
        TIMER_GLYPH => "alarm-symbolic",
        OFFICE_GLYPH => "power-profile-power-saver-symbolic",
        GAMING_GLYPH => "power-profile-balanced-symbolic",
        TURBO_GLYPH => "power-profile-performance-symbolic",
        FAN_GLYPH => "weather-windy-symbolic",
        SETUP_GLYPH => "view-list-symbolic",
        _ => "dialog-information-symbolic",
    }
}

/// The extension's `Show(icon, label, level)` for a pop-up: the level is the bar's fraction, or -1 for no bar.
fn gnome_show(request: &Request) -> (&'static str, String, f64) {
    match request {
        Request::KeyboardBrightness(percent) => ("keyboard-brightness-symbolic", String::new(), *percent as f64 / 100.0),
        Request::Text { glyph, text } => (gnome_icon(*glyph), text.clone(), -1.0),
    }
}

/// Sends the pop-ups to the desktop from a thread of its own, in order, so the UI never waits for the session bus.
pub struct OsdWindow {
    requests: Option<Sender<Request>>,
}

impl OsdWindow {
    pub fn new() -> OsdWindow {
        let desktop = desktop_from(&std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default());
        let (requests, received) = mpsc::channel::<Request>();
        let started = std::thread::Builder::new().name("Pop-ups".into()).spawn(move || {
            let mut connection = None;
            for request in received {
                if connection.is_none() {
                    connection = zbus::blocking::Connection::session().ok();
                }
                let Some(bus) = &connection else { continue };
                if let Err(zbus::Error::InputOutput(_)) = send(bus, desktop, &request) {
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

/// The GNOME Shell extension's name on the session bus, its object and its interface.
pub const GNOME_EXTENSION_SERVICE: &str = "io.github.thnorty.uucc.Osd";
const GNOME_EXTENSION_PATH: &str = "/io/github/thnorty/uucc/Osd";

/// Asks the desktop for one pop-up. Without its service (or the extension, on GNOME) the call fails, and nothing shows.
fn send(bus: &zbus::blocking::Connection, desktop: Desktop, request: &Request) -> zbus::Result<()> {
    match desktop {
        Desktop::Plasma => {
            const SERVICE: &str = "org.kde.plasmashell";
            const PATH: &str = "/org/kde/osdService";
            const INTERFACE: &str = "org.kde.osdService";
            match request {
                Request::KeyboardBrightness(percent) => {
                    bus.call_method(Some(SERVICE), PATH, Some(INTERFACE), "keyboardBrightnessChanged", &(*percent,))?;
                }
                Request::Text { glyph, text } => {
                    bus.call_method(Some(SERVICE), PATH, Some(INTERFACE), "showText", &(plasma_icon(*glyph), text.as_str()))?;
                }
            }
        }
        Desktop::Gnome => {
            let (icon, label, level) = gnome_show(request);
            let service = GNOME_EXTENSION_SERVICE;
            bus.call_method(Some(service), GNOME_EXTENSION_PATH, Some(service), "Show", &(icon, label.as_str(), level))?;
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

    #[test]
    fn keyboard_brightness_uses_plasmas_own_pop_up() {
        assert_eq!(brightness_request(0.6), Request::KeyboardBrightness(60));
        assert_eq!(brightness_request(0.0), Request::KeyboardBrightness(0));
        assert_eq!(brightness_request(1.4), Request::KeyboardBrightness(100));
    }

    #[test]
    fn the_windows_key_shows_a_padlock() {
        assert_eq!(plasma_icon(LOCK_GLYPH), "lock");
        assert_eq!(plasma_icon(UNLOCK_GLYPH), "object-unlocked");
    }

    #[test]
    fn each_mode_has_its_power_profile_icon() {
        assert_eq!(plasma_icon(OFFICE_GLYPH), "battery-profile-powersave");
        assert_eq!(plasma_icon(GAMING_GLYPH), "battery-profile-balanced");
        assert_eq!(plasma_icon(TURBO_GLYPH), "battery-profile-performance");
    }

    #[test]
    fn the_other_pop_ups_have_breeze_icons() {
        assert_eq!(plasma_icon(TIMER_GLYPH), "chronometer");
        assert_eq!(plasma_icon(LIGHT_GLYPH), "brightness-high");
        assert_eq!(plasma_icon(FAN_GLYPH), "temperature-cold");
        assert_eq!(plasma_icon(SETUP_GLYPH), "view-list-icons");
        assert_eq!(plasma_icon(KEYBOARD_GLYPH), "input-keyboard");
        assert_eq!(plasma_icon(TOUCHPAD_GLYPH), "input-touchpad");
    }

    #[test]
    fn an_unknown_glyph_still_shows_its_text() {
        assert_eq!(plasma_icon('x'), "dialog-information");
        assert_eq!(gnome_icon('x'), "dialog-information-symbolic");
        assert_eq!(text_request('x', "Something"), Request::Text { glyph: 'x', text: "Something".into() });
    }

    #[test]
    fn gnome_gets_adwaita_symbolic_icons() {
        assert_eq!(gnome_icon(LOCK_GLYPH), "changes-prevent-symbolic");
        assert_eq!(gnome_icon(UNLOCK_GLYPH), "changes-allow-symbolic");
        assert_eq!(gnome_icon(OFFICE_GLYPH), "power-profile-power-saver-symbolic");
        assert_eq!(gnome_icon(GAMING_GLYPH), "power-profile-balanced-symbolic");
        assert_eq!(gnome_icon(TURBO_GLYPH), "power-profile-performance-symbolic");
        assert_eq!(gnome_icon(TIMER_GLYPH), "alarm-symbolic");
        assert_eq!(gnome_icon(LIGHT_GLYPH), "display-brightness-symbolic");
        assert_eq!(gnome_icon(FAN_GLYPH), "weather-windy-symbolic");
        assert_eq!(gnome_icon(SETUP_GLYPH), "view-list-symbolic");
        assert_eq!(gnome_icon(KEYBOARD_GLYPH), "input-keyboard-symbolic");
        assert_eq!(gnome_icon(TOUCHPAD_GLYPH), "input-touchpad-symbolic");
    }

    #[test]
    fn gnome_shows_keyboard_brightness_as_a_bar_and_text_without_one() {
        assert_eq!(gnome_show(&brightness_request(0.6)), ("keyboard-brightness-symbolic", String::new(), 0.6));
        assert_eq!(gnome_show(&text_request(TIMER_GLYPH, "Time's up")), ("alarm-symbolic", "Time's up".to_string(), -1.0));
    }

    #[test]
    fn the_desktop_comes_from_xdg_current_desktop() {
        assert_eq!(desktop_from("ubuntu:GNOME"), Desktop::Gnome);
        assert_eq!(desktop_from("GNOME"), Desktop::Gnome);
        assert_eq!(desktop_from("KDE"), Desktop::Plasma);
        assert_eq!(desktop_from(""), Desktop::Plasma); // unknown: Plasma's service is asked, and fails quietly
    }
}
