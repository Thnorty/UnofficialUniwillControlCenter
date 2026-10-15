//! The Monitor page, the fan readings on the Performance page and the Battery page's status: a thread reads them
//! once a second while one of those pages shows (or `uucc status` asks), and otherwise sleeps.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, ModelRc, VecModel};

use super::{App, post};
use crate::hardware::ec::Ec;
use crate::hardware::fans::{self, FanReading};
use crate::hardware::status::{self, BatteryDetails, SystemStatus};
use crate::lighting::engine::Inputs;
use crate::ui::{Detail, Metric};

const HISTORY: usize = 60;
const PERFORMANCE_PAGE: i32 = 4;
const BATTERY_PAGE: i32 = 5;
const MONITOR_PAGE: i32 = 6;
/// A reading older than this isn't shown by `uucc status` as the current one.
const CURRENT: Duration = Duration::from_secs(3);
/// How long the CPU load is measured over when reading starts again after a pause.
const LOAD_WINDOW: Duration = Duration::from_millis(500);

#[derive(Clone, Copy)]
pub(super) struct Sample {
    pub(super) taken: Instant,
    pub(super) cpu_temperature: Option<f32>,
    pub(super) cpu_load: Option<f32>,
    pub(super) gpu_temperature: Option<f32>,
    pub(super) gpu_off: bool,
    pub(super) fans: Option<(FanReading, FanReading)>,
    memory: Option<(u64, u64)>,
    pub(super) battery_percent: Option<u8>,
    pub(super) charging: bool,
    pub(super) on_ac: bool,
    battery: BatteryDetails,
}

/// What `uucc status` asks of the thread: readings until `until`, whatever page shows, the first one at once (`new`).
#[derive(Default)]
struct Asked {
    until: Option<Instant>,
    new: bool,
}

type AskedShared = Arc<(Mutex<Asked>, Condvar)>;

pub(super) struct Monitor {
    wanted: Arc<AtomicBool>,
    asked: AskedShared,
    stop: Arc<AtomicBool>,
    history: VecDeque<Sample>,
}

impl Monitor {
    pub fn new() -> Monitor {
        Monitor { wanted: Arc::new(AtomicBool::new(false)), asked: AskedShared::default(), stop: Arc::new(AtomicBool::new(false)), history: VecDeque::new() }
    }

    /// Ends the thread, which may be asleep until something wants readings.
    pub fn stop(&self) {
        let (lock, wake) = &*self.asked;
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        self.stop.store(true, Ordering::Release);
        wake.notify_one();
    }
}

impl App {
    pub(super) fn start_monitor(&self) {
        let (wanted, asked, stop) = (self.monitor.wanted.clone(), self.monitor.asked.clone(), self.monitor.stop.clone());
        let (ec, demo) = (self.power.shared_ec.clone(), self.demo);
        let deliver = |sample: Sample| post(move |app| app.monitor_sample(sample));
        let _ = std::thread::Builder::new().name("Monitor".into()).spawn(move || monitor_thread(wanted, asked, stop, ec, demo, deliver));
    }

    /// Has the monitor read now and for the next 5 s, whatever page shows; returns when it was asked.
    pub(super) fn want_readings(&self) -> Instant {
        ask(&self.monitor.asked)
    }

    /// Whether a reading taken after `t` is in: one with the CPU load, or a second one (the load needs two reads, and
    /// may not come at all).
    pub(super) fn fresh_sample_since(&self, t: Instant) -> bool {
        let newer = self.monitor.history.iter().filter(|s| s.taken > t).count();
        newer >= 2 || self.monitor.history.back().is_some_and(|s| s.taken > t && s.cpu_load.is_some())
    }

    /// The newest reading, if it's current.
    pub(super) fn latest_sample(&self) -> Option<Sample> {
        current(self.monitor.history.back().copied())
    }

    /// Reads only while a page that shows the readings is open.
    pub(super) fn update_monitor_wanted(&mut self) {
        let Some(ui) = self.ui() else { return };
        let page = ui.get_page();
        let wanted = ui.window().is_visible() && matches!(page, PERFORMANCE_PAGE | BATTERY_PAGE | MONITOR_PAGE);
        set_wanted(&self.monitor.wanted, &self.monitor.asked, wanted);
        if wanted && page == PERFORMANCE_PAGE {
            self.read_fan_defaults_if_needed();
        }
    }

