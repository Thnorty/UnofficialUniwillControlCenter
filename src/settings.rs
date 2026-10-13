//! Last-used settings, kept in %APPDATA% because the controllers can't report colors back. The file's format comes from
//! the app's earlier C# version, so its settings carry over.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::color::Rgb;
use crate::hardware::battery::ChargeMode;
use crate::hardware::fans::Table;
use crate::hardware::performance::Mode;
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
    /// None until the app has read the EC, so the first run keeps the mode and battery care already set.
    pub power: Option<PowerSettings>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { keyboard: None, light_bar: None, idle_timeout_seconds: 0, flash_caps_lock: true, flash_num_lock: true, power: None }
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
}

impl Default for PowerSettings {
    fn default() -> Self {
        PowerSettings {
            mode: Mode::Gaming,
            fan_boost: false,
            charge_mode: ChargeMode::HighCapacity,
            default_fan_tables: HashMap::new(),
            fan_curves: HashMap::new(),
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
