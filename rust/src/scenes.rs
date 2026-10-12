//! The effects this app draws: light bar scenes (sent LED by LED in user mode), indicator flashes drawn over them,
//! and the keyboard's sound and temperature effects (sent as zone colors or brightness).

use std::sync::{Arc, Mutex};

use rand_lite::Random;

use crate::color::{HEAT_SCALE, Rgb, gradient};
use crate::device::{Device, LIGHT_BAR_LEDS, MAX_LIGHT};
use crate::engine::{Frame, Inputs, Target};

/// The latest colors an effect showed, for the window's preview.
pub type Live = Arc<Mutex<Vec<Rgb>>>;

pub fn new_live() -> Live {
    Arc::new(Mutex::new(Vec::new()))
}

fn publish(live: &Live, colors: &[Rgb]) {
    let mut shown = live.lock().unwrap_or_else(|e| e.into_inner());
    shown.clear();
    shown.extend_from_slice(colors);
}

const LEDS: usize = LIGHT_BAR_LEDS;
const CENTER: usize = LEDS / 2;

/// How fast things happen, by speed 0-4.
fn rate(speed: u8, slowest: f32, fastest: f32) -> f32 {
    slowest + (fastest - slowest) * speed.min(4) as f32 / 4.0
}

/// Sends a light bar effect drawn by this app frame by frame, with any indicator flashes on top. The light bar must
/// be in user mode (Device::enter_user_mode).
pub struct LightBarRenderer {
    light_bar: Arc<Device>,
    scene: Box<dyn Scene>,
    overlays: Arc<Mutex<Vec<Overlay>>>,
    overlays_done: Option<Box<dyn FnMut() + Send>>,
    had_overlays: bool,
    last_sent: Option<Vec<Rgb>>,
    live: Live,
}

impl LightBarRenderer {
    pub fn new(light_bar: Arc<Device>, scene: Box<dyn Scene>, overlays: Arc<Mutex<Vec<Overlay>>>, live: Live) -> Self {
        LightBarRenderer { light_bar, scene, overlays, overlays_done: None, had_overlays: false, last_sent: None, live }
    }

    /// Called (on the effects thread) once the flashes on top have finished.
    pub fn on_overlays_done(mut self, done: impl FnMut() + Send + 'static) -> Self {
        self.overlays_done = Some(Box::new(done));
        self
    }
}

impl Target for LightBarRenderer {
    fn inputs(&self) -> Inputs {
        self.scene.inputs()
    }

    fn tick(&mut self, frame: &Frame) -> anyhow::Result<()> {
        let mut leds = self.scene.render(frame);
        let any_overlay = {
            let mut overlays = self.overlays.lock().unwrap_or_else(|e| e.into_inner());
            overlays.retain_mut(|o| o.apply(&mut leds, frame.time));
            !overlays.is_empty()
        };
        if self.had_overlays && !any_overlay
            && let Some(done) = &mut self.overlays_done
        {
            done();
        }
        self.had_overlays = any_overlay;
        publish(&self.live, &leds);
        if self.last_sent.as_deref() == Some(&leds[..]) {
            return Ok(());
        }
        self.light_bar.set_leds(&leds)?;
        self.last_sent = Some(leds);
        Ok(())
    }
}

/// A light bar effect drawn by this app: one color per LED, left to right, every tick.
pub trait Scene: Send {
    fn inputs(&self) -> Inputs {
        Inputs::NONE
    }

    fn render(&mut self, frame: &Frame) -> Vec<Rgb>;
}

/// Fixed colors, such as the keyboard's zones as four sections.
pub struct StaticScene(pub Vec<Rgb>);

impl Scene for StaticScene {
    fn render(&mut self, _: &Frame) -> Vec<Rgb> {
        self.0.clone()
    }
}

/// A dark bar, under an indicator flash shown while a firmware effect is paused.
pub struct DarkScene;