    fn monitor_sample(&mut self, sample: Sample) {
        self.monitor.history.push_back(sample);
        while self.monitor.history.len() > HISTORY {
            self.monitor.history.pop_front();
        }
        self.refresh_monitor();
    }

    fn refresh_monitor(&self) {
        let Some(ui) = self.ui() else { return };
        let Some(last) = self.monitor.history.back().copied() else { return };
        let history = &self.monitor.history;
        let line = |value: &dyn Fn(&Sample) -> Option<f32>, low: f32, high: f32| history_path(history.iter().map(value), low, high);

        let cpu_temperature = Metric {
            icon: "\u{E950}".into(),
            title: "CPU temperature".into(),
            value: last.cpu_temperature.map_or("—".into(), |t| format!("{t:.0} °C")).into(),
            detail: "The hottest of the laptop's thermal sensors".into(),
            history: line(&|s| s.cpu_temperature, 30.0, 100.0).into(),
            level: last.cpu_temperature.map_or(-1.0, |t| (t - 30.0) / 70.0),
            warn: last.cpu_temperature.is_some_and(|t| t >= 90.0),
        };
        let cpu_load = Metric {
            icon: "\u{E9D9}".into(),
            title: "CPU load".into(),
            value: last.cpu_load.map_or("—".into(), |l| format!("{:.0}%", l * 100.0)).into(),
            detail: "All cores".into(),
            history: line(&|s| s.cpu_load.map(|l| l * 100.0), 0.0, 100.0).into(),
            level: last.cpu_load.unwrap_or(-1.0),
            warn: false,
        };
        let gpu_temperature = Metric {
            icon: "\u{E7F4}".into(),
            title: "GPU temperature".into(),
            value: match last.gpu_temperature {
                Some(t) => format!("{t:.0} °C"),
                None if last.gpu_off => "Off".into(),
                None => "—".into(),
            }
            .into(),
            detail: if last.gpu_temperature.is_some() {
                "NVIDIA GPU"
            } else if last.gpu_off {
                "Switched off while nothing uses it, which saves power. It isn't asked for a reading then, as that would switch it on."
            } else {
                "Not reported: no NVIDIA GPU or driver"
            }
            .into(),
            history: line(&|s| s.gpu_temperature, 30.0, 100.0).into(),
            level: last.gpu_temperature.map_or(-1.0, |t| (t - 30.0) / 70.0),
            warn: last.gpu_temperature.is_some_and(|t| t >= 87.0),
        };
        let fan = |name: &str, which: fn(&(FanReading, FanReading)) -> FanReading| {
            let reading = last.fans.as_ref().map(which);
            Metric {
                icon: "\u{E9CA}".into(),
                title: name.into(),
                value: reading.map_or("—".into(), |r| if r.rpm == 0 && r.duty == 0 { "Stopped".into() } else { format!("{} RPM", thousands(r.rpm as u64)) }).into(),
                detail: reading.map_or_else(
                    || if self.demo { "".into() } else { "Needs the laptop's embedded controller".into() },
                    |r| format!("{}% speed", r.duty),
                )
                .into(),
                history: line(&|s| s.fans.as_ref().map(|f| which(f).duty as f32), 0.0, 100.0).into(),
                level: reading.map_or(-1.0, |r| r.duty as f32 / 100.0),
                warn: false,
            }
        };
        let cpu_fan = fan("CPU fan", |f| f.0);
        let gpu_fan = fan("GPU fan", |f| f.1);
        let memory = Metric {
            icon: "\u{EEA0}".into(),
            title: "Memory".into(),
            value: last.memory.map_or("—".into(), |(used, _)| gigabytes(used)).into(),
            detail: last.memory.map_or("".into(), |(used, total)| format!("in use of {} ({:.0}%)", gigabytes(total), used as f64 * 100.0 / total.max(1) as f64)).into(),
            history: line(&|s| s.memory.map(|(used, total)| used as f32 * 100.0 / total.max(1) as f32), 0.0, 100.0).into(),
            level: last.memory.map_or(-1.0, |(used, total)| used as f32 / total.max(1) as f32),
            warn: false,
        };
        let battery = Metric {
            icon: "\u{E83F}".into(),
            title: "Battery".into(),
            value: last.battery_percent.map_or("—".into(), |p| format!("{p}%")).into(),
            detail: battery_state(&last).into(),
            history: line(&|s| s.battery_percent.map(|p| p as f32), 0.0, 100.0).into(),
            level: last.battery_percent.map_or(-1.0, |p| p as f32 / 100.0),
            warn: last.battery_percent.is_some_and(|p| p <= 15) && !last.on_ac,
        };
        let mode = Metric {
            icon: "\u{EC4A}".into(),
            title: "Performance".into(),
            value: format!("{} mode", self.power.mode.name()).into(),
            detail: ui.get_perf_limits(),
            history: "".into(),
            level: -1.0,
            warn: false,
        };
        ui.set_fan_metrics(ModelRc::new(VecModel::from(vec![cpu_fan.clone(), gpu_fan.clone()])));
        ui.set_metrics(ModelRc::new(VecModel::from(vec![cpu_temperature, cpu_load, gpu_temperature, memory, cpu_fan, gpu_fan, battery, mode])));

        // The Battery page.
        ui.set_battery_percent(last.battery_percent.map_or(-1, |p| p as i32));
        ui.set_battery_state(battery_state(&last).into());
        let details = &last.battery;
        let mut rows = Vec::new();
        if let (Some(full), Some(designed)) = (details.full, details.designed) {
            rows.push(("Health", format!("{:.0}% of its design capacity", full as f64 * 100.0 / designed as f64)));
        }
        if let Some(full) = details.full {
            rows.push(("Full charge capacity", watt_hours(full)));
        }
        if let Some(designed) = details.designed {
            rows.push(("Design capacity", watt_hours(designed)));
        }
        if let Some(remaining) = details.remaining {
            rows.push(("Remaining", watt_hours(remaining)));
        }
        if let Some(voltage) = details.voltage {
            rows.push(("Voltage", format!("{:.1} V", voltage as f64 / 1000.0)));
        }
        if let Some(cycles) = details.cycles {
            rows.push(("Charge cycles", cycles.to_string()));
        }
        let rows: Vec<Detail> = rows.into_iter().map(|(label, value)| Detail { label: label.into(), value: value.into() }).collect();
        ui.set_battery_details(ModelRc::new(VecModel::from(rows)));
    }
}

