//! The performance mode, Fan Boost, the fan curves and battery care: what the Performance and Battery pages, the
//! tray and the performance keys change. All of it goes through the EC thread; results come back with `post`.
//!
//! While Control Center's service runs it sets these itself, so the app only shows them then.

use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use slint::{Model, VecModel};

use super::{App, post, with_app};
use crate::hardware::battery::{self, ChargeMode};
use crate::hardware::ec::{Ec, Registers};
use crate::hardware::fans::{self, Step, Table};
use crate::hardware::performance::{self, Mode, PowerLimits};
use crate::hardware::{hotkeys, status, system};
use crate::settings::{FanCurve, PowerSettings};
use crate::shell::osd;
use crate::ui::CurvePoint;

pub(super) struct Power {
    pub ec: Option<Ec>,
    /// The same EC for the monitor thread.
    pub shared_ec: Arc<Mutex<Option<Ec>>>,
    connecting: bool,
    ec_error: Option<String>,
    /// This app sets the mode, the fans and battery care; false while Control Center's service runs.
    owned: bool,
    /// The EC's state has been read since connecting.
    ready: bool,
    /// The settings hold the user's choices (not the first run).
    chosen: bool,
    turbo: bool,
    default_mode: Mode,
    fan_tables: bool,
    pub mode: Mode,
    pub fan_boost: bool,
    limits: Option<PowerLimits>,
    pub on_ac: bool,
    charge: ChargeMode,
    defaults: [Option<Table>; 3],
    curves: [Option<FanCurve>; 3],
    curve_fan: usize, // 0: CPU, 1: GPU
    reading_defaults: bool,
    defaults_tried: bool,
    curve_error: Option<String>,
    safety: bool,
    pub custom_applied: bool,
    curve_timer: slint::Timer,
    /// Updated in place, so a point being dragged isn't recreated under the pointer.
    pub curve_model: Rc<VecModel<CurvePoint>>,
}

impl Power {
    pub fn new(settings: Option<&PowerSettings>, demo: bool) -> Power {
        let defaults = std::array::from_fn(|i| {
            let mode = Mode::from_index(i);
            match settings.and_then(|s| s.default_fan_tables.get(mode.name())) {
                Some(table) if table.is_valid() => Some(table.clone()),
                _ if demo => Some(demo_table(mode)),
                _ => None,
            }
        });
        let curves = std::array::from_fn(|i| settings.and_then(|s| s.fan_curves.get(Mode::from_index(i).name())).cloned());
        Power {
            ec: None,
            shared_ec: Arc::new(Mutex::new(None)),
            connecting: false,
            ec_error: None,
            owned: demo,
            ready: demo,
            chosen: settings.is_some(),
            turbo: demo,
            default_mode: Mode::Office,
            fan_tables: demo,
            mode: settings.map_or(Mode::Gaming, |s| s.mode),
            fan_boost: settings.is_some_and(|s| s.fan_boost),
            limits: demo.then_some(PowerLimits { pl1: 60, pl2: 60, pl4: 165 }),
            on_ac: status::on_ac(),
            charge: settings.map_or(ChargeMode::HighCapacity, |s| s.charge_mode),
            defaults,
            curves,
            curve_fan: 0,
            reading_defaults: false,
            defaults_tried: false,
            curve_error: None,
            safety: false,
            custom_applied: false,
            curve_timer: slint::Timer::default(),
            curve_model: Rc::new(VecModel::default()),
        }
    }

    /// Whether the controls on the Performance and Battery pages work now.
    pub fn available(&self, demo: bool) -> bool {
        demo || (self.owned && self.ready && self.ec.is_some())
    }

    pub fn store(&self, settings: &mut Option<PowerSettings>) {
        if !self.chosen {
            return; // nothing chosen or read yet
        }
        let stored = settings.get_or_insert_with(PowerSettings::default);
        stored.mode = self.mode;
        stored.fan_boost = self.fan_boost;
        stored.charge_mode = self.charge;
        for mode in Mode::ALL {
            if let Some(table) = &self.defaults[mode.index()] {
                stored.default_fan_tables.insert(mode.name().into(), table.clone());
            }
            if let Some(curve) = &self.curves[mode.index()] {
                stored.fan_curves.insert(mode.name().into(), curve.clone());
            }
        }
    }