impl Scene for DarkScene {
    fn render(&mut self, _: &Frame) -> Vec<Rgb> {
        vec![Rgb::BLACK; LEDS]
    }
}

/// The four zone colors as four sections of the light bar, left to right.
pub fn zone_sections(zones: &[Rgb]) -> Vec<Rgb> {
    (0..LEDS).map(|i| zones[i * zones.len() / LEDS]).collect()
}

// Kept per tick, by speed: faster speeds fall back sooner.
const DECAY: [f32; 5] = [0.93, 0.89, 0.84, 0.77, 0.68];

/// Music (LM_ITE_RGB.AudioData_LightbarEventHandler): a level meter that grows from the middle outwards, each LED
/// colored by its distance from the middle.
pub struct MusicMeterScene {
    colors: Vec<Rgb>,
    speed: u8,
    level: f32,
}

impl MusicMeterScene {
    pub fn new(colors: Vec<Rgb>, speed: u8) -> Self {
        MusicMeterScene { colors, speed, level: 0.0 }
    }
}

impl Scene for MusicMeterScene {
    fn inputs(&self) -> Inputs {
        Inputs::LOUDNESS
    }

    fn render(&mut self, frame: &Frame) -> Vec<Rgb> {
        self.level = frame.loudness.max(self.level * DECAY[self.speed.min(4) as usize]);
        let mut leds = vec![Rgb::BLACK; LEDS];
        let lit = (self.level * (CENTER + 1) as f32).round() as usize; // LEDs lit on each side, counting the middle one
        for distance in 0..lit.min(CENTER + 1) {
            // The service gives the middle two steps the first color, then one color per step outwards.
            let color = self.colors[distance.saturating_sub(1).min(self.colors.len() - 1)];
            leds[CENTER - distance] = color;
            leds[CENTER + distance] = color;
        }
        leds
    }
}

/// A spectrum analyzer, mirrored: the lowest notes in the middle, the highest at the ends. Each LED glows as loud as
/// its band, colored from the palette by pitch.
pub struct SpectrumScene {
    colors: Vec<Rgb>,
    speed: u8,
    levels: [f32; CENTER + 1],
}

impl SpectrumScene {
    pub fn new(colors: Vec<Rgb>, speed: u8) -> Self {
        SpectrumScene { colors, speed, levels: [0.0; CENTER + 1] }
    }
}

impl Scene for SpectrumScene {
    fn inputs(&self) -> Inputs {
        Inputs::SPECTRUM
    }

    fn render(&mut self, frame: &Frame) -> Vec<Rgb> {
        let bands = frame.spectrum.bands(self.levels.len(), 50.0, 14000.0);
        let fall = rate(self.speed, 2.5, 9.0) * frame.delta; // how much a band drops per tick
        let mut leds = vec![Rgb::BLACK; LEDS];
        let count = self.levels.len();
        for d in 0..count {
            self.levels[d] = bands[d].max(self.levels[d] - fall);
            let color = gradient(&self.colors, d as f32 / (count - 1) as f32).scale(self.levels[d] * self.levels[d]);
            leds[CENTER - d] = color;
            leds[CENTER + d] = color;
        }
        leds
    }
}

/// Star Spark (LM_ITE_RGB.AP_Effect_Task, effect 24): every LED twinkles at a random brightness of its color.
pub struct StarSparkScene {
    colors: Vec<Rgb>,
    speed: u8,
    random: Random,
    brightness: [f32; LEDS],
    target: [f32; LEDS],
    next_change: f64,
}

impl StarSparkScene {
    pub fn new(colors: Vec<Rgb>, speed: u8) -> Self {
        StarSparkScene { colors, speed, random: Random::new(), brightness: [0.0; LEDS], target: [0.0; LEDS], next_change: 0.0 }
    }
}