fn current(sample: Option<Sample>) -> Option<Sample> {
    sample.filter(|s| s.taken.elapsed() < CURRENT)
}

fn battery_state(sample: &Sample) -> String {
    let rate = sample.battery.rate.filter(|&r| r != 0).map(|r| format!("{:.1} W", r.unsigned_abs() as f64 / 1000.0));
    match (sample.battery_percent, sample.on_ac, sample.charging) {
        (None, _, _) => "No battery".into(),
        (_, true, true) => format!("Charging{}", rate.map_or(String::new(), |r| format!(" at {r}"))),
        (_, true, false) => "Plugged in, not charging".into(),
        (_, false, _) => {
            let left = match (sample.battery.remaining, sample.battery.rate) {
                (Some(remaining), Some(rate)) if rate < 0 => {
                    let minutes = remaining as u64 * 60 / rate.unsigned_abs() as u64;
                    format!(" · about {} h {:02} min left", minutes / 60, minutes % 60)
                }
                _ => String::new(),
            };
            format!("On battery{}{left}", rate.map_or(String::new(), |r| format!(", using {r}")))
        }
    }
}

fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
}

fn watt_hours(milliwatt_hours: u32) -> String {
    format!("{:.1} Wh", milliwatt_hours as f64 / 1000.0)
}

/// A reading's last minute as path commands in a 1000 × 1000 view box, the newest at the right.
fn history_path(values: impl ExactSizeIterator<Item = Option<f32>>, low: f32, high: f32) -> String {
    let count = values.len();
    let mut path = String::new();
    let mut drawing = false;
    for (i, value) in values.enumerate() {
        let Some(value) = value else {
            drawing = false;
            continue;
        };
        let slot = HISTORY - count + i;
        let x = (slot as f32 * 1000.0 / (HISTORY - 1) as f32).round();
        let y = (950.0 - ((value - low) / (high - low)).clamp(0.0, 1.0) * 900.0).round();
        path += &format!("{}{} {} ", if drawing { "L " } else { "M " }, x, y);
        drawing = true;
    }
    path
}

