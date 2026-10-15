//! The app's answers to `uucc`. Each request runs on the window's thread and calls the same methods the window's
//! callbacks call, so the window, tray and pop-ups follow as if clicked.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use super::{App, Lighting, post};
use crate::cli::protocol::{
    ChargeResult, Device, DisplayResult, EffectInfo, EffectsResult, Envelope, FanBoostResult, GpuModeResult, LightingChanges, LightingResult,
    FanResult, ModeResult, ProfileResult, Reply, Request, SetupResult, StatusResult, resolve_kind,
};
use crate::color::Rgb;
use crate::lighting::device::Kind;
use crate::lighting::effects::EffectColors;
use crate::hardware::battery::ChargeMode;
use crate::hardware::ec::Ec;
use crate::hardware::fans::FanReading;
use crate::hardware::performance::Mode;
use crate::hardware::uefi::GpuMode;

/// How long a request may wait for the window's thread.
const UI_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the EC's queued writes and a BIOS write may take before the reply.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long `status` waits for a fresh reading.
const READING_TIMEOUT: Duration = Duration::from_secs(5);

const MODE_WORDS: [&str; 3] = ["office", "gaming", "turbo"];
const CHARGE_WORDS: [&str; 3] = ["full", "balanced", "long-life"];
const GPU_WORDS: [&str; 3] = ["hybrid", "intel", "nvidia"];

/// What a command did besides its reply: work queued for the EC, whose errors arrive later.
pub(super) struct Done {
    reply: Reply,
    ec: Option<Ec>,
}

/// Runs `f` with the app on the window's thread and waits for its result; `None` if the thread didn't get to it.
fn on_ui<T: Send + 'static>(f: impl FnOnce(&mut App) -> T + Send + 'static) -> Option<T> {
    call_claimed(|job| post(job), f, UI_TIMEOUT)
}

/// Runs `f` through `post` and waits up to `timeout` for its result. If it hasn't started by then, it never runs: uucc
/// has been told the app didn't answer, and doing it later would surprise whoever ran it.
fn call_claimed<C: 'static, T: Send + 'static>(
    post: impl FnOnce(Box<dyn FnOnce(&mut C) + Send>),
    f: impl FnOnce(&mut C) -> T + Send + 'static,
    timeout: Duration,
) -> Option<T> {
    let claimed = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let started = claimed.clone();
    post(Box::new(move |c| {
        if started.swap(true, Ordering::AcqRel) {
            return; // given up on
        }
        let _ = tx.send(f(c));
    }));
    match rx.recv_timeout(timeout) {
        Ok(result) => Some(result),
        // It started just as the time ran out: it's running, so its result comes.
        Err(_) if claimed.swap(true, Ordering::AcqRel) => rx.recv_timeout(timeout).ok(),
        Err(_) => None,
    }
}

/// Which errors are a uucc command's: those shown while it runs, and those from work it set off (power, battery care,
/// BIOS writes), until its reply is made. Not those something else shows meanwhile.
#[derive(Default)]
pub(super) struct CliCapture {
    running: Cell<bool>,
    waiting: Cell<Option<u64>>,
    last: Cell<u64>,
    error: RefCell<Option<String>>,
}

impl CliCapture {
    /// A command starts.
    fn begin(&self) {
        let id = self.last.get() + 1;
        self.last.set(id);
        self.waiting.set(Some(id));
        self.running.set(true);
        *self.error.borrow_mut() = None;
    }

    /// It has returned; the work it set off may still report.
    fn returned(&self) {
        self.running.set(false);
    }

    /// For work set off now: the command it's for, if one is running.
    pub(super) fn tag(&self) -> Option<u64> {
        if self.running.get() { self.waiting.get() } else { None }
    }

    /// An error the app shows now.
    pub(super) fn shown(&self, text: &str) {
        if self.running.get() {
            self.keep(text);
        }
    }

    /// An error from work set off for `tag`.
    pub(super) fn reported(&self, tag: Option<u64>, text: &str) {
        if tag.is_some() && tag == self.waiting.get() {
            self.keep(text);
        }
    }

    /// The first error: what went wrong; any after it follow from it.
    fn keep(&self, text: &str) {
        self.error.borrow_mut().get_or_insert_with(|| text.to_string());
    }

    /// The reply is being made: the command's error, if any.
    fn finish(&self) -> Option<String> {
        self.running.set(false);
        self.waiting.set(None);
        self.error.borrow_mut().take()
    }
}