impl Scene for StarSparkScene {
    fn render(&mut self, frame: &Frame) -> Vec<Rgb> {
        if frame.time >= self.next_change {
            // The service picks new brightnesses every 150 ms; the speed slider sets how often here.
            for t in &mut self.target {
                *t = self.random.next_f32();
            }
            self.next_change = frame.time + 0.6 / rate(self.speed, 1.0, 5.0) as f64;
        }
        let step = rate(self.speed, 2.0, 10.0) * frame.delta;
        (0..LEDS)
            .map(|i| {
                self.brightness[i] += (self.target[i] - self.brightness[i]).clamp(-step, step);
                self.colors[i % self.colors.len()].scale(self.brightness[i] * self.brightness[i])
            })
            .collect()
    }
}

/// Star Hitting (LM_ITE_RGB.AP_Effect_Task, effect 25): a ball of light per color bouncing along the bar, turning
/// back at the ends and off each other.
pub struct StarHittingScene {
    colors: Vec<Rgb>,
    velocity: f32,
    position: Vec<f32>,
    direction: Vec<f32>,
}

impl StarHittingScene {
    pub fn new(colors: Vec<Rgb>, speed: u8) -> Self {
        let n = colors.len();
        StarHittingScene {
            velocity: rate(speed, 4.0, 18.0), // LEDs per second
            position: (0..n).map(|i| (i as f32 + 0.5) * (LEDS - 1) as f32 / n as f32).collect(),
            direction: (0..n).map(|i| if i % 2 == 0 { 1.0 } else { -1.0 }).collect(), // like the service: alternate directions
            colors,
        }
    }
}

impl Scene for StarHittingScene {
    fn render(&mut self, frame: &Frame) -> Vec<Rgb> {
        let end = (LEDS - 1) as f32;
        for i in 0..self.position.len() {
            self.position[i] += self.direction[i] * self.velocity * frame.delta;
            if self.position[i] <= 0.0 {
                self.position[i] = 0.0;
                self.direction[i] = 1.0;
            }
            if self.position[i] >= end {
                self.position[i] = end;
                self.direction[i] = -1.0;
            }
        }
        // Balls heading into each other bounce apart.
        for i in 0..self.position.len() {
            for j in i + 1..self.position.len() {
                let (pi, pj) = (self.position[i], self.position[j]);
                let (di, dj) = (self.direction[i], self.direction[j]);
                if (pi - pj).abs() < 1.0 && di != dj && (pi - pj) * (di - dj) < 0.0 {
                    self.direction.swap(i, j);
                }
            }
        }
        let mut leds = vec![Rgb::BLACK; LEDS];
        for (i, &position) in self.position.iter().enumerate() {
            for (led, value) in leds.iter_mut().enumerate() {
                let glow = 1.0 - (led as f32 - position).abs() / 1.5; // a soft ball about three LEDs wide
                if glow > 0.0 {
                    *value = value.max(self.colors[i].scale(glow));
                }
            }
        }
        leds
    }
}

/// Typing ripples: each key press sends a pulse of light outwards from the matching spot on the bar. The colors take
/// turns, one per key press.
pub struct TypingScene {
    colors: Vec<Rgb>,
    speed: u8,
    ripples: Vec<(f32, f64, Rgb)>,
    next_color: usize,
}

impl TypingScene {
    pub fn new(colors: Vec<Rgb>, speed: u8) -> Self {
        TypingScene { colors, speed, ripples: Vec::new(), next_color: 0 }
    }
}

impl Scene for TypingScene {
    fn inputs(&self) -> Inputs {
        Inputs::KEYS
    }