    /// The table the fans should follow in the current mode, if it's a custom one.
    fn custom_table(&self) -> Option<Table> {
        let defaults = self.defaults[self.mode.index()].as_ref()?;
        let curve = self.curves[self.mode.index()].as_ref().filter(|c| c.enabled)?;
        if !self.fan_tables || self.safety {
            return None;
        }
        let mut table = curve.table.clone();
        fans::make_safe(&mut table.cpu, &defaults.cpu);
        fans::make_safe(&mut table.gpu, &defaults.gpu);
        table.is_valid().then_some(table)
    }

    /// The mode the performance key switches to (MyFanManager_RamFan1p5.ModeSwitchChanged).
    fn next_mode(&self) -> Mode {
        match (self.mode, self.turbo) {
            (Mode::Office, _) => Mode::Gaming,
            (Mode::Gaming, true) => Mode::Turbo,
            _ => Mode::Office,
        }
    }
}

/// What the EC reports, read in one go.
struct EcState {
    turbo: bool,
    default_mode: Mode,
    mode: Mode,
    fan_boost: bool,
    charge: ChargeMode,
    fan_tables: bool,
    safety: bool,
    limits: PowerLimits,
}

fn read_state(ec: &Registers) -> Result<EcState> {
    let support = performance::support(ec)?;
    let (mode, fan_boost) = performance::current(ec)?;
    Ok(EcState {
        turbo: support.turbo,
        default_mode: support.default_mode,
        mode,
        fan_boost,
        charge: battery::current(ec)?,
        fan_tables: fans::supported(ec)?,
        safety: fans::safety_protection(ec)?,
        limits: performance::limits(ec)?,
    })
}

fn error_text(e: anyhow::Error) -> String {
    format!("{e:#}")
}

fn mode_glyph(mode: Mode) -> char {
    match mode {
        Mode::Office => osd::OFFICE_GLYPH,
        Mode::Gaming => osd::GAMING_GLYPH,
        Mode::Turbo => osd::TURBO_GLYPH,
    }
}

impl App {
    /// Connects to the EC off the UI thread; the Fn keys and the power settings start once it answers.
    pub(super) fn connect_ec(&mut self) {
        if self.demo || self.power.ec.is_some() || self.power.connecting {
            return;
        }
        self.power.connecting = true;
        let _ = std::thread::Builder::new().name("EC connect".into()).spawn(|| {
            let result = Ec::connect().map_err(error_text);
            post(move |app| app.ec_connected(result));
        });
    }

    fn ec_connected(&mut self, result: std::result::Result<Ec, String>) {
        self.power.connecting = false;
        match result {
            Ok(ec) => {
                *self.power.shared_ec.lock().unwrap_or_else(|e| e.into_inner()) = Some(ec.clone());
                self.power.ec = Some(ec);
                self.power.ec_error = None;
                self.start_hotkeys();
                self.check_power_owner();
            }
            Err(reason) => {
                self.set_fn_mode(&format!("Fn keys: handled by the firmware ({reason})"));
                self.power.ec_error = Some(reason);
                self.refresh_power();
            }
        }
    }

    /// Takes the power settings over when Control Center isn't running, and steps back when it starts. Also notices
    /// the EC's fan safety protection while a custom curve is on. Called every few seconds.
    pub(super) fn check_power_owner(&mut self) {
        let Some(ec) = self.power.ec.clone() else { return };
        if system::control_center_running() {
            if self.power.owned || !self.power.ready {
                self.power.owned = false;
                self.read_power_state();
            }
            return;
        }
        if !self.power.owned {
            self.power.owned = true;
            self.read_power_state();
        } else if self.power.custom_applied {
            ec.spawn(|ec| {
                if let Ok(true) = fans::safety_protection(ec) {
                    post(|app| app.safety_changed(true));
                }
            });
        }
    }

    fn read_power_state(&mut self) {
        let Some(ec) = self.power.ec.clone() else { return };
        ec.spawn(|ec| {
            let state = read_state(ec).map_err(error_text);
            post(move |app| app.power_state_read(state));
        });
    }

