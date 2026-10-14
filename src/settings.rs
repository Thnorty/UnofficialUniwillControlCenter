//! Last-used settings, kept in %APPDATA% because the controllers can't report colors back. The file's format comes from
//! the app's earlier C# version, so its settings carry over.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::color::Rgb;
use crate::hardware::battery::ChargeMode;
use crate::hardware::fans::Table;
use crate::hardware::performance::{Mode, Tuning};
use crate::lighting::device::MAX_LIGHT;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "PascalCase", default)]
pub struct Settings {
    pub keyboard: Option<LightingSettings>,
    /// None until the app has seen the light bar, so the first run can pick up its current state.
    pub light_bar: Option<LightingSettings>,
    pub idle_timeout_seconds: u32,
    /// The light bar flashes when these change.
    pub flash_caps_lock: bool,
    pub flash_num_lock: bool,
    /// A pop-up shows when these change.
    pub popup_caps_lock: bool,
    pub popup_num_lock: bool,
    /// None until the app has read the EC, so the first run keeps the mode and battery care already set.
    pub power: Option<PowerSettings>,
    /// The startup lighting the BIOS had before this app first changed it, by device ("Keyboard", "LightBar"): its
    /// 08h and 1Ah commands in hex.
    pub startup_lighting_backup: HashMap<String, String>,
    /// Plugins by id: whether they're on, and their settings. Plugins that come with the app are on unless turned off.
    pub plugins: HashMap<String, PluginSettings>,
    /// Saved setups, in the order shown.
    pub setups: Vec<Setup>,
    /// Programs that switch to a setup while they run; the first running one wins.
    pub app_profiles: Vec<AppProfile>,
    pub app_profiles_enabled: bool,
    /// The Display page's color modes.
    pub display: DisplaySettings,
    /// The window's navigation pane shows only icons.
    pub nav_collapsed: bool,
    /// No icon in the notification area; the app still runs, and starting it again opens the window.
    pub hide_tray_icon: bool,
}

/// Whole-screen color modes: whether one is on, which, and each mode's values (missing ones are the mode's own).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct DisplaySettings {
    pub on: bool,
    pub mode: usize,
    pub modes: Vec<DisplayValues>,
}

/// One color mode's adjustments, in percent except the color temperature (kelvin).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "PascalCase", default)]
pub struct DisplayValues {
    pub saturation: i32,
    pub contrast: i32,
    pub temperature: i32,
    pub red: i32,
    pub green: i32,
    pub blue: i32,
}

impl DisplayValues {
    /// No change to the screen's colors.
    pub const NEUTRAL: DisplayValues = DisplayValues { saturation: 100, contrast: 100, temperature: 6500, red: 100, green: 100, blue: 100 };
}

impl Default for DisplayValues {
    fn default() -> Self {
        DisplayValues::NEUTRAL
    }
}

/// The lighting of both devices with the performance mode and Fan Boost, saved under a name.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct Setup {
    pub name: String,
    pub keyboard: LightingSettings,
    pub light_bar: Option<LightingSettings>,
    /// None leaves the mode and Fan Boost as they are (saved while they couldn't be set).
    pub mode: Option<Mode>,
    pub fan_boost: bool,
}

/// A program, by its file name ("game.exe"), and the setup it switches to.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct AppProfile {
    pub program: String,
    pub setup: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct PluginSettings {
    pub enabled: bool,
    pub values: serde_json::Map<String, serde_json::Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            keyboard: None,
            light_bar: None,
            idle_timeout_seconds: 0,
            flash_caps_lock: true,
            flash_num_lock: true,
            popup_caps_lock: true,
            popup_num_lock: true,
            power: None,
            startup_lighting_backup: HashMap::new(),
            plugins: HashMap::new(),
            setups: Vec::new(),
            app_profiles: Vec::new(),
            app_profiles_enabled: true,
            display: DisplaySettings::default(),
            nav_collapsed: false,
            hide_tray_icon: false,
        }
    }
}

/// The performance mode, the fans and battery care.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "PascalCase", default)]
pub struct PowerSettings {
    pub mode: Mode,
    pub fan_boost: bool,
    pub charge_mode: ChargeMode,
    /// The EC's own fan table for each mode, by mode name, read once from the EC.
    pub default_fan_tables: HashMap<String, Table>,
    /// Custom fan curves, by mode name.
    pub fan_curves: HashMap<String, FanCurve>,
    /// CPU power limits and TCC offset, by mode name.
    pub tuning: HashMap<String, Tuning>,
    /// USB charging while the laptop is off; None leaves it as it is.
    pub usb_power: Option<bool>,
    /// Each mode's profiles, by mode name, "Standard" first. The active one's values are also in `tuning` and
    /// `fan_curves`.
    pub profiles: HashMap<String, Vec<ModeProfile>>,
    /// The active profile of each mode, by mode name.
    pub active_profiles: HashMap<String, usize>,
}

