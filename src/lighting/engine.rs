//! Runs the effects this app draws (Music, Spectrum, Star Spark, Typing, status meters, …) on one background
//! thread, and gathers the audio, key and system inputs they need. Each device has at most one effect running.

use std::collections::HashMap;
use std::ops::BitOr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use crate::lighting::audio::{AudioMeter, LoopbackCapture, SampleHistory};
use crate::lighting::device::Kind;
use crate::hardware::status::SystemStatus;

/// What an effect reads each frame; the engine only gathers what some effect needs.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Inputs(pub u32);

impl Inputs {
    pub const NONE: Inputs = Inputs(0);
    pub const LOUDNESS: Inputs = Inputs(1);
    pub const SPECTRUM: Inputs = Inputs(2);
    pub const KEYS: Inputs = Inputs(4);
    pub const BATTERY: Inputs = Inputs(8);
    pub const CPU_LOAD: Inputs = Inputs(16);
    pub const CPU_TEMPERATURE: Inputs = Inputs(32);
    pub const GPU_TEMPERATURE: Inputs = Inputs(64);
    pub const STATUS: Inputs = Inputs(8 | 16 | 32 | 64);

    pub fn contains(self, other: Inputs) -> bool {
        self.0 & other.0 == other.0 && other.0 != 0
    }

    pub fn intersects(self, other: Inputs) -> bool {
        self.0 & other.0 != 0
    }
}

impl BitOr for Inputs {
    type Output = Inputs;
    fn bitor(self, rhs: Inputs) -> Inputs {
        Inputs(self.0 | rhs.0)
    }
}

/// An effect running on a device, ticked about 40 times a second.
pub trait Target: Send {
    fn inputs(&self) -> Inputs;

    fn tick(&mut self, frame: &Frame) -> anyhow::Result<()>;

    /// Whether it changes the device's brightness (the keyboard's Music), so a changed reading isn't the Fn keys.
    fn drives_brightness(&self) -> bool {
        false
    }
}

/// The inputs of one engine tick.
pub struct Frame {
    /// Seconds since the engine started.
    pub time: f64,
    /// Seconds since the previous tick.
    pub delta: f32,
    /// How loud the output is, 0-1, scaled to the loudest recent sound.
    pub loudness: f32,
    pub spectrum: AudioSpectrum,
    /// Keys pressed since the previous tick, as positions across the keyboard from 0 (left) to 1 (right).
    pub keys: Vec<f32>,
    pub status: SystemStatus,
}

struct Running {
    id: u64,
    target: Box<dyn Target>,
}

struct Shared {
    targets: Mutex<HashMap<Kind, Running>>,
    wake: Mutex<bool>,
    wake_signal: Condvar,
    keys: Mutex<Vec<f32>>,
    inputs_in_use: AtomicU32,
    stopping: AtomicBool,
    next_id: AtomicU64,
}

impl Shared {
    fn targets(&self) -> std::sync::MutexGuard<'_, HashMap<Kind, Running>> {
        self.targets.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn update_inputs(&self, targets: &HashMap<Kind, Running>) {
        let all = targets.values().fold(Inputs::NONE, |all, r| all | r.target.inputs());
        self.inputs_in_use.store(all.0, Ordering::Release);
    }
}

pub struct Engine {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

const TICK: Duration = Duration::from_millis(25);
const SPECTRUM_SIZE: usize = 2048;

impl Engine {
    pub fn new() -> Engine {
        let shared = Arc::new(Shared {
            targets: Mutex::new(HashMap::new()),
            wake: Mutex::new(false),
            wake_signal: Condvar::new(),
            keys: Mutex::new(Vec::new()),
            inputs_in_use: AtomicU32::new(0),
            stopping: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
        });
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new().name("Lighting effects".into()).spawn(move || run(thread_shared)).expect("effects thread");
        Engine { shared, thread: Some(thread) }
    }

    /// Starts `target` on `owner`, replacing what ran there before. Returns an ID to recognise it by.
    pub fn play(&self, owner: Kind, target: Box<dyn Target>) -> u64 {
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut targets = self.shared.targets();
            targets.insert(owner, Running { id, target });
            self.shared.update_inputs(&targets);
        }
        *self.shared.wake.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.shared.wake_signal.notify_one();
        id
    }

    /// Stops `owner`'s target. Once this returns, it won't be ticked again.
    pub fn pause(&self, owner: Kind) {
        let mut targets = self.shared.targets();
        targets.remove(&owner);
        self.shared.update_inputs(&targets);
    }