    fn power_state_read(&mut self, state: std::result::Result<EcState, String>) {
        let state = match state {
            Ok(state) => state,
            Err(reason) => {
                self.power.ec_error = Some(reason);
                self.refresh_power();
                return;
            }
        };
        let power = &mut self.power;
        power.turbo = state.turbo;
        power.default_mode = state.default_mode;
        power.fan_tables = state.fan_tables;
        power.safety = state.safety;
        power.limits = Some(state.limits);
        power.ready = true;
        if !power.owned {
            // Control Center's choices, shown as they are.
            power.mode = state.mode;
            power.fan_boost = state.fan_boost;
            power.charge = state.charge;
            power.custom_applied = false;
            self.refresh_power();
            return;
        }
        if !power.chosen {
            // The first run keeps what's set now, and battery care as Control Center last had it.
            power.mode = state.mode;
            power.fan_boost = state.fan_boost;
            power.charge = battery::control_center_choice().unwrap_or(state.charge);
            power.chosen = true;
            self.settings_dirty = true;
        }
        if power.mode == Mode::Turbo && !power.turbo {
            power.mode = power.default_mode;
        }
        self.apply_power();
    }

    /// Sends the mode, Fan Boost, the power limits, battery care and the fan curve, as Control Center does at start,
    /// on resume and when the power source changes.
    pub(super) fn apply_power(&mut self) {
        self.power.on_ac = status::on_ac();
        self.refresh_power();
        let Some(ec) = self.power.ec.clone() else { return };
        if !self.power.owned || !self.power.ready {
            return;
        }
        let (mode, boost, charge, on_ac, fan_tables) = (self.power.mode, self.power.fan_boost, self.power.charge, self.power.on_ac, self.power.fan_tables);
        let table = self.power.custom_table();
        let custom = table.is_some();
        ec.spawn(move |ec| {
            let result = (|| -> Result<PowerLimits> {
                let limits = performance::apply(ec, mode, boost, on_ac)?;
                battery::set(ec, charge)?;
                if fan_tables {
                    apply_fan_table(ec, table.as_ref())?;
                }
                Ok(limits)
            })();
            let result = result.map_err(error_text);
            post(move |app| app.power_applied(result, custom));
        });
    }

    fn power_applied(&mut self, result: std::result::Result<PowerLimits, String>, custom: bool) {
        match result {
            Ok(limits) => {
                self.power.limits = Some(limits);
                self.note_custom(custom);
            }
            Err(reason) => self.set_status(&format!("Couldn't set the performance mode: {reason}"), true, false),
        }
        self.refresh_power();
    }

    fn note_custom(&mut self, custom: bool) {
        self.power.custom_applied = custom;
        fans::note_custom(if custom { self.power.ec.as_ref() } else { None });
    }

    pub(super) fn set_mode(&mut self, mode: Mode) {
        if !self.power.available(self.demo) || (mode == Mode::Turbo && !self.power.turbo) {
            return;
        }
        if mode == self.power.mode {
            return;
        }
        self.power.mode = mode;
        self.power.chosen = true;
        self.settings_dirty = true;
        self.apply_power();
    }

    pub(super) fn set_fan_boost(&mut self, on: bool) {
        if !self.power.available(self.demo) {
            return;
        }
        self.power.fan_boost = on;
        self.power.chosen = true;
        self.settings_dirty = true;
        self.write_fan_mode();
        self.refresh_power();
    }

    fn write_fan_mode(&self) {
        let (mode, boost) = (self.power.mode, self.power.fan_boost);
        if let Some(ec) = &self.power.ec
            && self.power.owned
        {
            ec.spawn(move |ec| {
                let _ = performance::set_fan_mode(ec, mode, boost);
            });
        }
    }

    pub(super) fn set_charge_mode(&mut self, mode: ChargeMode) {
        if !self.power.available(self.demo) {
            return;
        }
        self.power.charge = mode;
        self.power.chosen = true;
        self.settings_dirty = true;
        if let Some(ec) = &self.power.ec {
            ec.spawn(move |ec| {
                if let Err(e) = battery::set(ec, mode) {
                    let reason = error_text(e);
                    post(move |app| app.set_status(&format!("Couldn't set battery care: {reason}"), true, false));
                }
            });
        }
        self.refresh_power();
    }