    fn render(&mut self, frame: &Frame) -> Vec<Rgb> {
        for &key in &frame.keys {
            self.ripples.push((key * (LEDS - 1) as f32, frame.time, self.colors[self.next_color % self.colors.len()]));
            self.next_color += 1;
        }
        let spread = rate(self.speed, 10.0, 30.0); // LEDs per second
        let lifetime = rate(self.speed, 1.2, 0.4);
        self.ripples.retain(|r| frame.time - r.1 <= lifetime as f64);
        let mut leds = vec![Rgb::BLACK; LEDS];
        for &(origin, start, color) in &self.ripples {
            let age = (frame.time - start) as f32;
            let (radius, fade) = (age * spread, 1.0 - age / lifetime);
            for (led, value) in leds.iter_mut().enumerate() {
                let edge = 1.0 - ((led as f32 - origin).abs() - radius).abs() / 1.5; // a ring about three LEDs wide
                if edge > 0.0 {
                    *value = value.max(color.scale(edge * fade));
                }
            }
        }
        leds
    }
}

/// The battery level as a bar from the left, green when full through yellow to red when low. While charging, a pulse
/// runs along it.
pub struct BatteryScene;

impl Scene for BatteryScene {
    fn inputs(&self) -> Inputs {
        Inputs::BATTERY
    }

    fn render(&mut self, frame: &Frame) -> Vec<Rgb> {
        const LOW: Rgb = Rgb::new(255, 0, 0);
        const MIDDLE: Rgb = Rgb::new(255, 160, 0);
        const FULL: Rgb = Rgb::new(0, 255, 0);
        let mut leds = vec![Rgb::BLACK; LEDS];
        let Some(percent) = frame.status.battery_percent else {
            leds[CENTER] = Rgb::WHITE.scale(0.2); // no battery reading
            return leds;
        };
        let charge = percent as f32 / 100.0;
        let color = if charge < 0.5 { LOW.lerp(MIDDLE, (charge - 0.1) / 0.4) } else { MIDDLE.lerp(FULL, (charge - 0.5) / 0.4) };
        let filled = (charge * LEDS as f32).max(1.0);
        let pulse = frame.status.charging.then(|| (frame.time * 8.0 % (filled as f64 + 4.0)) as f32 - 2.0);
        for (led, value) in leds.iter_mut().enumerate() {
            let inside = (filled - led as f32).clamp(0.0, 1.0); // the last LED can be partly lit
            if inside <= 0.0 {
                continue;
            }
            *value = color.scale(0.35 + 0.65 * inside);
            if let Some(pulse) = pulse {
                let glow = 1.0 - (led as f32 - pulse).abs() / 2.0;
                if glow > 0.0 {
                    *value = value.lerp(Rgb::WHITE, glow * 0.7);
                }
            }
        }
        leds
    }
}

/// The reading of a status gauge as 0-1, or None if there isn't one (e.g. the GPU is switched off).
fn gauge_value(input: Inputs, frame: &Frame) -> Option<f32> {
    let status = &frame.status;
    match input {
        Inputs::CPU_LOAD => status.cpu_load,
        Inputs::CPU_TEMPERATURE => status.cpu_temperature.map(|c| (c - 35.0) / 60.0), // 35-95 °C
        Inputs::GPU_TEMPERATURE => status.gpu_temperature.map(|g| (g - 35.0) / 55.0), // 35-90 °C
        _ => None,
    }
}

/// A gauge growing from the middle: CPU load, or CPU or GPU temperature. It turns from blue through green and yellow
/// to red as the value rises, and moves smoothly between the once-a-second readings.
pub struct GaugeScene {
    input: Inputs,
    shown: f32,
}

impl GaugeScene {
    pub fn new(input: Inputs) -> Self {
        GaugeScene { input, shown: -1.0 }
    }
}

impl Scene for GaugeScene {
    fn inputs(&self) -> Inputs {
        self.input
    }

