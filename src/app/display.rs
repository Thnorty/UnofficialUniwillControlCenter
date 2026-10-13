//! The Display page: whole-screen color modes (Vibrant, Cinema, Low blue light, Custom), each with its own
//! saturation, contrast, color temperature and channel levels, as Control Center's Display page has them.

use super::App;
use crate::settings::DisplayValues;
use crate::shell::display;

/// The modes and their starting values. Vibrant is Control Center's 1.5× saturation.
const MODES: [(&str, DisplayValues); 4] = [
    ("Vibrant", DisplayValues { saturation: 150, ..DisplayValues::NEUTRAL }),
    ("Cinema", DisplayValues { saturation: 110, contrast: 110, temperature: 6000, ..DisplayValues::NEUTRAL }),
    ("Low blue light", DisplayValues { temperature: 4500, ..DisplayValues::NEUTRAL }),
    ("Custom", DisplayValues::NEUTRAL),
];

impl App {
    fn display_mode(&self) -> usize {
        self.settings.display.mode.min(MODES.len() - 1)
    }

    /// The current mode's values, its starting ones until changed.
    fn display_values(&self) -> DisplayValues {
        let mode = self.display_mode();
        self.settings.display.modes.get(mode).copied().unwrap_or(MODES[mode].1)
    }

    fn set_display_values(&mut self, values: DisplayValues) {
        let mode = self.display_mode();
        let modes = &mut self.settings.display.modes;
        while modes.len() < MODES.len() {
            modes.push(MODES[modes.len()].1);
        }
        modes[mode] = values;
        self.settings_dirty = true;
    }

    /// Puts the current adjustments on the screen, or takes them off. Demo mode leaves the screen alone.
    pub(super) fn apply_display(&mut self) {
        if self.demo {
            return;
        }
        let matrix = if self.settings.display.on { display::matrix(&self.display_values()) } else { display::IDENTITY };
        if let Err(e) = display::apply(&matrix) {
            self.set_status(&format!("Couldn't change the display's colors: {e:#}"), true, false);
        }
    }

    pub(super) fn refresh_display(&self) {
        let Some(ui) = self.ui() else { return };
        let v = self.display_values();
        ui.set_display_demo(self.demo);
        ui.set_display_on(self.settings.display.on);
        ui.set_display_mode(self.display_mode() as i32);
        ui.set_display_saturation(v.saturation as f32);
        ui.set_display_contrast(v.contrast as f32);
        ui.set_display_temperature(v.temperature as f32);
        ui.set_display_red(v.red as f32);
        ui.set_display_green(v.green as f32);
        ui.set_display_blue(v.blue as f32);
        ui.set_display_changed(v != MODES[self.display_mode()].1);
    }

    pub(super) fn set_display_on(&mut self, on: bool) {
        self.settings.display.on = on;
        self.settings_dirty = true;
        self.apply_display();
        self.refresh_display();
    }

    pub(super) fn set_display_mode(&mut self, mode: usize) {
        self.settings.display.mode = mode.min(MODES.len() - 1);
        self.settings.display.on = true; // picking a mode means using it
        self.settings_dirty = true;
        self.apply_display();
        self.refresh_display();
    }

    /// One slider: 0 saturation, 1 contrast, 2 color temperature, 3 red, 4 green, 5 blue.
    pub(super) fn set_display_value(&mut self, which: i32, value: f32) {
        let mut v = self.display_values();
        let value = value.round() as i32;
        match which {
            0 => v.saturation = value.clamp(0, 200),
            1 => v.contrast = value.clamp(50, 150),
            2 => v.temperature = value.clamp(3000, 7500),
            3 => v.red = value.clamp(50, 100),
            4 => v.green = value.clamp(50, 100),
            5 => v.blue = value.clamp(50, 100),
            _ => return,
        }
        self.set_display_values(v);
        self.settings.display.on = true; // adjusting a mode means seeing it
        self.apply_display();
        self.refresh_display();
    }

    pub(super) fn reset_display_mode(&mut self) {
        self.set_display_values(MODES[self.display_mode()].1);
        self.apply_display();
        self.refresh_display();
    }
}