/// Turns the readings for a page on or off.
fn set_wanted(wanted: &AtomicBool, asked: &AskedShared, on: bool) {
    // Under the thread's lock, so it can't miss the change between looking and going to sleep.
    let (lock, wake) = &**asked;
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    wanted.store(on, Ordering::Release);
    wake.notify_one();
}

fn ask(asked: &AskedShared) -> Instant {
    let now = Instant::now();
    let (lock, wake) = &**asked;
    let mut asked = lock.lock().unwrap_or_else(|e| e.into_inner());
    asked.until = Some(now + Duration::from_secs(5));
    asked.new = true;
    wake.notify_one();
    now
}

fn monitor_thread(wanted: Arc<AtomicBool>, asked: AskedShared, stop: Arc<AtomicBool>, ec: Arc<Mutex<Option<Ec>>>, demo: bool, deliver: impl Fn(Sample)) {
    let mut status = SystemStatus::default();
    let mut battery_reader = status::BatteryReader::default();
    let mut battery = BatteryDetails::default();
    let mut battery_read: Option<Instant> = None;
    let started = Instant::now();
    let mut last_read: Option<Instant> = None;
    loop {
        // While something wants readings, a second between them (one asked for comes at once); otherwise asleep
        // until a page or `uucc status` wants them, so an idle app doesn't wake.
        let asked = {
            let (lock, wake) = &*asked;
            let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
            let stopping = || stop.load(Ordering::Acquire);
            let reading = wanted.load(Ordering::Acquire) || guard.until.is_some_and(|until| Instant::now() < until);
            let mut asked = if reading {
                wake.wait_timeout_while(guard, Duration::from_secs(1), |a| !a.new && !stopping()).unwrap_or_else(|e| e.into_inner()).0
            } else {
                wake.wait_while(guard, |a| !a.new && !stopping() && !wanted.load(Ordering::Acquire)).unwrap_or_else(|e| e.into_inner())
            };
            asked.new = false;
            asked.until.is_some_and(|until| Instant::now() < until)
        };
        if stop.load(Ordering::Acquire) {
            break;
        }
        if !wanted.load(Ordering::Acquire) && !asked {
            continue;
        }
        let sample = if demo {
            demo_sample(started.elapsed().as_secs_f32())
        } else {
            // The CPU load is the use since the read before: after a pause that's a long average, so it's measured
            // anew.
            if last_read.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
                status.update(Inputs::CPU_LOAD);
                std::thread::sleep(LOAD_WINDOW);
            }
            last_read = Some(Instant::now());
            status.update(Inputs::STATUS);
            let ec = ec.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let fans = ec.and_then(|ec| ec.run_for(Duration::from_secs(2), fans::readings).ok());
            if battery_read.is_none_or(|t| t.elapsed() >= Duration::from_secs(5)) {
                battery = battery_reader.read();
                battery_read = Some(Instant::now());
            }
            Sample {
                taken: Instant::now(),
                cpu_temperature: status.cpu_temperature,
                cpu_load: status.cpu_load,
                gpu_temperature: status.gpu_temperature,
                gpu_off: status.gpu_off,
                fans,
                memory: status::memory(),
                battery_percent: status.battery_percent,
                charging: status.charging,
                on_ac: status.on_ac,
                battery,
            }
        };
        deliver(sample);
    }
}