    /// The performance key, Fan Boost and the EC's fan safety protection report through the hotkey events.
    pub(super) fn on_power_hotkey(&mut self, code: u32) {
        if !self.power.owned {
            return;
        }
        match code {
            hotkeys::MODE_SWITCH => {
                let next = self.power.next_mode();
                self.osd.show_text(mode_glyph(next), &format!("{} mode", next.name()));
                self.set_mode(next);
            }
            hotkeys::FAN_BOOST_CHANGED | hotkeys::FAN_BOOST_OFF => {
                let Some(ec) = self.power.ec.clone() else { return };
                ec.spawn(move |ec| {
                    let on = code == hotkeys::FAN_BOOST_CHANGED && performance::current(ec).is_ok_and(|(_, boost)| boost);
                    post(move |app| app.fan_boost_from_ec(on, code == hotkeys::FAN_BOOST_CHANGED));
                });
            }
            hotkeys::FAN_SAFETY => {
                let Some(ec) = self.power.ec.clone() else { return };
                ec.spawn(|ec| {
                    if let Ok(on) = fans::safety_protection(ec) {
                        post(move |app| app.safety_changed(on));
                    }
                });
            }
            _ => {}
        }
    }

    fn fan_boost_from_ec(&mut self, on: bool, show: bool) {
        self.power.fan_boost = on;
        self.settings_dirty = true;
        self.write_fan_mode(); // as FanBoostUpdate: the mode's byte again, with the new Fan Boost
        if show {
            self.osd.show_text(osd::FAN_GLYPH, if on { "Fan Boost on" } else { "Fan Boost off" });
        }
        self.refresh_power();
    }

    /// The EC's fan safety protection: Control Center goes back to the default tables while it's on.
    fn safety_changed(&mut self, on: bool) {
        if on == self.power.safety {
            return;
        }
        self.power.safety = on;
        let had_custom = self.power.custom_applied;
        self.apply_fan_curve();
        if on
            && had_custom
            && let Some(tray) = &self.tray
        {
            tray.notify("Fan safety protection", "The laptop turned its fan safety protection on, so the fans follow its own curve for now.");
        }
    }

    pub(super) fn power_source_changed(&mut self) {
        if status::on_ac() != self.power.on_ac {
            self.apply_power(); // the power limits differ on battery
        }
    }

    // ---- Fan curves -----------------------------------------------------------------------------------------------

    /// Reads the EC's own fan tables once, so the curve editor has them to start from and to keep within.
    pub(super) fn read_fan_defaults_if_needed(&mut self) {
        let power = &self.power;
        if self.demo || power.reading_defaults || power.defaults_tried || !power.fan_tables || !power.available(false) {
            return;
        }
        if power.defaults.iter().all(|d| d.is_some()) {
            return;
        }
        self.read_fan_defaults(false);
    }

    fn read_fan_defaults(&mut self, enable: bool) {
        let Some(ec) = self.power.ec.clone() else { return };
        self.power.reading_defaults = true;
        self.power.defaults_tried = true;
        self.power.curve_error = None;
        self.refresh_power();
        ec.spawn(move |ec| {
            let result = fans::read_defaults(ec).map_err(error_text);
            post(move |app| app.fan_defaults_read(result, enable));
        });
    }

    fn fan_defaults_read(&mut self, result: std::result::Result<Vec<(Mode, Table)>, String>, enable: bool) {
        self.power.reading_defaults = false;
        match result {
            Ok(tables) => {
                for (mode, table) in tables {
                    self.power.defaults[mode.index()] = Some(table);
                }
                self.power.chosen = true;
                self.settings_dirty = true;
                if enable {
                    self.set_curve_enabled(true);
                    return;
                }
            }
            Err(reason) => self.power.curve_error = Some(format!("Couldn't read the laptop's fan curves: {reason}")),
        }
        // Reading them turned the RAM tables off; a custom curve goes back on.
        self.apply_fan_curve();
    }

    /// Makes the fans follow the custom curve of the current mode, or the EC's own.
    fn apply_fan_curve(&mut self) {
        self.refresh_power();
        let Some(ec) = self.power.ec.clone() else { return };
        if !self.power.owned || !self.power.fan_tables {
            return;
        }
        let table = self.power.custom_table();
        let custom = table.is_some();
        ec.spawn(move |ec| {
            let result = apply_fan_table(ec, table.as_ref()).map_err(error_text);
            post(move |app| match result {
                Ok(()) => {
                    app.note_custom(custom);
                    app.refresh_power();
                }
                Err(reason) => {
                    app.note_custom(false);
                    app.power.curve_error = Some(format!("Couldn't set the fan curve: {reason}"));
                    app.refresh_power();
                }
            });
        });
    }

