//! The performance mode, Fan Boost, the fan curves and battery care: what the Performance and Battery pages, the
//! tray and the performance keys change. All of it goes through the EC thread; results come back with `post`.
//!
//! While Control Center's service runs it sets these itself, so the app only shows them then.

use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use slint::{ComponentHandle, Model, SharedString, VecModel};

use super::{App, post, with_app};
use crate::hardware::battery::{self, ChargeMode};
use crate::hardware::ec::{Ec, Registers};
use crate::hardware::fans::{self, Step, Table};
use crate::hardware::performance::{self, Defaults, GpuPower, Mode, PowerLimits, Ranges, Tuning};
use crate::hardware::{hotkeys, status, system};
use crate::settings::{FanCurve, ModeProfile, PowerSettings};
use crate::shell::osd;
use crate::ui::{CurvePoint, TuningView};

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
    pub turbo: bool,
    default_mode: Mode,
    fan_tables: bool,
    pub mode: Mode,
    pub fan_boost: bool,
    limits: Option<PowerLimits>,
    pub on_ac: bool,
    pub charge: ChargeMode,
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
    /// The highest power limits and TCC offset the BIOS allows; None if its table can't be read.
    ranges: Option<Ranges>,
    mode_defaults: [Option<Defaults>; 3],
    /// The watts above the GPU's default TGP the EC picks for each mode, read while it's left to the EC.
    gpu_own: [Option<u8>; 3],
    gpu_own_timer: slint::Timer,
    pub tuning: [Tuning; 3],
    tuning_timer: slint::Timer,
    /// Each mode's profiles, "Standard" first, and the active one, whose values are `tuning` and `curves`.
    pub profiles: [Vec<ModeProfile>; 3],
    pub active_profile: [usize; 3],
    /// The current mode's profile names, for the page.
    pub profile_model: Rc<VecModel<SharedString>>,
    /// USB charging while off: whether the laptop has it, whether it's on, and whether the user chose that.
    usb_supported: bool,
    usb_power: bool,
    usb_chosen: bool,
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
        let tuning = std::array::from_fn(|i| settings.and_then(|s| s.tuning.get(Mode::from_index(i).name())).copied().unwrap_or_default());
        let profiles: [Vec<ModeProfile>; 3] = std::array::from_fn(|i| {
            let mut list = settings.and_then(|s| s.profiles.get(Mode::from_index(i).name())).cloned().unwrap_or_default();
            if list.is_empty() {
                list.push(ModeProfile { name: STANDARD_PROFILE.into(), ..Default::default() });
            }
            list.truncate(MAX_PROFILES);
            list
        });
        let active_profile = std::array::from_fn(|i| {
            settings.and_then(|s| s.active_profiles.get(Mode::from_index(i).name())).copied().unwrap_or(0).min(profiles[i].len() - 1)
        });
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
            ranges: demo.then_some(Ranges { pl1: 120, pl2: 120, pl4: 165, tcc: 5, gpu_tgp: 80, gpu_extra: 35, dynamic_boost: 15 }),
            mode_defaults: std::array::from_fn(|i| demo.then(|| demo_defaults(Mode::from_index(i)))),
            gpu_own: [demo.then_some(10), demo.then_some(25), demo.then_some(35)],
            gpu_own_timer: slint::Timer::default(),
            tuning,
            tuning_timer: slint::Timer::default(),
            profiles,
            active_profile,
            profile_model: Rc::new(VecModel::default()),
            usb_supported: demo,
            usb_power: settings.and_then(|s| s.usb_power).unwrap_or(false),
            usb_chosen: settings.is_some_and(|s| s.usb_power.is_some()),
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
            stored.tuning.insert(mode.name().into(), self.tuning[mode.index()]);
            stored.profiles.insert(mode.name().into(), self.profiles_now(mode));
            stored.active_profiles.insert(mode.name().into(), self.active_profile[mode.index()]);
        }
        if self.usb_chosen {
            stored.usb_power = Some(self.usb_power);
        }
    }

    /// The BIOS's ranges, or if its table can't be read, the highest of the modes' own limits (no TCC offset then).
    fn ranges(&self) -> Option<Ranges> {
        self.ranges.or_else(|| {
            let limits: Vec<PowerLimits> = self.mode_defaults.iter().flatten().map(|d| d.limits).collect();
            let max = |f: fn(&PowerLimits) -> u8| limits.iter().map(f).max().unwrap_or(0);
            let ranges = Ranges { pl1: max(|l| l.pl1), pl2: max(|l| l.pl2), pl4: max(|l| l.pl4), ..Ranges::default() };
            (ranges.pl1 >= performance::MIN_LIMIT).then_some(ranges)
        })
    }

    /// The tuning to send for `mode`, kept within the ranges; None leaves the mode's defaults.
    fn tuning_for(&self, mode: Mode) -> Option<Tuning> {
        let ranges = self.ranges()?;
        let mut tuning = self.tuning[mode.index()];
        tuning.limits = tuning.limits.checked(&ranges);
        tuning.tcc_offset = tuning.tcc_offset.min(ranges.tcc);
        tuning.tcc_enabled &= ranges.tcc > 0;
        Some(tuning)
    }

    /// The GPU's power to send for `mode`; None if the BIOS's table doesn't offer it (or couldn't be read).
    fn gpu_power_for(&self, mode: Mode) -> Option<GpuPower> {
        self.tuning[mode.index()].gpu_power(self.ranges.as_ref()?)
    }

    /// Back to each mode's defaults (after the BIOS reset the tuning, as Control Center's LoadDefault).
    pub fn reset_all_tuning(&mut self) {
        self.tuning = Default::default();
        for profile in self.profiles.iter_mut().flatten() {
            profile.tuning = Tuning::default();
        }
        self.chosen = true;
    }

    /// A mode's profiles with the active one holding what's set now.
    fn profiles_now(&self, mode: Mode) -> Vec<ModeProfile> {
        let i = mode.index();
        let mut list = self.profiles[i].clone();
        if let Some(active) = list.get_mut(self.active_profile[i]) {
            active.tuning = self.tuning[i];
            active.fan_curve = self.curves[i].clone();
        }
        list
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
    ranges: Option<Ranges>,
    defaults: [Option<Defaults>; 3],
    usb_power: Option<bool>,
}

