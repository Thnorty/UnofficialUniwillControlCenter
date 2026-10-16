//! What this app keeps in the BIOS's settings variable (Control Center's `UniWillVariable`): the lighting the
//! keyboard and the light bar show while the laptop starts, the CPU core voltage offset and the GPU mode. Writes run
//! off the UI thread; results come back with `post`.
//!
//! Control Center's service rewrites the whole variable from its own copy, so nothing is written while it runs.

use super::{App, Kind, boots_as_is, post};
use crate::hardware::system;
use crate::hardware::uefi::{self, GpuMode, Startup, Variable, VoltageOffset};
use crate::lighting::effects;
use crate::ui::TuningView;

/// What the BIOS had for each device's startup lighting on this laptop before anything changed it: Mix on the
/// keyboard and Thinking on the light bar, each for the BIOS's 4-second timeout.
const FACTORY_KEYBOARD: ([u8; 8], [u8; 8]) = ([0x08, 0x02, 0x13, 0x05, 0x32, 0x08, 0x00, 0x01], [0x1A, 0x00, 0x01, 0x04, 0, 0, 0, 0]);
const FACTORY_LIGHT_BAR: ([u8; 8], [u8; 8]) = ([0x08, 0x02, 0x21, 0x05, 0x24, 0x00, 0x00, 0x01], [0x1A, 0x00, 0x01, 0x04, 0, 0, 0, 0]);

#[derive(Default)]
pub(super) struct Bios {
    variable: Option<Variable>,
    error: Option<String>,
    pub busy: bool,
    /// The voltage offset chosen on the page but not yet saved, mV.
    voltage: Option<i32>,
    /// Saved since the app started, so the laptop needs a restart for it.
    voltage_saved: bool,
    /// The GPU mode the laptop started in, as first read.
    pub gpu_mode_started: Option<GpuMode>,
}

fn startup_of(kind: Kind) -> Startup {
    match kind {
        Kind::Keyboard => Startup::Keyboard,
        Kind::LightBar => Startup::LightBar,
    }
}