/// One of a mode's profiles (Control Center keeps five per mode): its CPU tuning and fan curve, under a name.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct ModeProfile {
    pub name: String,
    pub tuning: Tuning,
    pub fan_curve: Option<FanCurve>,
}

impl Default for PowerSettings {
    fn default() -> Self {
        PowerSettings {
            mode: Mode::Gaming,
            fan_boost: false,
            charge_mode: ChargeMode::HighCapacity,
            default_fan_tables: HashMap::new(),
            fan_curves: HashMap::new(),
            tuning: HashMap::new(),
            usb_power: None,
            profiles: HashMap::new(),
            active_profiles: HashMap::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct FanCurve {
    pub enabled: bool,
    pub table: Table,
}

/// The lighting of one device: the keyboard or the light bar.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "PascalCase", default)]
pub struct LightingSettings {
    pub on: bool,
    pub effect: String,
    /// Static and Music colors of the keyboard's four zones (ARGB).
    pub zone_colors: Vec<i32>,
    /// Colors of each palette effect, by effect key.
    pub palettes: HashMap<String, Vec<i32>>,
    pub wave_direction: u8,
    /// Firmware brightness, 0-50 (all that earlier versions saved).
    pub light: u8,
    /// The brightness in percent, which can fall between two firmware levels. Ignored if `light` doesn't match it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brightness_percent: Option<u8>,
    pub speed: u8,
    /// The light bar shows the keyboard's effect, colors and speed.
    pub linked: bool,
}

impl Default for LightingSettings {
    fn default() -> Self {
        let red = Rgb::new(255, 0, 0).to_argb();
        LightingSettings {
            on: true,
            effect: "static".into(),
            zone_colors: vec![red; 4],
            palettes: HashMap::new(),
            wave_direction: 1,
            light: MAX_LIGHT,
            brightness_percent: None,
            speed: 2,
            linked: false,
        }
    }
}

fn folder(name: &str) -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join(name))
}

fn path() -> Option<PathBuf> {
    folder("TulparControl").map(|f| f.join("settings.json"))
}

/// Where the app kept its settings when it was called Keyboard RGB.
fn old_path() -> Option<PathBuf> {
    folder("MonsterKeyboardRGB").map(|f| f.join("settings.json"))
}

impl Settings {
    pub fn load() -> Settings {
        let text = path().and_then(|p| std::fs::read_to_string(p).ok()).or_else(|| old_path().and_then(|p| std::fs::read_to_string(p).ok()));
        let mut settings: Settings = text
            .and_then(|text| serde_json::from_str(text.trim_start_matches('\u{FEFF}')).ok())
            .unwrap_or_default();
        settings.keyboard.get_or_insert_with(LightingSettings::default);
        settings
    }

    /// The settings as a backup file holds them.
    pub fn to_backup(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Settings from a backup file; refuses JSON that isn't this app's settings.
    pub fn from_backup(text: &str) -> anyhow::Result<Settings> {
        let value: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{FEFF}')).map_err(|e| anyhow::anyhow!("The file isn't valid JSON ({e})"))?;
        let known = ["Keyboard", "LightBar", "Power", "Plugins", "IdleTimeoutSeconds"];
        if !value.as_object().is_some_and(|o| known.iter().any(|k| o.contains_key(*k))) {
            anyhow::bail!("The file isn't a Tulpar Control settings backup");
        }
        let mut settings: Settings = serde_json::from_value(value).map_err(|e| anyhow::anyhow!("The settings in the file can't be read ({e})"))?;
        settings.keyboard.get_or_insert_with(LightingSettings::default);
        Ok(settings)
    }

    pub fn save(&self) {
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string(self) {
            let _ = std::fs::write(path, text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backups() {
        let mut settings = Settings::default();
        settings.idle_timeout_seconds = 300;
        settings.popup_num_lock = false;
        let restored = Settings::from_backup(&settings.to_backup().unwrap()).unwrap();
        assert_eq!(restored.idle_timeout_seconds, 300);
        assert!(!restored.popup_num_lock);
        assert!(restored.keyboard.is_some());
        assert!(Settings::from_backup(r#"{ "name": "something else" }"#).is_err());
        assert!(Settings::from_backup("not json").is_err());
    }
}