/// Why uucc can't reach the app, for whoever looks (uucc names the file); a stale note goes once the pipe is there.
pub(super) fn log_pipe(result: &anyhow::Result<()>) {
    let Some(path) = std::env::var_os("APPDATA").map(|a| std::path::PathBuf::from(a).join(crate::settings::FOLDER).join("uucc.log")) else {
        return;
    };
    match result {
        Ok(()) => {
            let _ = std::fs::remove_file(path);
        }
        Err(e) => {
            let since_1970 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let _ = std::fs::write(path, format!("{since_1970} (Unix time): couldn't open the connection for uucc: {e:#}\n"));
        }
    }
}

fn no_answer() -> Reply {
    Reply::Error { code: 3, message: "The app didn't answer.".into() }
}

/// The pipe server's handler (on its thread): runs the request, waits for what it set off, and turns any error
/// message the app showed meanwhile into the reply.
pub(super) fn handle(envelope: Envelope) -> Reply {
    let request = envelope.request;
    if matches!(request, Request::Status) {
        // The monitor reads only while a page shows its readings; have it read now.
        if let Some(asked) = on_ui(|app| app.want_readings()) {
            let deadline = Instant::now() + READING_TIMEOUT;
            while !on_ui(move |app| app.fresh_sample_since(asked)).unwrap_or(true) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
    let Some(done) = on_ui(move |app| {
        app.cli.begin();
        let done = app.run_cli(request);
        app.send_pending_lighting();
        app.cli.returned();
        done
    }) else {
        return no_answer();
    };
    // The EC thread runs its jobs in order, so this one runs after the command's writes; their results are posted
    // to the window's thread before the next `on_ui`.
    if let Some(ec) = &done.ec {
        let _ = ec.run_for(SETTLE_TIMEOUT, |_| Ok(()));
    }
    // A BIOS write runs on a thread of its own.
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    while on_ui(|app| app.bios.busy).unwrap_or(false) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let Some(error) = on_ui(|app| app.cli.finish()) else {
        return no_answer();
    };
    match error {
        Some(message) => Reply::Error { code: 3, message },
        None => done.reply,
    }
}

fn ok(result: impl Serialize) -> Result<Value, Reply> {
    serde_json::to_value(result).map_err(|e| Reply::Error { code: 3, message: e.to_string() })
}

fn bad(message: String) -> Reply {
    Reply::Error { code: 1, message }
}

fn refused(message: impl Into<String>) -> Reply {
    Reply::Error { code: 3, message: message.into() }
}

impl App {
    pub(super) fn run_cli(&mut self, request: Request) -> Done {
        let mut ec = None;
        let result = self.cli_command(request, &mut ec);
        Done { reply: result.map(Reply::Ok).unwrap_or_else(|error| error), ec }
    }

    /// The EC, when this app sets the mode, fans and charging (for waiting on its writes).
    fn owned_ec(&self) -> Option<Ec> {
        self.power.ec.clone().filter(|_| self.power.available(self.demo) && !self.demo)
    }

    fn require_power(&self) -> Result<(), Reply> {
        if self.power.available(self.demo) { Ok(()) } else { Err(refused(self.power_note())) }
    }

    fn cli_command(&mut self, request: Request, ec: &mut Option<Ec>) -> Result<Value, Reply> {
        match request {
            Request::Mode(value) => {
                if let Some(value) = value {
                    let mode = Mode::from_index(resolve_kind("mode", &value, &MODE_WORDS).map_err(bad)?);
                    self.require_power()?;
                    if mode == Mode::Turbo && !self.power.turbo {
                        return Err(refused("This laptop has no Turbo mode."));
                    }
                    self.set_mode(mode);
                    *ec = self.owned_ec();
                }
                let available = Mode::ALL.iter().filter(|m| **m != Mode::Turbo || self.power.turbo).map(|m| m.name().to_string()).collect();
                ok(ModeResult { mode: self.power.mode.name().into(), available })
            }
            Request::FanBoost(value) => {
                if let Some(on) = value {
                    self.require_power()?;
                    self.set_fan_boost(on);
                    *ec = self.owned_ec();
                }
                ok(FanBoostResult { on: self.power.fan_boost })
            }
            Request::Profile(value) => {
                let i = self.power.mode.index();
                if let Some(value) = value {
                    let names: Vec<&str> = self.power.profiles[i].iter().map(|p| p.name.as_str()).collect();
                    let index = resolve_kind("profile", &value, &names).map_err(bad)?;
                    self.require_power()?;
                    self.select_profile(index);
                    *ec = self.owned_ec();
                }
                let profiles: Vec<String> = self.power.profiles[i].iter().map(|p| p.name.clone()).collect();
                let active = profiles.get(self.power.active_profile[i]).cloned().unwrap_or_default();
                ok(ProfileResult { mode: self.power.mode.name().into(), profiles, active })
            }
            Request::Charge(value) => {
                if let Some(value) = value {
                    let index = resolve_kind("charge mode", &value, &CHARGE_WORDS).map_err(bad)?;
                    self.require_power()?;
                    self.set_charge_mode(ChargeMode::from_index(index));
                    *ec = self.owned_ec();
                }
                ok(ChargeResult { charge: CHARGE_WORDS[self.power.charge as usize].into() })
            }
            Request::Setup(value) => {
                let names: Vec<String> = self.settings.setups.iter().map(|s| s.name.clone()).collect();
                let mut applied = None;
                if let Some(value) = value {
                    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                    let index = resolve_kind("setup", &value, &refs).map_err(bad)?;
                    if self.settings.setups[index].mode.is_some() {
                        self.require_power()?;
                        *ec = self.owned_ec();
                    }
                    self.apply_setup(index);
                    applied = Some(names[index].clone());
                }
                ok(SetupResult { setups: names, applied })
            }
            Request::Display { on, mode } => {
                let names = App::display_mode_names();
                match (on, mode) {
                    (Some(false), Some(_)) => return Err(bad("--mode turns the display colors on; drop off".into())),
                    (_, Some(mode)) => {
                        let index = resolve_kind("display mode", &mode, &names).map_err(bad)?;
                        self.set_display_mode(index);
                    }
                    (Some(on), None) => self.set_display_on(on),
                    (None, None) => {}
                }
                let modes = names.iter().map(|n| n.to_string()).collect();
                ok(DisplayResult { on: self.settings.display.on, mode: names[self.display_mode()].into(), modes })
            }
            Request::GpuMode(value) => {
                let Some(current) = self.gpu_mode() else {
                    return Err(refused("This laptop has no GPU mode switch, or its BIOS settings can't be read."));
                };
                let mut chosen = current;
                if let Some(value) = value {
                    chosen = GpuMode::ALL[resolve_kind("GPU mode", &value, &GPU_WORDS).map_err(bad)?];
                    if chosen != current {
                        self.bios_writable().map_err(refused)?;
                        self.set_gpu_mode(GpuMode::ALL.iter().position(|m| *m == chosen).unwrap_or(0));
                    }
                }
                let word = GPU_WORDS[GpuMode::ALL.iter().position(|m| *m == chosen).unwrap_or(0)];
                let restart_needed = self.bios.gpu_mode_started.is_some_and(|started| started != chosen);
                ok(GpuModeResult { mode: word.into(), restart_needed })
            }
            Request::Lighting { device, changes } => {
                let kind = match device {
                    Device::Keyboard => Kind::Keyboard,
                    Device::LightBar => Kind::LightBar,
                };
                self.cli_lighting(kind, changes)?;
                ok(self.lighting_result(kind))
            }
            Request::Effects => ok(EffectsResult { keyboard: effect_infos(&self.keyboard), light_bar: self.light_bar.as_ref().map(effect_infos) }),
            Request::Status => ok(self.status_result()),
        }
    }

    fn status_result(&self) -> StatusResult {
        let sample = self.latest_sample();
        let fan = |which: fn(&(FanReading, FanReading)) -> FanReading| {
            sample.and_then(|s| s.fans).map(|f| which(&f)).map(|r| FanResult { rpm: r.rpm, duty: r.duty })
        };
        // Without a reading (none came in time), the battery is read here: it's quick.
        let (battery_percent, charging, on_ac) = match sample {
            Some(s) => (s.battery_percent, s.charging, s.on_ac),
            None => crate::hardware::status::power_now(),
        };
        let i = self.power.mode.index();
        let names = App::display_mode_names();
        StatusResult {
            mode: self.power.mode.name().into(),
            fan_boost: self.power.fan_boost,
            profile: self.power.profiles[i].get(self.power.active_profile[i]).map(|p| p.name.clone()).unwrap_or_default(),
            cpu_temperature: sample.and_then(|s| s.cpu_temperature),
            cpu_load: sample.and_then(|s| s.cpu_load),
            gpu_temperature: sample.and_then(|s| s.gpu_temperature),
            gpu_off: sample.is_some_and(|s| s.gpu_off),
            cpu_fan: fan(|f| f.0),
            gpu_fan: fan(|f| f.1),
            battery_percent,
            charging,
            on_ac,
            charge: CHARGE_WORDS[self.power.charge as usize].into(),
            keyboard: self.lighting_result(Kind::Keyboard).expect("the keyboard is always there"),
            light_bar: self.lighting_result(Kind::LightBar),
            display: DisplayResult {
                on: self.settings.display.on,
                mode: names[self.display_mode()].into(),
                modes: names.iter().map(|n| n.to_string()).collect(),
            },
            gpu_mode: self.gpu_mode().map(|m| GPU_WORDS[GpuMode::ALL.iter().position(|g| *g == m).unwrap_or(0)].into()),
        }
    }

    /// Applies `changes` in the order effect, colors, brightness, speed, on/off, linked; the first that fails stops
    /// the rest.
    fn cli_lighting(&mut self, kind: Kind, changes: LightingChanges) -> Result<(), Reply> {
        let Some(lighting) = self.lighting(kind) else { return Err(refused("This laptop has no light bar.")) };
        if lighting.device.is_none() && !self.demo {
            let name = if kind == Kind::Keyboard { "keyboard" } else { "light bar" };
            return Err(refused(format!("The {name} can't be reached: it wasn't found, or sending to it failed. The app's window says more.")));
        }
        if let Some(wanted) = &changes.effect {
            let keys: Vec<&str> = lighting.effects.iter().map(|e| e.key).collect();
            let names: Vec<&str> = lighting.effects.iter().map(|e| e.name).collect();
            let index = resolve_kind("effect", wanted, &keys).or_else(|e| resolve_kind("effect", wanted, &names).map_err(|_| e)).map_err(bad)?;
            self.select_effect(kind, index);
        }
        if let Some(colors) = &changes.colors {
            let colors: Vec<Rgb> = colors.iter().map(|c| Rgb::parse_hex(c).ok_or_else(|| bad(format!("color \"{c}\": give #rrggbb")))).collect::<Result<_, _>>()?;
            self.cli_colors(kind, &colors)?;
        }
        if let Some(percent) = changes.brightness {
            self.set_light(kind, percent.min(100));
        }
        if let Some(speed) = changes.speed {
            let lighting = self.lighting(kind).expect("checked above");
            let effect = &lighting.effects[lighting.effect];
            if !effect.has_speed {
                return Err(bad(format!("{} has no speed", effect.name)));
            }
            self.set_speed(kind, speed.min(4));
        }
        if let Some(on) = changes.on {
            match kind {
                Kind::LightBar => self.set_light_bar_on(on),
                // The keyboard has no switch of its own: off is brightness 0, as with the Fn keys.
                Kind::Keyboard => {
                    let percent = self.keyboard.percent;
                    if !on && percent > 0 {
                        self.keyboard_before_off = Some(percent);
                        self.set_light(kind, 0);
                    } else if on && percent == 0 && changes.brightness.is_none() {
                        let back = self.keyboard_before_off.take().unwrap_or(100);
                        self.set_light(kind, back);
                    }
                }
            }
        }
        if let Some(linked) = changes.linked {
            if kind != Kind::LightBar {
                return Err(bad("--linked is for the light bar".into()));
            }
            self.set_linked(linked);
        }
        Ok(())
    }

    /// Sends lighting changes now instead of after the window's short pause, so a failure is in the reply. The
    /// keyboard goes first: a light bar that follows it is queued again by it.
    fn send_pending_lighting(&mut self) {
        for kind in [Kind::Keyboard, Kind::LightBar] {
            if self.lighting(kind).is_some_and(|l| l.pending) {
                self.apply(kind, false, None);
            }
        }
    }

    /// The effect's colors, as the window edits them: one or four zone colors, or the palette.
    fn cli_colors(&mut self, kind: Kind, colors: &[Rgb]) -> Result<(), Reply> {
        let lighting = self.lighting(kind).expect("checked by the caller");
        let effect = &lighting.effects[lighting.effect];
        match effect.colors {
            EffectColors::None => Err(bad(format!("{} has no colors", effect.name))),
            EffectColors::Zones => match colors {
                [one] => {
                    self.edit_color(kind, 2, 0, *one);
                    Ok(())
                }
                [_, _, _, _] => {
                    for (i, color) in colors.iter().enumerate() {
                        self.edit_color(kind, 0, i, *color);
                    }
                    Ok(())
                }
                _ => Err(bad(format!("{} takes 1 color (all zones) or 4 (one per zone)", effect.name))),
            },
            EffectColors::Palette => {
                let (min, max, name) = (effect.min_colors, effect.max_colors, effect.name);
                if !(min..=max).contains(&colors.len()) {
                    return Err(bad(format!("{name} takes {min} to {max} colors")));
                }
                let effect_index = lighting.effect;
                while self.lighting(kind).is_some_and(|l| l.palettes[effect_index].len() < colors.len()) {
                    self.add_color(kind);
                }
                while let Some(len) = self.lighting(kind).map(|l| l.palettes[effect_index].len()).filter(|len| *len > colors.len()) {
                    self.remove_color(kind, len - 1);
                }
                for (i, color) in colors.iter().enumerate() {
                    self.edit_color(kind, 1, i, *color);
                }
                Ok(())
            }
        }
    }

    pub(super) fn lighting_result(&self, kind: Kind) -> Option<LightingResult> {
        let lighting = self.lighting(kind)?;
        let effect = &lighting.effects[lighting.effect];
        let colors: Vec<Rgb> = match effect.colors {
            EffectColors::None => Vec::new(),
            EffectColors::Zones => lighting.zones.to_vec(),
            EffectColors::Palette => lighting.palettes[lighting.effect].clone(),
        };
        Some(LightingResult {
            device: if kind == Kind::Keyboard { Device::Keyboard } else { Device::LightBar },
            on: if kind == Kind::Keyboard { lighting.percent > 0 } else { lighting.on },
            effect: effect.key.into(),
            effect_name: effect.name.into(),
            brightness: lighting.percent,
            speed: effect.has_speed.then_some(lighting.speed),
            colors: colors.iter().map(|c| c.hex().to_lowercase()).collect(),
            linked: (kind == Kind::LightBar).then_some(lighting.linked),
        })
    }
}

fn effect_infos(lighting: &Lighting) -> Vec<EffectInfo> {
    lighting
        .effects
        .iter()
        .map(|e| {
            // Zone colors: one for all four zones, or one each.
            let color_counts: Vec<usize> = match e.colors {
                EffectColors::None => Vec::new(),
                EffectColors::Zones => vec![1, 4],
                EffectColors::Palette => (e.min_colors..=e.max_colors).collect(),
            };
            let (min_colors, max_colors) = (color_counts.first().copied().unwrap_or(0), color_counts.last().copied().unwrap_or(0));
            EffectInfo { id: e.key.into(), name: e.name.into(), min_colors, max_colors, color_counts, has_speed: e.has_speed }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type Parked = Arc<Mutex<Option<Box<dyn FnOnce(&mut i32) + Send>>>>;

    #[test]
    fn a_call_given_up_on_never_runs() {
        let parked: Parked = Arc::default();
        let park = parked.clone();
        let result = call_claimed(move |job| *park.lock().unwrap() = Some(job), |n: &mut i32| *n += 1, Duration::from_millis(50));
        assert_eq!(result, None);
        let mut n = 0;
        let job = parked.lock().unwrap().take().unwrap();
        job(&mut n); // the window's thread gets to it after all
        assert_eq!(n, 0, "it ran after uucc was told the app didn't answer");
    }

    #[test]
    fn a_call_in_time_answers() {
        let result = call_claimed(
            |job| {
                let mut n = 41;
                job(&mut n);
            },
            |n: &mut i32| *n + 1,
            Duration::from_millis(50),
        );
        assert_eq!(result, Some(42));
    }

    #[test]
    fn an_error_while_it_runs_is_its_reply() {
        let capture = CliCapture::default();
        capture.begin();
        capture.shown("Sending to the keyboard failed");
        capture.returned();
        assert_eq!(capture.finish().as_deref(), Some("Sending to the keyboard failed"));
    }

    #[test]
    fn an_error_from_something_else_isnt_the_commands() {
        let capture = CliCapture::default();
        capture.begin();
        capture.returned();
        capture.shown("Couldn't save the settings"); // a click in the window meanwhile
        assert_eq!(capture.finish(), None);
    }

    #[test]
    fn an_error_from_the_commands_work_is_its_reply() {
        let capture = CliCapture::default();
        capture.begin();
        let tag = capture.tag();
        capture.returned();
        capture.reported(tag, "Couldn't set battery care");
        assert_eq!(capture.finish().as_deref(), Some("Couldn't set battery care"));
        // The next command doesn't get this one's late errors.
        capture.begin();
        capture.returned();
        capture.reported(tag, "Couldn't set battery care");
        assert_eq!(capture.finish(), None);
    }
}