    /// The ID of the target `owner` is running, if any.
    pub fn current(&self, owner: Kind) -> Option<u64> {
        self.shared.targets().get(&owner).map(|r| r.id)
    }

    pub fn drives_brightness(&self, owner: Kind) -> bool {
        self.shared.targets().get(&owner).is_some_and(|r| r.target.drives_brightness())
    }

    /// Whether a running effect reacts to typing, so the keyboard hook is worth having.
    pub fn needs_keys(&self) -> bool {
        Inputs(self.shared.inputs_in_use.load(Ordering::Acquire)).contains(Inputs::KEYS)
    }

    /// Reports a key press at a position across the keyboard (0 = left, 1 = right).
    pub fn key_pressed(&self, position: f32) {
        if self.needs_keys() {
            self.shared.keys.lock().unwrap_or_else(|e| e.into_inner()).push(position);
        }
    }

    pub fn shutdown(&mut self) {
        {
            let mut targets = self.shared.targets();
            targets.clear();
            self.shared.update_inputs(&targets);
        }
        self.shared.stopping.store(true, Ordering::Release);
        *self.shared.wake.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.shared.wake_signal.notify_one();
        if let Some(thread) = self.thread.take() {
            // Don't hang the exit on a stuck audio or WMI call.
            let deadline = Instant::now() + Duration::from_secs(1);
            while !thread.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

fn run(shared: Arc<Shared>) {
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let mut frame = Frame {
        time: 0.0,
        delta: 0.0,
        loudness: 0.0,
        spectrum: AudioSpectrum::default(),
        keys: Vec::new(),
        status: SystemStatus::default(),
    };
    let mut audio = AudioInputs::default();
    let mut samples = SampleHistory::new(SPECTRUM_SIZE);
    let start = Instant::now();
    let mut previous = start;
    let mut last_status: Option<Instant> = None;
    while !shared.stopping.load(Ordering::Acquire) {
        let needed = Inputs(shared.inputs_in_use.load(Ordering::Acquire));
        let now = Instant::now();
        frame.time = (now - start).as_secs_f64();
        frame.delta = (now - previous).as_secs_f32().min(0.25);
        previous = now;

        // Gather inputs outside the lock: audio and WMI calls can take a while.
        audio.update(needed, &mut frame, &mut samples);
        frame.keys.clear();
        frame.keys.append(&mut shared.keys.lock().unwrap_or_else(|e| e.into_inner()));
        if needed.intersects(Inputs::STATUS) && last_status.is_none_or(|t| now - t >= Duration::from_secs(1)) {
            frame.status.update(needed);
            last_status = Some(now);
        }

        let any_running = {
            let mut targets = shared.targets();
            targets.retain(|_, running| running.target.tick(&frame).is_ok()); // a failed device is dropped; its page reports it on the next send
            shared.update_inputs(&targets);
            !targets.is_empty()
        };

        if any_running {
            std::thread::sleep(TICK);
        } else {
            audio.close();
            last_status = None;
            let mut woken = shared.wake.lock().unwrap_or_else(|e| e.into_inner());
            while !*woken {
                woken = shared.wake_signal.wait(woken).unwrap_or_else(|e| e.into_inner());
            }
            *woken = false;
            previous = Instant::now();
        }
    }
    audio.close();
    drop(frame);
    unsafe { CoUninitialize() };
}

/// The audio devices behind Frame::loudness and Frame::spectrum.
#[derive(Default)]
struct AudioInputs {
    meter: Option<(AudioMeter, Instant)>,
    capture: Option<(LoopbackCapture, Instant)>,
    capture_failed_at: Option<Instant>,
    ceiling: f32,
    window: Vec<f32>,
}

impl AudioInputs {
    fn update(&mut self, needed: Inputs, frame: &mut Frame, samples: &mut SampleHistory) {
        if needed.contains(Inputs::LOUDNESS) {
            frame.loudness = self.read_loudness();
        } else {
            self.meter = None;
        }
        if needed.contains(Inputs::SPECTRUM) {
            self.read_spectrum(&mut frame.spectrum, samples);
        } else {
            self.capture = None;
        }
    }

    fn read_loudness(&mut self) -> f32 {
        // Reopen now and then so a change of default output device is picked up.
        if self.meter.as_ref().is_none_or(|(_, opened)| opened.elapsed() > Duration::from_secs(3)) {
            self.meter = AudioMeter::open_default_output().ok().map(|m| (m, Instant::now()));
        }
        let Some((meter, _)) = &self.meter else { return 0.0 };
        match meter.read_peak() {
            Ok(peak) => {
                // Automatic gain like the service's EqualMachine: scale to the loudest recent peak.
                self.ceiling = peak.max(self.ceiling * 0.997).max(0.02);
                (peak / self.ceiling).clamp(0.0, 1.0).powf(1.6)
            }
            Err(_) => {
                self.meter = None; // the audio device went away; try again on the next tick
                0.0
            }
        }
    }

    fn read_spectrum(&mut self, spectrum: &mut AudioSpectrum, samples: &mut SampleHistory) {
        if self.capture.as_ref().is_none_or(|(_, opened)| opened.elapsed() > Duration::from_secs(5)) {
            self.capture = None;
            if self.capture_failed_at.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
                return;
            }
            match LoopbackCapture::open_default_output() {
                Ok(Some(capture)) => self.capture = Some((capture, Instant::now())),
                _ => {
                    self.capture_failed_at = Some(Instant::now());
                    return;
                }
            }
        }
        let Some((capture, _)) = &self.capture else { return };
        if capture.read_into(samples).is_err() {
            self.capture = None;
            samples.clear();
            return;
        }
        self.window.resize(SPECTRUM_SIZE, 0.0);
        samples.copy_to(&mut self.window);
        spectrum.analyze(&self.window, capture.sample_rate);
    }

    fn close(&mut self) {
        self.meter = None;
        self.capture = None;
    }
}

/// The frequency content of the last ~40 ms of audio, with automatic gain.
pub struct AudioSpectrum {
    power: Vec<f32>,
    bin_hz: f32,
    peak_db: f32,
}

impl Default for AudioSpectrum {
    fn default() -> Self {
        AudioSpectrum { power: Vec::new(), bin_hz: 0.0, peak_db: -60.0 }
    }
}

const RANGE_DB: f32 = 45.0; // a band shows from this far below the loudest recent band up to it

impl AudioSpectrum {
    pub fn analyze(&mut self, samples: &[f32], sample_rate: u32) {
        let n = samples.len();
        let mut re: Vec<f32> = samples
            .iter()
            .enumerate()
            .map(|(i, s)| s * (0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos())) // Hann window
            .collect();
        let mut im = vec![0.0f32; n];
        fft(&mut re, &mut im);
        self.power = vec![0.0; n / 2];
        let mut loudest = -120.0f32;
        for i in 1..n / 2 {
            let amplitude = (re[i] * re[i] + im[i] * im[i]).sqrt() / (n as f32 / 4.0);
            self.power[i] = amplitude * amplitude;
            loudest = loudest.max(10.0 * (self.power[i] + 1e-12).log10());
        }
        self.bin_hz = sample_rate as f32 / n as f32;
        // Follow the loudest band at once and let the reference fall back slowly (about 6 dB a second).
        self.peak_db = loudest.max(self.peak_db - 0.15).max(-60.0);
    }

    /// `count` bands spaced evenly in pitch between `low_hz` and `high_hz`, each 0-1.
    pub fn bands(&self, count: usize, low_hz: f32, high_hz: f32) -> Vec<f32> {
        let mut bands = vec![0.0; count];
        if self.power.is_empty() {
            return bands;
        }
        let last_bin = self.power.len() - 1;
        for (b, band) in bands.iter_mut().enumerate() {
            let from = low_hz * (high_hz / low_hz).powf(b as f32 / count as f32);
            let to = low_hz * (high_hz / low_hz).powf((b + 1) as f32 / count as f32);
            let first = ((from / self.bin_hz) as usize).clamp(1, last_bin);
            let last = ((to / self.bin_hz) as usize).clamp(first, last_bin);
            let sum: f32 = self.power[first..=last].iter().sum();
            let db = 10.0 * (sum + 1e-12).log10();
            *band = ((db - (self.peak_db - RANGE_DB)) / RANGE_DB).clamp(0.0, 1.0);
        }
        bands
    }
}

/// In-place radix-2 FFT; the length must be a power of two.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut size = 2;
    while size <= n {
        let angle = -2.0 * std::f32::consts::PI / size as f32;
        let (step_re, step_im) = (angle.cos(), angle.sin());
        for start in (0..n).step_by(size) {
            let (mut w_re, mut w_im) = (1.0f32, 0.0f32);
            for k in 0..size / 2 {
                let a = start + k;
                let b = a + size / 2;
                let t_re = re[b] * w_re - im[b] * w_im;
                let t_im = re[b] * w_im + im[b] * w_re;
                re[b] = re[a] - t_re;
                im[b] = im[a] - t_im;
                re[a] += t_re;
                im[a] += t_im;
                (w_re, w_im) = (w_re * step_re - w_im * step_im, w_re * step_im + w_im * step_re);
            }
        }
        size <<= 1;
    }
}