    fn render(&mut self, frame: &Frame) -> Vec<Rgb> {
        let mut leds = vec![Rgb::BLACK; LEDS];
        let Some(value) = gauge_value(self.input, frame) else {
            leds[CENTER] = Rgb::WHITE.scale(0.15); // no reading right now
            self.shown = -1.0;
            return leds;
        };
        let value = value.clamp(0.0, 1.0);
        self.shown = if self.shown < 0.0 { value } else { self.shown + (value - self.shown).clamp(-frame.delta, frame.delta) };
        let color = gradient(&HEAT_SCALE, self.shown);
        let reach = self.shown * CENTER as f32 + 1.0; // LEDs on each side, counting the middle one
        for d in 0..=CENTER {
            let lit = (reach - d as f32).clamp(0.0, 1.0);
            if lit <= 0.0 {
                break;
            }
            leds[CENTER - d] = color.scale(lit);
            leds[CENTER + d] = color.scale(lit);
        }
        leds
    }
}

/// How an indicator flash moves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OverlayStyle {
    /// Light fills from the middle outwards, then fades: something turned on.
    Open,
    /// The bar lights up, then shrinks to the middle: something turned off.
    Close,
    /// The whole bar blinks: an alert.
    Blink,
}

/// A short animation drawn over the light bar for an indicator, such as Caps Lock turning on.
pub struct Overlay {
    color: Rgb,
    style: OverlayStyle,
    duration: f64,
    start: Option<f64>,
}

impl Overlay {
    pub fn new(color: Rgb, style: OverlayStyle, seconds: f64) -> Self {
        Overlay { color, style, duration: seconds, start: None }
    }

    /// Draws over `leds`; returns false once finished.
    pub fn apply(&mut self, leds: &mut [Rgb], time: f64) -> bool {
        let start = *self.start.get_or_insert(time);
        let t = ((time - start) / self.duration) as f32;
        if t >= 1.0 {
            return false;
        }
        match self.style {
            OverlayStyle::Open => {
                let reach = (t * 3.0).min(1.0) * (CENTER + 1) as f32;
                let fade = if t < 0.5 { 1.0 } else { 1.0 - (t - 0.5) * 2.0 };
                let mut d = 0;
                while (d as f32) < reach && d <= CENTER {
                    leds[CENTER - d] = self.color.scale(fade);
                    leds[CENTER + d] = self.color.scale(fade);
                    d += 1;
                }
            }
            OverlayStyle::Close => {
                let reach = (1.0 - t) * (CENTER + 1) as f32;
                for (i, led) in leds.iter_mut().enumerate() {
                    *led = if ((i as f32) - CENTER as f32).abs() < reach { self.color.scale(1.0 - t * 0.5) } else { Rgb::BLACK };
                }
            }
            OverlayStyle::Blink => {
                // On and off every quarter second: two blinks a second.
                let on = ((t as f64 * self.duration * 4.0) as i64) % 2 == 0;
                leds.fill(if on { self.color } else { Rgb::BLACK });
            }
        }
        true
    }
}

/// The keyboard's Music (LM_ITE_RGB.DLL_SetMusicMode, MUSICTYPE.BRIGHTNESS): the static zone colors, with the
/// brightness following the sound.
pub struct KeyboardMusic {
    keyboard: Arc<Device>,
    zones: Vec<Rgb>,
    peak_light: u8,
    speed: u8,
    level: f32,
    last_sent: i32,
    live: Live,
}

impl KeyboardMusic {
    pub fn new(keyboard: Arc<Device>, zones: Vec<Rgb>, peak_light: u8, speed: u8, live: Live) -> Self {
        KeyboardMusic { keyboard, zones, peak_light, speed, level: 0.0, last_sent: -1, live }
    }
}

impl Target for KeyboardMusic {
    fn inputs(&self) -> Inputs {
        Inputs::LOUDNESS
    }

    fn drives_brightness(&self) -> bool {
        true
    }

    fn tick(&mut self, frame: &Frame) -> anyhow::Result<()> {
        self.level = frame.loudness.max(self.level * DECAY[self.speed.min(4) as usize]);
        let light = (self.peak_light as f32 * self.level).round() as i32;
        let shown: Vec<Rgb> = self.zones.iter().map(|z| z.scale(self.level)).collect();
        publish(&self.live, &shown);
        if light == self.last_sent {
            return Ok(());
        }
        self.keyboard.set_raw_light(light.clamp(0, MAX_LIGHT as i32) as u8)?;
        self.last_sent = light;
        Ok(())
    }
}