/// `usb_known`: the laptop is known to have USB charging while off.
fn read_state(ec: &Registers, usb_known: bool) -> Result<EcState> {
    let support = performance::support(ec)?;
    let (mode, fan_boost) = performance::current(ec)?;
    let mut defaults = [None; 3];
    for mode in Mode::ALL {
        defaults[mode.index()] = Some(performance::defaults(ec, mode)?);
    }
    Ok(EcState {
        ranges: performance::ranges().ok(),
        defaults,
        turbo: support.turbo,
        default_mode: support.default_mode,
        mode,
        fan_boost,
        charge: battery::current(ec)?,
        fan_tables: fans::supported(ec)?,
        safety: fans::safety_protection(ec)?,
        limits: performance::limits(ec)?,
        usb_power: battery::usb_power(ec, usb_known)?,
    })
}

/// The profile every mode starts with, which can't be deleted.
const STANDARD_PROFILE: &str = "Standard";
/// As many profiles per mode as Control Center keeps.
const MAX_PROFILES: usize = 5;

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
                self.set_ec_unavailable("");
                *self.power.shared_ec.lock().unwrap_or_else(|e| e.into_inner()) = Some(ec.clone());
                self.power.ec = Some(ec);
                self.power.ec_error = None;
                self.start_hotkeys();
                self.check_power_owner();
            }
            Err(reason) => {
                self.set_ec_unavailable(&reason);
                self.set_fn_mode(&format!("Fn keys: handled by the firmware ({reason})"));
                self.power.ec_error = Some(reason);
                self.refresh_power();
            }
        }
    }

    /// On Linux, the Performance and Battery pages show why the EC can't be used instead of their controls. Windows
    /// keeps its controls and says why in their note.
    fn set_ec_unavailable(&self, reason: &str) {
        if cfg!(target_os = "linux")
            && let Some(ui) = self.ui()
        {
            ui.global::<crate::ui::Platform>().set_ec_unavailable(reason.into());
        }
    }

    /// Takes the power settings over when Control Center isn't running, and steps back when it starts. Also notices
    /// the EC's fan safety protection while a custom curve is on. Called every few seconds.
    pub(super) fn check_power_owner(&mut self) {
        if self.power.ec.is_none() {
            // A helper that wasn't running may be now.
            if self.power.ec_error.as_deref().is_some_and(crate::hardware::ec::may_come_back) {
                self.connect_ec();
            }
            return;
        }
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
        } else {
            self.check_fan_safety();
        }
    }

    /// Notices the EC's fan safety protection while a custom curve is on. On Windows the firmware also reports it as
    /// a hotkey; Linux has only this, so it runs every second there.
    pub(super) fn check_fan_safety(&self) {
        let Some(ec) = self.power.ec.clone() else { return };
        if !safety_check_due(self.power.owned, self.power.custom_applied) {
            return;
        }
        ec.spawn(|ec| {
            if let Ok(true) = fans::safety_protection(ec) {
                post(|app| app.safety_changed(true));
            }
        });
    }

    fn read_power_state(&mut self) {
        let Some(ec) = self.power.ec.clone() else { return };
        // A saved USB charging choice means the switch was there, even if Control Center's own setting is gone now.
        let usb_known = self.power.usb_chosen;
        ec.spawn(move |ec| {
            let state = read_state(ec, usb_known).map_err(error_text);
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
        power.ranges = state.ranges;
        power.mode_defaults = state.defaults;
        power.ready = true;
        power.usb_supported = state.usb_power.is_some();
        if !power.owned || !power.usb_chosen {
            power.usb_power = state.usb_power.unwrap_or(false);
        }
        if power.owned && !power.usb_chosen && power.usb_supported {
            // The first run keeps Control Center's choice, which its service set at every start.
            if let Some(on) = battery::control_center_usb_power() {
                power.usb_power = on;
                power.usb_chosen = true;
                self.settings_dirty = true;
            }
        }
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
        let tuning = self.power.tuning_for(mode);
        let gpu = self.power.gpu_power_for(mode);
        let usb_power = (self.power.usb_supported && self.power.usb_chosen).then_some(self.power.usb_power);
        let cli = self.cli.tag();
        ec.spawn(move |ec| {
            let result = (|| -> Result<PowerLimits> {
                let limits = performance::apply(ec, mode, boost, on_ac, tuning.as_ref(), gpu)?;
                battery::set(ec, charge)?;
                if let Some(on) = usb_power {
                    battery::set_usb_power(ec, on)?;
                }
                if fan_tables {
                    apply_fan_table(ec, table.as_ref())?;
                }
                Ok(limits)
            })();
            let result = result.map_err(error_text);
            post(move |app| app.power_applied(result, custom, cli));
        });
    }

    fn power_applied(&mut self, result: std::result::Result<PowerLimits, String>, custom: bool, cli: Option<u64>) {
        match result {
            Ok(limits) => {
                self.power.limits = Some(limits);
                self.note_custom(custom);
                self.queue_gpu_own_read();
            }
            Err(reason) => {
                let text = format!("Couldn't set the performance mode: {reason}");
                self.cli.reported(cli, &text);
                self.set_status(&text, true, false);
            }
        }
        self.refresh_power();
    }

    // ---- CPU and GPU tuning ---------------------------------------------------------------------------------------

    /// Sends the current mode's power limits, TCC offset and GPU power a moment after the last change.
    fn queue_tuning(&mut self) {
        self.power.chosen = true;
        self.settings_dirty = true;
        self.refresh_power();
        self.power.tuning_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(300), || with_app(|app| app.apply_tuning()));
    }

    fn apply_tuning(&mut self) {
        self.power.on_ac = status::on_ac();
        let Some(ec) = self.power.ec.clone() else { return };
        if !self.power.owned || !self.power.ready {
            return;
        }
        let (mode, on_ac) = (self.power.mode, self.power.on_ac);
        let tuning = self.power.tuning_for(mode);
        let gpu = self.power.gpu_power_for(mode);
        ec.spawn(move |ec| {
            let result = performance::set_tuning(ec, mode, on_ac, tuning.as_ref(), gpu).map_err(error_text);
            post(move |app| match result {
                Ok(limits) => {
                    app.power.limits = Some(limits);
                    app.queue_gpu_own_read();
                    app.refresh_power();
                }
                Err(reason) => app.set_status(&format!("Couldn't set the tuning: {reason}"), true, false),
            });
        });
    }

    /// While the current mode's TGP is left to the EC, reads what it picked, once the EC has passed it on (within
    /// about 2 s), so the page can show it.
    fn queue_gpu_own_read(&mut self) {
        let mode = self.power.mode;
        let left_to_ec = self.power.gpu_power_for(mode).is_some_and(|g| g.extra.is_none());
        if !left_to_ec || !self.power.on_ac || self.demo {
            return;
        }
        self.power.gpu_own_timer.start(slint::TimerMode::SingleShot, Duration::from_secs(3), move || {
            with_app(move |app| {
                let Some(ec) = app.power.ec.clone() else { return };
                if app.power.mode != mode {
                    return;
                }
                ec.spawn(move |ec| {
                    if let Ok(extra) = performance::gpu_extra_in_use(ec) {
                        post(move |app| {
                            app.power.gpu_own[mode.index()] = Some(extra);
                            app.refresh_power();
                        });
                    }
                });
            })
        });
    }

    fn can_tune(&self) -> bool {
        self.power.available(self.demo) && self.power.ranges().is_some()
    }

    pub(super) fn set_custom_limits(&mut self, on: bool) {
        if !self.can_tune() {
            return;
        }
        let index = self.power.mode.index();
        let defaults = self.power.mode_defaults[index].map(|d| d.limits).unwrap_or_default();
        let tuning = &mut self.power.tuning[index];
        if on && tuning.limits == PowerLimits::default() {
            tuning.limits = defaults; // start from the mode's own
        }
        tuning.custom_limits = on;
        self.queue_tuning();
    }

    /// `which`: 0 PL1, 1 PL2, 2 PL4.
    pub(super) fn set_limit(&mut self, which: usize, watts: u8) {
        let Some(ranges) = self.power.ranges() else { return };
        if !self.can_tune() {
            return;
        }
        let tuning = &mut self.power.tuning[self.power.mode.index()];
        if !tuning.custom_limits {
            return;
        }
        let limits = &mut tuning.limits;
        match which {
            0 => limits.pl1 = watts,
            1 => limits.pl2 = watts,
            _ => limits.pl4 = watts,
        }
        // Lowering a later limit below an earlier one lowers the earlier one with it.
        limits.pl1 = limits.pl1.min(if which == 1 { limits.pl2 } else if which == 2 { limits.pl4 } else { limits.pl1 });
        limits.pl2 = limits.pl2.min(if which == 2 { limits.pl4 } else { limits.pl2 });
        *limits = limits.checked(&ranges);
        self.queue_tuning();
    }

    pub(super) fn set_tcc(&mut self, on: bool) {
        let Some(ranges) = self.power.ranges() else { return };
        if !self.can_tune() || ranges.tcc == 0 {
            return;
        }
        let index = self.power.mode.index();
        let default = self.power.mode_defaults[index].map_or(0, |d| d.tcc);
        let tuning = &mut self.power.tuning[index];
        if on && !tuning.tcc_enabled && tuning.tcc_offset == 0 {
            tuning.tcc_offset = default.min(ranges.tcc); // Control Center's profiles start there
        }
        tuning.tcc_enabled = on;
        self.queue_tuning();
    }

    pub(super) fn set_tcc_offset(&mut self, degrees: u8) {
        let Some(ranges) = self.power.ranges() else { return };
        let tuning = &mut self.power.tuning[self.power.mode.index()];
        if !tuning.tcc_enabled {
            return;
        }
        tuning.tcc_offset = degrees.min(ranges.tcc);
        self.queue_tuning();
    }

    /// The GPU's TGP above its default, or back to it.
    pub(super) fn set_custom_gpu(&mut self, on: bool) {
        let Some(ranges) = self.power.ranges.filter(|r| r.gpu()) else { return };
        if !self.can_tune() {
            return;
        }
        let own = self.power.gpu_own[self.power.mode.index()];
        let tuning = &mut self.power.tuning[self.power.mode.index()];
        if on && tuning.gpu_tgp == 0 {
            tuning.gpu_tgp = ranges.gpu_tgp + own.unwrap_or(0).min(ranges.gpu_extra); // start from the mode's own
        }
        tuning.custom_gpu = on;
        self.queue_tuning();
    }

    pub(super) fn set_gpu_tgp(&mut self, watts: u8) {
        let Some(ranges) = self.power.ranges.filter(|r| r.gpu()) else { return };
        let tuning = &mut self.power.tuning[self.power.mode.index()];
        if !tuning.custom_gpu {
            return;
        }
        tuning.gpu_tgp = watts.clamp(ranges.gpu_tgp, ranges.gpu_tgp + ranges.gpu_extra);
        self.queue_tuning();
    }

    pub(super) fn set_dynamic_boost(&mut self, on: bool) {
        if !self.can_tune() || !self.power.ranges.is_some_and(|r| r.dynamic_boost > 0) {
            return;
        }
        self.power.tuning[self.power.mode.index()].dynamic_boost_off = !on;
        self.queue_tuning();
    }

    pub(super) fn reset_tuning(&mut self) {
        if !self.power.available(self.demo) {
            return;
        }
        self.power.tuning[self.power.mode.index()] = Tuning::default();
        self.queue_tuning();
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
            let cli = self.cli.tag();
            ec.spawn(move |ec| {
                if let Err(e) = battery::set(ec, mode) {
                    let text = format!("Couldn't set battery care: {}", error_text(e));
                    post(move |app| {
                        app.cli.reported(cli, &text);
                        app.set_status(&text, true, false);
                    });
                }
            });
        }
        self.refresh_power();
    }

    pub(super) fn set_usb_power(&mut self, on: bool) {
        if !self.power.available(self.demo) || !self.power.usb_supported {
            return;
        }
        self.power.usb_power = on;
        self.power.usb_chosen = true;
        self.settings_dirty = true;
        if let Some(ec) = &self.power.ec {
            ec.spawn(move |ec| {
                if let Err(e) = battery::set_usb_power(ec, on) {
                    let reason = error_text(e);
                    post(move |app| app.set_status(&format!("Couldn't change USB charging: {reason}"), true, false));
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

    // ---- Profiles -------------------------------------------------------------------------------------------------

    /// Switches the current mode to another of its profiles, keeping what's set now in the one it leaves.
    pub(super) fn select_profile(&mut self, index: usize) {
        let i = self.power.mode.index();
        if !self.power.available(self.demo) || index >= self.power.profiles[i].len() || index == self.power.active_profile[i] {
            return;
        }
        self.power.profiles[i] = self.power.profiles_now(self.power.mode);
        let profile = self.power.profiles[i][index].clone();
        self.power.tuning[i] = profile.tuning;
        self.power.curves[i] = profile.fan_curve;
        self.power.active_profile[i] = index;
        self.power.chosen = true;
        self.power.curve_error = None;
        self.settings_dirty = true;
        self.apply_power();
        self.osd.show_text(mode_glyph(self.power.mode), &format!("{}: {}", self.power.mode.name(), profile.name));
    }

    /// A new profile for the current mode, starting as a copy of what's set now; it becomes the active one.
    pub(super) fn add_profile(&mut self, name: &str) {
        let name = name.trim();
        let i = self.power.mode.index();
        if !self.power.available(self.demo) {
            return;
        }
        if name.is_empty() {
            self.set_status("Give the profile a name first.", true, false);
            return;
        }
        if self.power.profiles[i].len() >= MAX_PROFILES {
            self.set_status(&format!("A mode has at most {MAX_PROFILES} profiles: delete one first."), true, false);
            return;
        }
        if self.power.profiles[i].iter().any(|p| p.name.eq_ignore_ascii_case(name)) {
            self.set_status(&format!("{} mode already has a profile called \"{name}\".", self.power.mode.name()), true, false);
            return;
        }
        self.power.profiles[i] = self.power.profiles_now(self.power.mode);
        self.power.profiles[i].push(ModeProfile { name: name.to_string(), tuning: self.power.tuning[i], fan_curve: self.power.curves[i].clone() });
        self.power.active_profile[i] = self.power.profiles[i].len() - 1;
        self.power.chosen = true;
        self.settings_dirty = true;
        self.refresh_power();
    }

    /// Deletes the current mode's active profile and goes back to its Standard one.
    pub(super) fn delete_profile(&mut self) {
        let i = self.power.mode.index();
        let active = self.power.active_profile[i];
        if !self.power.available(self.demo) || active == 0 {
            return; // Standard stays
        }
        self.power.profiles[i].remove(active);
        let standard = self.power.profiles[i][0].clone();
        self.power.tuning[i] = standard.tuning;
        self.power.curves[i] = standard.fan_curve;
        self.power.active_profile[i] = 0;
        self.power.chosen = true;
        self.settings_dirty = true;
        self.apply_power();
    }

    // ---- The pages ------------------------------------------------------------------------------------------------

    /// What the Performance page says about whether modes, fans and charging can be set here.
    pub(super) fn power_note(&self) -> String {
        note(&self.power, self.demo)
    }

    pub(super) fn refresh_power(&self) {
        let Some(ui) = self.ui() else { return };
        let power = &self.power;
        let available = power.available(self.demo);
        let note = self.power_note();
        ui.set_perf_available(available);
        ui.set_perf_note(note.clone().into());
        ui.set_perf_mode(power.mode.index() as i32);
        ui.set_perf_turbo(power.turbo || power.mode == Mode::Turbo);
        ui.set_fan_boost(power.fan_boost);
        // Demo mode sends nothing, so it shows what would be sent.
        let limits = if self.demo {
            let tuning = power.tuning_for(power.mode).filter(|t| t.custom_limits);
            tuning.map(|t| t.limits).or(power.mode_defaults[power.mode.index()].map(|d| d.limits))
        } else {
            power.limits
        };
        ui.set_perf_limits(match limits {
            _ if !power.on_ac => "on battery".into(),
            Some(limits) if limits.pl1 > 0 => format!("CPU {} W sustained, {} W boost", limits.pl1, limits.pl2.max(limits.pl1)).into(),
            _ => "".into(),
        });
        self.refresh_tuning(&ui, available);
        let names: Vec<SharedString> = power.profiles[power.mode.index()].iter().map(|p| SharedString::from(p.name.as_str())).collect();
        if power.profile_model.iter().ne(names.iter().cloned()) {
            power.profile_model.set_vec(names);
        }
        ui.set_profile_index(power.active_profile[power.mode.index()] as i32);
        ui.set_profile_full(power.profiles[power.mode.index()].len() >= MAX_PROFILES);
        ui.set_battery_available(available);
        ui.set_battery_note(if available { "".into() } else { note.into() });
        ui.set_charge_mode(power.charge as i32);
        ui.set_usb_supported(power.usb_supported);
        ui.set_usb_power(power.usb_power);

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

    fn refresh_tuning(&self, ui: &crate::ui::AppWindow, available: bool) {
        let power = &self.power;
        let index = power.mode.index();
        let ranges = power.ranges();
        let tuning = power.tuning_for(power.mode).unwrap_or(power.tuning[index]);
        let defaults = power.mode_defaults[index];
        let shown = if tuning.custom_limits { tuning.limits } else { defaults.map(|d| d.limits).unwrap_or_default() };
        let range = ranges.unwrap_or_default();
        let gpu_range = power.ranges.filter(|r| r.gpu()).unwrap_or_default();
        let gpu = tuning.gpu_power(&gpu_range).unwrap_or_default();
        let own = power.gpu_own[index];
        // None: left to the EC, which hasn't been read yet.
        let gpu_tgp = gpu.extra.or(own).map(|extra| gpu_range.gpu_tgp + extra);
        let note = if !available {
            String::new()
        } else if ranges.is_none() {
            "The BIOS's power limit ranges couldn't be read, so the limits can't be changed.".into()
        } else if power.ranges.is_none() {
            "Control Center's ACPI driver isn't there to read the BIOS's ranges, so the limits go up to the highest mode's own and the TCC offset can't be changed.".into()
        } else {
            String::new()
        };
        ui.set_tuning(TuningView {
            available: available && ranges.is_some(),
            custom: tuning.custom_limits,
            pl_min: performance::MIN_LIMIT as i32,
            pl1_max: range.pl1 as i32,
            pl2_max: range.pl2 as i32,
            pl4_max: range.pl4 as i32,
            defaults: match defaults {
                Some(d) => format!("{} mode's own: {} W sustained, {} W boost, {} W peak.", power.mode.name(), d.limits.pl1, d.limits.pl2, d.limits.pl4).into(),
                None => "".into(),
            },
            tcc_available: available && range.tcc > 0,
            tcc_on: tuning.tcc_enabled,
            tcc_max: range.tcc as i32,
            gpu_available: available && gpu_range.gpu(),
            gpu_custom: tuning.custom_gpu && gpu_range.gpu_extra > 0,
            gpu_extra_available: gpu_range.gpu_extra > 0,
            gpu_min: gpu_range.gpu_tgp as i32,
            gpu_max: (gpu_range.gpu_tgp + gpu_range.gpu_extra) as i32,
            gpu_defaults: match own {
                Some(extra) => format!(
                    "{} mode's own: {} W, which the laptop picks. On, it can be set from {} to {} W (configurable TGP).",
                    power.mode.name(),
                    gpu_range.gpu_tgp + extra,
                    gpu_range.gpu_tgp,
                    gpu_range.gpu_tgp + gpu_range.gpu_extra
                ),
                None => format!(
                    "Off: the laptop picks it for the mode. On, it can be set from {} to {} W (configurable TGP).",
                    gpu_range.gpu_tgp,
                    gpu_range.gpu_tgp + gpu_range.gpu_extra
                ),
            }
            .into(),
            boost_available: gpu_range.dynamic_boost > 0,
            boost_on: gpu.dynamic_boost > 0,
            boost_text: match (gpu.dynamic_boost > 0, gpu_tgp) {
                (true, Some(tgp)) => format!(
                    "Gives the GPU up to {} W more when the CPU needs less, so it can draw up to {} W on the charger.",
                    gpu.dynamic_boost,
                    tgp + gpu.dynamic_boost
                ),
                (true, None) => format!("Gives the GPU up to {} W more when the CPU needs less, on the charger.", gpu.dynamic_boost),
                (false, Some(tgp)) => {
                    format!("Off: the GPU draws up to {tgp} W on the charger. On, it gets up to {} W more when the CPU needs less.", gpu_range.dynamic_boost)
                }
                (false, None) => format!("Off. On, the GPU gets up to {} W more when the CPU needs less, on the charger.", gpu_range.dynamic_boost),
            }
            .into(),
            note: note.into(),
            ..ui.get_tuning()
        });
        ui.set_tune_pl1(shown.pl1 as f32);
        ui.set_tune_pl2(shown.pl2 as f32);
        ui.set_tune_pl4(shown.pl4 as f32);
        ui.set_tune_gpu(gpu_tgp.unwrap_or(gpu_range.gpu_tgp) as f32);
        ui.set_tune_tcc(if tuning.tcc_enabled { tuning.tcc_offset } else { defaults.map_or(0, |d| d.tcc).min(range.tcc) } as f32);
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

/// This laptop's defaults, for demo mode.
fn demo_defaults(mode: Mode) -> Defaults {
    let (pl1, pl2, pl4) = match mode {
        Mode::Office => (35, 45, 120),
        Mode::Gaming => (60, 60, 165),
        Mode::Turbo => (80, 115, 165),
    };
    Defaults { limits: PowerLimits { pl1, pl2, pl4 }, tcc: 0 }
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

/// Whether to look at the EC's fan safety protection: only while this app's custom curve drives the fans.
fn safety_check_due(owned: bool, custom_applied: bool) -> bool {
    owned && custom_applied
}

/// What the Performance page says about whether modes, fans and charging can be set here; also the reason `uucc`
/// gives for not doing it.
pub(super) fn note(power: &Power, demo: bool) -> String {
    if demo {
        "Demo mode: nothing is sent to the laptop.".to_string()
    } else if let Some(reason) = &power.ec_error {
        format!("Can't reach the laptop's embedded controller: {reason}")
    } else if power.ec.is_some() && !power.owned && power.ready {
        "Control Center is running and sets these itself, so they're only shown here. To set them here, close Control Center and stop its service (GCUService).".to_string()
    } else if !power.ready {
        "Connecting to the laptop's embedded controller…".to_string()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_check_runs_only_with_custom_curve() {
        assert!(safety_check_due(true, true));
        assert!(!safety_check_due(true, false)); // the EC's own curve
        assert!(!safety_check_due(false, true)); // Control Center sets the fans
    }

    #[test]
    fn ec_commands_report_unavailable_reason() {
        let mut power = Power::new(None, false);
        let reason = "Secure Boot is on, so Linux blocks direct access to the embedded controller.";
        power.ec_error = Some(reason.into());
        match super::super::cli::refused(note(&power, false)) {
            crate::cli::protocol::Reply::Error { code, message } => {
                assert_eq!(code, 3);
                assert!(message.contains(reason), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }
}