    /// Writes the curve a moment after the last change, so dragging doesn't flood the EC.
    fn queue_fan_curve(&mut self) {
        self.settings_dirty = true;
        self.refresh_power();
        self.power.curve_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(250), || with_app(|app| app.apply_fan_curve()));
    }

    pub(super) fn set_curve_enabled(&mut self, on: bool) {
        if !self.power.available(self.demo) || !self.power.fan_tables {
            return;
        }
        let index = self.power.mode.index();
        let Some(defaults) = self.power.defaults[index].clone() else {
            if on {
                self.read_fan_defaults(true);
            }
            return;
        };
        let curve = self.power.curves[index].get_or_insert_with(|| FanCurve { enabled: false, table: defaults.clone() });
        curve.enabled = on;
        fans::make_safe(&mut curve.table.cpu, &defaults.cpu);
        fans::make_safe(&mut curve.table.gpu, &defaults.gpu);
        self.power.chosen = true;
        self.power.curve_error = None;
        self.settings_dirty = true;
        self.apply_fan_curve();
    }

    pub(super) fn select_curve_fan(&mut self, fan: usize) {
        self.power.curve_fan = fan.min(1);
        self.refresh_power();
    }

    pub(super) fn set_curve_duty(&mut self, step: usize, duty: i32) {
        let index = self.power.mode.index();
        let Some(defaults) = self.power.defaults[index].clone() else { return };
        let fan = self.power.curve_fan;
        let Some(curve) = self.power.curves[index].as_mut().filter(|c| c.enabled) else { return };
        let (steps, default_steps) = if fan == 0 { (&mut curve.table.cpu, &defaults.cpu) } else { (&mut curve.table.gpu, &defaults.gpu) };
        fans::make_safe(steps, default_steps);
        let before = steps[step.min(fans::STEPS - 1)].duty;
        fans::set_duty(steps, default_steps, step, duty.clamp(0, 100) as u8);
        if steps[step.min(fans::STEPS - 1)].duty != before {
            self.queue_fan_curve();
        }
    }

    pub(super) fn reset_curve(&mut self) {
        let index = self.power.mode.index();
        let Some(defaults) = self.power.defaults[index].clone() else { return };
        if let Some(curve) = self.power.curves[index].as_mut() {
            curve.table = defaults;
            self.queue_fan_curve();
        }
    }

    /// On exit: the fans go back to the EC's own curve, since nothing will watch the safety protection.
    pub(super) fn release_fans(&mut self) {
        if self.power.custom_applied
            && let Some(ec) = &self.power.ec
        {
            let _ = ec.run_for(Duration::from_secs(2), |ec| fans::set_custom(ec, false));
            self.note_custom(false);
        }
    }

    // ---- The pages ------------------------------------------------------------------------------------------------

    pub(super) fn refresh_power(&self) {
        let Some(ui) = self.ui() else { return };
        let power = &self.power;
        let available = power.available(self.demo);
        let note = if self.demo {
            "Demo mode: nothing is sent to the laptop.".to_string()
        } else if let Some(reason) = &power.ec_error {
            format!("Can't reach the laptop's embedded controller: {reason}")
        } else if power.ec.is_some() && !power.owned && power.ready {
            "Control Center is running and sets these itself, so they're only shown here. To set them here, close Control Center and stop its service (GCUService).".to_string()
        } else if !power.ready {
            "Connecting to the laptop's embedded controller…".to_string()
        } else {
            String::new()
        };
        ui.set_perf_available(available);
        ui.set_perf_note(note.clone().into());
        ui.set_perf_mode(power.mode.index() as i32);
        ui.set_perf_turbo(power.turbo || power.mode == Mode::Turbo);
        ui.set_fan_boost(power.fan_boost);
        ui.set_perf_limits(match power.limits {
            _ if !power.on_ac => "on battery".into(),
            Some(limits) if limits.pl1 > 0 => format!("CPU {} W sustained, {} W boost", limits.pl1, limits.pl2.max(limits.pl1)).into(),
            _ => "".into(),
        });
        ui.set_battery_available(available);
        ui.set_battery_note(if available { "".into() } else { note.into() });
        ui.set_charge_mode(power.charge as i32);

        // The fan curve: the custom one if there is one, else the EC's.
        let index = power.mode.index();
        let defaults = power.defaults[index].as_ref();
        let curve = power.curves[index].as_ref();
        ui.set_curve_available(power.fan_tables && available);
        ui.set_curve_enabled(curve.is_some_and(|c| c.enabled));
        ui.set_curve_busy(power.reading_defaults);
        ui.set_fan_safety(power.safety);
        ui.set_curve_fan(power.curve_fan as i32);
        ui.set_curve_note(
            if power.reading_defaults {
                "Reading the laptop's fan curves. This takes a few seconds; the Fn keys are handled by the laptop meanwhile.".into()
            } else if let Some(error) = &power.curve_error {
                error.clone().into()
            } else if !power.fan_tables && power.ready {
                "This laptop's controller doesn't take custom fan curves.".into()
            } else {
                "".into()
            },
        );
        let pick = |table: &Table| if power.curve_fan == 0 { table.cpu.clone() } else { table.gpu.clone() };
        let default_steps = defaults.map(pick).unwrap_or_default();
        let shown = match curve {
            Some(c) if c.enabled => pick(&c.table),
            _ => default_steps.clone(),
        };
        let (points, path) = curve_shape(&shown, &default_steps);
        let (_, default_path) = curve_shape(&default_steps, &default_steps);
        let model = &power.curve_model;
        if model.row_count() == points.len() {
            for (i, point) in points.into_iter().enumerate() {
                if model.row_data(i) != Some(point.clone()) {
                    model.set_row_data(i, point);
                }
            }
        } else {
            model.set_vec(points);
        }
        ui.set_curve_path(path.into());
        ui.set_default_path(default_path.into());
    }

    /// Tray menu entries for the performance mode and Fan Boost.
    pub(super) fn power_menu(&self) -> Option<(Vec<(u32, &'static str, bool)>, bool)> {
        if !self.power.available(self.demo) {
            return None;
        }
        let modes = Mode::ALL
            .iter()
            .filter(|&&m| m != Mode::Turbo || self.power.turbo)
            .map(|&m| (m.index() as u32, m.name(), m == self.power.mode))
            .collect();
        Some((modes, self.power.fan_boost))
    }
}