fn backup_key(kind: Kind) -> &'static str {
    match kind {
        Kind::Keyboard => "Keyboard",
        Kind::LightBar => "LightBar",
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

fn from_hex(text: &str) -> Option<([u8; 8], [u8; 8])> {
    let bytes: Vec<u8> = (0..text.len() / 2).map(|i| u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()).collect::<Option<_>>()?;
    Some((bytes.get(..8)?.try_into().ok()?, bytes.get(8..16)?.try_into().ok()?))
}

fn error_text(e: anyhow::Error) -> String {
    format!("{e:#}")
}

impl App {
    /// Reads the variable once at start, off the UI thread.
    pub(super) fn read_bios(&mut self) {
        if self.demo {
            return;
        }
        self.bios.busy = true;
        let _ = std::thread::Builder::new().name("BIOS read".into()).spawn(|| {
            let result = uefi::read().map_err(error_text);
            post(move |app| app.bios_read(result));
        });
    }

    fn bios_read(&mut self, result: Result<Variable, String>) {
        self.bios.busy = false;
        match result {
            Ok(variable) => {
                let recovered = variable.recovered();
                self.bios.gpu_mode_started = variable.gpu_mode();
                self.bios.variable = Some(variable);
                self.bios.error = None;
                if recovered {
                    self.recover_from_failed_start();
                }
            }
            Err(reason) => self.bios.error = Some(reason),
        }
        self.refresh_bios();
    }

    /// The BIOS started without the tuning because the laptop failed to start with it. As Control Center's
    /// DetectRecoveryFlagFromBiosVariable: every mode back to its defaults, no voltage offset, and the flag cleared.
    fn recover_from_failed_start(&mut self) {
        if system::control_center_running() {
            return; // Control Center handles it
        }
        self.power.reset_all_tuning();
        self.settings_dirty = true;
        self.apply_power();
        self.write_bios(
            |variable| {
                if variable.voltage_offset().is_some_and(|v| v.value != 0) {
                    uefi::set_voltage_offset(0)?;
                }
                uefi::clear_recovery()
            },
            |app| {
                if let Some(tray) = &app.tray {
                    tray.notify(
                        "CPU tuning reset",
                        "The laptop didn't start properly with the CPU tuning, so the BIOS started without it. The power limits, TCC offset and voltage offset are back to their defaults.",
                    );
                }
            },
        );
    }

    /// Runs `write` off the UI thread with the current variable, then `done` on success.
    fn write_bios(&mut self, write: impl FnOnce(Variable) -> anyhow::Result<Variable> + Send + 'static, done: impl FnOnce(&mut App) + Send + 'static) {
        let Some(variable) = self.bios.variable.clone() else { return };
        self.bios.busy = true;
        self.refresh_bios();
        let cli = self.cli.tag();
        let _ = std::thread::Builder::new().name("BIOS write".into()).spawn(move || {
            let result = write(variable).map_err(error_text);
            post(move |app| {
                app.bios.busy = false;
                match result {
                    Ok(variable) => {
                        app.bios.variable = Some(variable);
                        app.bios.error = None;
                        done(app);
                    }
                    Err(reason) => {
                        let text = format!("Couldn't save to the BIOS: {reason}");
                        app.cli.reported(cli, &text);
                        app.set_status(&text, true, false);
                    }
                }
                app.refresh_bios();
            });
        });
    }

    pub(super) fn bios_writable(&self) -> Result<(), String> {
        if self.demo {
            return Err("Demo mode: nothing is saved to the BIOS.".into());
        }
        if system::control_center_running() {
            return Err("Control Center is running and would overwrite this. To save it here, close Control Center and stop its service (GCUService).".into());
        }
        if let Some(error) = &self.bios.error {
            return Err(format!("The BIOS settings can't be read: {error}"));
        }
        if self.bios.variable.is_none() {
            return Err("Reading the BIOS settings…".into());
        }
        Ok(())
    }

    // ---- Startup lighting -----------------------------------------------------------------------------------------

    /// Makes what `kind` shows now its lighting while the laptop starts. Effects drawn by this app can't run then,
    /// so they leave what stays once the app exits: the keyboard's zone colors, or the light bar's first color.
    pub(super) fn save_startup(&mut self, kind: Kind) {
        if let Err(reason) = self.bios_writable() {
            self.set_status(&reason, false, false);
            return;
        }
        let plan = self.plan(kind);
        let Some(lighting) = self.lighting(kind) else { return };
        let Some(device) = lighting.device.clone() else { return };
        if !lighting.on {
            return;
        }
        let (effect, colors) = if !boots_as_is(kind, plan.effect) {
            match kind {
                Kind::Keyboard => (&effects::KEYBOARD[0], lighting.zones.to_vec()),
                Kind::LightBar => (&effects::LIGHT_BAR[0], vec![plan.colors.first().copied().unwrap_or(effects::LIGHT_BAR[0].default_colors[0])]),
            }
        } else {
            (plan.effect, plan.colors.clone())
        };
        let light = super::to_level(lighting.percent);
        let commands = device.set_startup(effect, &colors, plan.direction, light, plan.speed);
        self.apply(kind, false, None); // the running effect again
        let (effect_command, timeout_command) = match commands {
            Ok(commands) => commands,
            Err(e) => {
                self.device_failed(kind, e);
                return;
            }
        };
        // The first time, keep what the BIOS had.
        let startup = startup_of(kind);
        if let Some(variable) = &self.bios.variable {
            let (old_effect, old_timeout) = variable.startup(startup);
            self.settings.startup_lighting_backup.entry(backup_key(kind).into()).or_insert_with(|| format!("{}{}", hex(&old_effect), hex(&old_timeout)));
            self.settings_dirty = true;
        }
        let name = effect.name;
        self.write_bios(
            move |_| uefi::set_startup(startup, effect_command, timeout_command),
            move |app| app.set_status(&format!("Saved to the BIOS: {name} while the laptop starts."), false, false),
        );
    }

    /// Puts back the startup lighting the BIOS had before this app changed it.
    pub(super) fn reset_startup(&mut self, kind: Kind) {
        if let Err(reason) = self.bios_writable() {
            self.set_status(&reason, false, false);
            return;
        }
        let (effect, timeout) = self
            .settings
            .startup_lighting_backup
            .get(backup_key(kind))
            .and_then(|text| from_hex(text))
            .unwrap_or(if kind == Kind::Keyboard { FACTORY_KEYBOARD } else { FACTORY_LIGHT_BAR });
        let startup = startup_of(kind);
        self.write_bios(
            move |_| uefi::set_startup(startup, effect, timeout),
            |app| app.set_status("The startup lighting the laptop came with is back.", false, false),
        );
    }

    /// What a device shows while the laptop starts, for its lighting page.
    fn startup_text(&self, kind: Kind) -> String {
        let Some(variable) = &self.bios.variable else {
            return match &self.bios.error {
                _ if self.demo => "While the laptop starts, it shows the startup lighting it came with.".into(),
                Some(error) => format!("The BIOS settings can't be read: {error}"),
                None => "Reading the BIOS settings…".into(),
            };
        };
        let (effect, _) = variable.startup(startup_of(kind));
        let factory = if kind == Kind::Keyboard { FACTORY_KEYBOARD.0 } else { FACTORY_LIGHT_BAR.0 };
        let list = if kind == Kind::Keyboard { effects::KEYBOARD } else { effects::LIGHT_BAR };
        let name = list.iter().find(|e| e.id == effect[2] && !e.drawn_by_app()).map_or_else(|| format!("effect {}", effect[2]), |e| e.name.to_string());
        if effect[0] != 0x08 {
            "While the laptop starts, it shows the startup lighting it came with.".into()
        } else if effect == factory {
            format!("While the laptop starts: {name}, the startup lighting it came with.")
        } else {
            format!("While the laptop starts: {name} at {}% brightness.", super::to_percent(effect[4]))
        }
    }

    // ---- Voltage offset -------------------------------------------------------------------------------------------

    fn voltage_range(&self) -> Option<VoltageOffset> {
        if self.demo {
            return Some(VoltageOffset { value: 0, minimum: 0, maximum: 100 });
        }
        self.bios.variable.as_ref()?.voltage_offset()
    }

    pub(super) fn set_voltage(&mut self, mv: i32) {
        let Some(range) = self.voltage_range() else { return };
        self.bios.voltage = Some(mv.clamp(range.minimum, range.maximum));
        self.refresh_bios();
    }

    pub(super) fn save_voltage(&mut self) {
        let Some(mv) = self.bios.voltage else { return };
        if let Err(reason) = self.bios_writable() {
            self.set_status(&reason, false, false);
            return;
        }
        self.write_bios(
            move |_| uefi::set_voltage_offset(mv),
            |app| {
                app.bios.voltage = None;
                app.bios.voltage_saved = true;
            },
        );
    }

    // ---- GPU mode ---------------------------------------------------------------------------------------------------

    pub(super) fn gpu_mode(&self) -> Option<GpuMode> {
        if self.demo {
            return Some(GpuMode::Hybrid);
        }
        self.bios.variable.as_ref()?.gpu_mode()
    }

    pub(super) fn set_gpu_mode(&mut self, index: usize) {
        let Some(mode) = GpuMode::ALL.get(index).copied() else { return };
        if self.gpu_mode() == Some(mode) {
            return;
        }
        if let Err(reason) = self.bios_writable() {
            self.set_status(&reason, false, false);
            self.refresh_bios();
            return;
        }
        self.write_bios(move |_| uefi::set_gpu_mode(mode), |_| {});
    }

    fn gpu_mode_note(&self, mode: Option<GpuMode>) -> String {
        let Some(mode) = mode else {
            return match &self.bios.error {
                Some(error) => format!("The BIOS settings can't be read: {error}"),
                None if self.bios.variable.is_some() => "This laptop's BIOS doesn't have a GPU mode this app knows.".into(),
                None => "Reading the BIOS settings…".into(),
            };
        };
        let about = match mode {
            GpuMode::Hybrid => {
                "The integrated graphics drive the screen, and the NVIDIA GPU works for the apps that need it and switches off when nothing does. The best of both, as the laptop comes."
            }
            GpuMode::Integrated => {
                "The NVIDIA GPU is switched off completely, for the longest battery life. Games run on the integrated graphics, and screens on the ports the NVIDIA GPU drives stay dark."
            }
            GpuMode::Discrete => {
                "The NVIDIA GPU drives the screen, for the most performance in games. It's on all the time, so the battery runs down faster."
            }
        };
        about.into()
    }

    /// What the GPU mode waits for, when it isn't the one the laptop started in; empty otherwise.
    fn gpu_mode_restart(&self, mode: Option<GpuMode>) -> String {
        let started = self.bios.gpu_mode_started.or(self.demo.then_some(GpuMode::Hybrid));
        match mode {
            Some(mode) if started.is_some_and(|s| s != mode) => {
                let name = match mode {
                    GpuMode::Hybrid => "Hybrid mode",
                    GpuMode::Integrated => "Integrated graphics only",
                    GpuMode::Discrete => "NVIDIA graphics only",
                };
                format!("Restart the laptop to switch to {name}.")
            }
            _ => String::new(),
        }
    }

    pub(super) fn refresh_bios(&self) {
        let Some(ui) = self.ui() else { return };
        let writable = self.bios_writable();
        let can_save = (writable.is_ok() || self.demo) && !self.bios.busy;
        ui.set_kb_startup(self.startup_text(Kind::Keyboard).into());
        ui.set_lb_startup(self.startup_text(Kind::LightBar).into());
        ui.set_startup_available(can_save && !self.demo);

        let range = self.voltage_range();
        let saved = range.map_or(0, |r| r.value);
        let chosen = self.bios.voltage.unwrap_or(saved);
        let note = match (&writable, range) {
            (_, None) if self.bios.variable.is_some() => "This BIOS doesn't offer a voltage offset.".to_string(),
            (_, None) => self.bios.error.clone().map_or_else(String::new, |e| format!("The BIOS settings can't be read: {e}")),
            (Err(reason), _) if !self.demo => reason.clone(),
            _ if self.bios.voltage_saved => format!("Saved to the BIOS: {saved:+} mV."),
            _ if chosen != saved => format!("Saved now: {saved:+} mV. Save to use {chosen:+} mV from the next start."),
            _ => "Takes effect when the laptop restarts.".into(),
        };
        let gpu_mode = self.gpu_mode();
        ui.set_gpu_mode_available(gpu_mode.is_some() && can_save && !self.demo);
        ui.set_gpu_mode_shown(gpu_mode.is_some() || self.bios.error.is_some());
        ui.set_gpu_mode(gpu_mode.and_then(|m| GpuMode::ALL.iter().position(|&x| x == m)).unwrap_or(0) as i32);
        ui.set_gpu_mode_note(self.gpu_mode_note(gpu_mode).into());
        ui.set_gpu_mode_restart(self.gpu_mode_restart(gpu_mode).into());
        ui.set_tune_voltage(chosen as f32);
        ui.set_tuning(TuningView {
            voltage_available: range.is_some() && can_save,
            voltage_min: range.map_or(0, |r| r.minimum),
            voltage_max: range.map_or(0, |r| r.maximum),
            voltage_saved: saved,
            voltage_changed: chosen != saved,
            voltage_note: note.into(),
            voltage_restart: if self.bios.voltage_saved { "Restart the laptop to use the new voltage offset.".into() } else { "".into() },
            ..ui.get_tuning()
        });
    }
}