/// A spectrum analyzer on the four zones: the lowest notes on the left, the highest on the right, each zone glowing
/// in its color as loud as its band.
pub struct KeyboardSpectrum {
    keyboard: Arc<Device>,
    zones: Vec<Rgb>,
    speed: u8,
    levels: [f32; 4],
    shown: [Rgb; 4],
    last_send: f64,
    live: Live,
}

impl KeyboardSpectrum {
    pub fn new(keyboard: Arc<Device>, zones: Vec<Rgb>, speed: u8, live: Live) -> Self {
        KeyboardSpectrum { keyboard, zones, speed, levels: [0.0; 4], shown: [Rgb::BLACK; 4], last_send: f64::NEG_INFINITY, live }
    }
}

impl Target for KeyboardSpectrum {
    fn inputs(&self) -> Inputs {
        Inputs::SPECTRUM
    }

    fn tick(&mut self, frame: &Frame) -> anyhow::Result<()> {
        const MIN_INTERVAL: f64 = 0.05; // the zones take a full color update; 20 a second is plenty
        let bands = frame.spectrum.bands(4, 50.0, 12000.0);
        let fall = (2.5 + 6.5 * self.speed.min(4) as f32 / 4.0) * frame.delta;
        for i in 0..4 {
            self.levels[i] = bands[i].max(self.levels[i] - fall);
        }
        let colors: [Rgb; 4] = std::array::from_fn(|i| self.zones[i].scale(self.levels[i] * self.levels[i]));
        publish(&self.live, &colors);
        if frame.time - self.last_send < MIN_INTERVAL || colors == self.shown {
            return Ok(());
        }
        self.keyboard.set_zone_colors(&colors)?;
        self.shown = colors;
        self.last_send = frame.time;
        Ok(())
    }
}

/// The whole keyboard colored by the CPU or GPU temperature: blue when cool, through green and yellow, to red when hot.
pub struct KeyboardTemperature {
    keyboard: Arc<Device>,
    gpu: bool,
    shown: Option<Rgb>,
    live: Live,
}

impl KeyboardTemperature {
    pub fn new(keyboard: Arc<Device>, gpu: bool, live: Live) -> Self {
        KeyboardTemperature { keyboard, gpu, shown: None, live }
    }
}

impl Target for KeyboardTemperature {
    fn inputs(&self) -> Inputs {
        if self.gpu { Inputs::GPU_TEMPERATURE } else { Inputs::CPU_TEMPERATURE }
    }

    fn tick(&mut self, frame: &Frame) -> anyhow::Result<()> {
        let celsius = if self.gpu { frame.status.gpu_temperature } else { frame.status.cpu_temperature };
        let color = match celsius {
            Some(c) => gradient(&HEAT_SCALE, (c - 35.0) / if self.gpu { 55.0 } else { 60.0 }),
            None => Rgb::new(40, 40, 40), // dim white: no reading (e.g. the GPU is off)
        };
        publish(&self.live, &[color; 4]);
        // Only resend for a visible change; readings arrive once a second.
        if self.shown.is_some_and(|shown| shown.distance(color) < 12) {
            return Ok(());
        }
        self.keyboard.set_zone_colors(&[color; 4])?;
        self.shown = Some(color);
        Ok(())
    }
}

/// A small random number generator for the twinkling effects (no need for a crate).
mod rand_lite {
    pub struct Random(u64);

    impl Random {
        pub fn new() -> Self {
            let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
            Random(seed | 1)
        }

        /// 0-1.
        pub fn next_f32(&mut self) -> f32 {
            // xorshift64*
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            ((self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32) / (1u64 << 24) as f32
        }
    }
}