/// Writes the custom table and turns the RAM tables on, or turns them off.
fn apply_fan_table(ec: &Registers, table: Option<&Table>) -> Result<()> {
    match table {
        Some(table) => fans::apply_custom(ec, table),
        None if fans::custom_active(ec)? => fans::set_custom(ec, false),
        None => Ok(()),
    }
}

/// Where a curve's steps go on the chart (30-100 °C across, 0-100% up), and its line as path commands in a
/// 1000 × 1000 view box: flat at a step's speed until the next step's temperature.
fn curve_shape(steps: &[Step], defaults: &[Step]) -> (Vec<CurvePoint>, String) {
    if steps.is_empty() {
        return (Vec::new(), String::new());
    }
    let used = fans::used_steps(if defaults.len() == steps.len() { defaults } else { steps }).min(steps.len());
    let x = |i: usize| if i == 0 { 0.0 } else { ((steps[i].up as f32).clamp(30.0, 100.0) - 30.0) / 70.0 };
    let y = |i: usize| steps[i].duty.min(100) as f32 / 100.0;
    let points = (0..used).map(|i| CurvePoint { x: x(i), y: y(i), temp: steps[i].up as i32, duty: steps[i].duty as i32 }).collect();
    let v = |f: f32| (f * 1000.0).round();
    let mut path = format!("M 0 {}", v(1.0 - y(0)));
    for i in 1..used {
        path += &format!(" L {} {} L {} {}", v(x(i)), v(1.0 - y(i - 1)), v(x(i)), v(1.0 - y(i)));
    }
    path += &format!(" L 1000 {}", v(1.0 - y(used - 1)));
    (points, path)
}

/// A plausible table for trying the editor in demo mode (the shape of this laptop's Gaming table).
fn demo_table(mode: Mode) -> Table {
    let ups = [0u8, 53, 57, 59, 61, 63, 65, 67, 75, 80];
    let duties: [u8; 10] = match mode {
        Mode::Office => [0, 25, 25, 30, 35, 40, 45, 50, 65, 80],
        Mode::Gaming => [0, 30, 30, 35, 45, 48, 50, 55, 70, 85],
        Mode::Turbo => [0, 35, 35, 40, 50, 55, 60, 65, 80, 95],
    };
    let steps: Vec<Step> = (0..fans::STEPS)
        .map(|i| Step {
            up: ups.get(i).copied().unwrap_or(255),
            down: ups.get(i + 1).map_or(255, |u| u.saturating_sub(4)),
            duty: duties.get(i).copied().unwrap_or(duties[9]),
        })
        .collect();
    Table { cpu: steps.clone(), gpu: steps }
}