/// Made-up readings for trying the page in demo mode.
fn demo_sample(t: f32) -> Sample {
    let wave = |period: f32, phase: f32| ((t / period + phase) * std::f32::consts::TAU).sin();
    let temperature = 62.0 + 14.0 * wave(40.0, 0.0) + 3.0 * wave(7.0, 0.3);
    let duty = (((temperature - 50.0) * 2.2).clamp(0.0, 100.0)) as u8;
    let fan = |duty: u8| FanReading { duty, rpm: if duty == 0 { 0 } else { 1800 + duty as u16 * 32 } };
    Sample {
        taken: Instant::now(),
        cpu_temperature: Some(temperature),
        cpu_load: Some((0.35 + 0.25 * wave(23.0, 0.1) + 0.1 * wave(3.0, 0.0)).clamp(0.0, 1.0)),
        gpu_temperature: Some(temperature - 8.0),
        gpu_off: false,
        fans: Some((fan(duty), fan(duty.saturating_sub(10)))),
        memory: Some((11_400_000_000, 34_100_000_000)),
        battery_percent: Some(82),
        charging: true,
        on_ac: true,
        battery: BatteryDetails { designed: Some(62_000), full: Some(57_300), remaining: Some(47_000), rate: Some(24_800), voltage: Some(12_430), cycles: Some(143) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(age: Duration) -> Sample {
        Sample {
            taken: Instant::now() - age,
            cpu_temperature: Some(50.0),
            cpu_load: None,
            gpu_temperature: None,
            gpu_off: false,
            fans: None,
            memory: None,
            battery_percent: None,
            charging: false,
            on_ac: false,
            battery: BatteryDetails::default(),
        }
    }

    #[test]
    fn asked_readings_come_at_once() {
        let monitor = Monitor::new();
        let (tx, rx) = std::sync::mpsc::channel();
        let (wanted, asked, stop) = (monitor.wanted.clone(), monitor.asked.clone(), monitor.stop.clone());
        let thread = std::thread::spawn(move || {
            // The demo's readings: the real ones (WMI, NVML) crash the test process when other tests run alongside.
            monitor_thread(wanted, asked, stop, Arc::new(Mutex::new(None)), true, move |sample| {
                let _ = tx.send(sample);
            })
        });
        std::thread::sleep(Duration::from_millis(1500)); // no page shows the readings meanwhile
        let t = ask(&monitor.asked);
        let sample = rx.recv_timeout(Duration::from_millis(300));
        // Ended before the test does: a thread still in WMI when the process exits can crash it.
        monitor.stop();
        thread.join().unwrap();
        let sample = sample.expect("no reading within 0.3 s of asking");
        assert!(sample.taken > t);
    }

    /// The monitor thread with the demo's readings: the real ones (WMI, NVML) crash the test process when other tests
    /// run alongside.
    fn demo_thread(monitor: &Monitor) -> (std::thread::JoinHandle<()>, std::sync::mpsc::Receiver<Sample>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (wanted, asked, stop) = (monitor.wanted.clone(), monitor.asked.clone(), monitor.stop.clone());
        let thread = std::thread::spawn(move || {
            monitor_thread(wanted, asked, stop, Arc::new(Mutex::new(None)), true, move |sample| {
                let _ = tx.send(sample);
            })
        });
        (thread, rx)
    }

    #[test]
    fn an_opened_page_gets_a_reading_at_once() {
        let monitor = Monitor::new();
        let (thread, rx) = demo_thread(&monitor);
        std::thread::sleep(Duration::from_millis(1500)); // no page shows the readings meanwhile
        let t = Instant::now();
        set_wanted(&monitor.wanted, &monitor.asked, true);
        let sample = rx.recv_timeout(Duration::from_millis(300));
        monitor.stop();
        thread.join().unwrap();
        assert!(sample.expect("no reading within 0.3 s of opening the page").taken > t);
    }

    #[test]
    fn stopping_ends_an_idle_thread_at_once() {
        let monitor = Monitor::new();
        let (thread, _rx) = demo_thread(&monitor);
        std::thread::sleep(Duration::from_millis(1500));
        let (done, ended) = std::sync::mpsc::channel();
        monitor.stop();
        std::thread::spawn(move || {
            thread.join().unwrap();
            let _ = done.send(());
        });
        assert!(ended.recv_timeout(Duration::from_millis(300)).is_ok(), "the thread didn't end within 0.3 s");
    }

    #[test]
    fn old_readings_are_not_current() {
        assert!(current(Some(sample(Duration::from_millis(500)))).is_some());
        assert!(current(Some(sample(Duration::from_secs(60)))).is_none());
        assert!(current(None).is_none());
    }
}
