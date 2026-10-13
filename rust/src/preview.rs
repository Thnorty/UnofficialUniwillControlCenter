//! What the window shows: the keyboard drawing's keys, and a simulation of each effect for the live preview and the
//! effect tiles. The firmware animates its own effects, so the preview imitates them; effects this app draws show
//! their real frames instead (see scenes::Live), and are imitated with made-up sound and readings in the tiles.

use crate::color::{HEAT_SCALE, Rgb, gradient};
use crate::effects::{Drawn, Effect, EffectColors, KEYBOARD_RAINBOW, LIGHT_BAR_RAINBOW, keyboard_wave};
use crate::ui::KeyCap;

fn speed_rate(speed: u8, slowest: f64, fastest: f64) -> f64 {
    slowest + (fastest - slowest) * speed.min(4) as f64 / 4.0
}

fn hash(n: u64) -> f32 {
    let mut x = n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

/// Made-up loudness that moves like music: a beat with some variation.
fn fake_loudness(t: f64) -> f32 {
    let beat = ((t * 2.1).fract() as f32 * -6.0).exp();
    let wobble = 0.5 + 0.5 * (t * 5.3).sin() as f32 * (t * 1.7).cos() as f32;
    (0.35 * wobble + 0.7 * beat).clamp(0.0, 1.0)
}

/// Made-up spectrum: louder bass, bands moving on their own.
fn fake_band(t: f64, band: usize, count: usize) -> f32 {
    let f = band as f32 / count.max(1) as f32;
    let beat = ((t * 2.1).fract() as f32 * -5.0).exp() * (1.0 - f);
    let motion = 0.5 + 0.5 * (t * (2.0 + band as f64 * 0.9) + band as f64).sin() as f32;
    (beat * 0.8 + motion * (0.75 - 0.4 * f)).clamp(0.0, 1.0)
}

fn cyclic(palette: &[Rgb], t: f64) -> Rgb {
    // A palette gradient that wraps around from the last color to the first.
    if palette.is_empty() {
        return Rgb::BLACK;
    }
    let n = palette.len() as f64;
    let position = t.rem_euclid(1.0) * n;
    let index = position as usize % palette.len();
    palette[index].lerp(palette[(index + 1) % palette.len()], (position - position.floor()) as f32)
}

/// The colors `n` positions of a device show at time `t` (seconds) for an effect. For the keyboard, n is 4 (zones);
/// the light bar has 23 LEDs; the tiles use 8.
pub fn simulate(effect: &Effect, colors: &[Rgb], direction: u8, speed: u8, t: f64, n: usize, zones_keyboard: bool) -> Vec<Rgb> {
    let fallback;
    if effect.drawn == Drawn::RainbowWave {
        return simulate(keyboard_wave(), &LIGHT_BAR_RAINBOW, direction, speed, t, n, zones_keyboard);
    }
    let palette: &[Rgb] = if effect.is_rainbow() {
        if zones_keyboard { &KEYBOARD_RAINBOW } else { &LIGHT_BAR_RAINBOW }
    } else if colors.is_empty() {
        fallback = effect.default_colors.to_vec();
        &fallback
    } else {
        colors
    };
    let position = |i: usize| if n <= 1 { 0.5 } else { i as f64 / (n - 1) as f64 };
    let mut out = vec![Rgb::BLACK; n];
    match effect.drawn {
        Drawn::RainbowWave => {} // drawn as Wave, above
        Drawn::Firmware => match effect.id {
            1 => {
                // Static: the keyboard's zones, or one color.
                for (i, o) in out.iter_mut().enumerate() {
                    *o = if effect.colors == EffectColors::Zones { palette[i * palette.len() / n] } else { palette[0] };
                }
            }
            2 => {
                // Breathing: each color fades in and out in turn.
                let period = speed_rate(speed, 6.0, 2.0);
                let cycle = t / period;
                let color = palette[(cycle as usize) % palette.len()];
                let level = (std::f64::consts::PI * cycle.fract()).sin().powi(2) as f32;
                out.fill(color.scale(level));
            }
            5 if zones_keyboard => {
                // The keyboard's Rainbow stands still: one color per zone.
                for (i, o) in out.iter_mut().enumerate() {
                    *o = palette[i * palette.len() / n];
                }
            }
            3 | 32 | 5 => {
                // Wave and Rainbow: the colors flow across.
                let cycles_per_second = speed_rate(speed, 0.12, 0.6);
                let sign = if direction == 2 { 1.0 } else { -1.0 };
                for (i, o) in out.iter_mut().enumerate() {
                    *o = cyclic(palette, position(i) * 0.8 + sign * t * cycles_per_second);
                }
            }
            18 => {
                // Flash: on and off, one color at a time.
                let period = speed_rate(speed, 1.6, 0.45);
                let cycle = t / period;
                let color = palette[(cycle as usize) % palette.len()];
                out.fill(if cycle.fract() < 0.5 { color } else { Rgb::BLACK });
            }
            19 => {
                // Mix: the positions take different colors and crossfade through the palette.
                let period = speed_rate(speed, 3.0, 0.8);
                let cycle = t / period;
                for (i, o) in out.iter_mut().enumerate() {
                    let k = cycle as usize + i * 3 / n.max(1);
                    let from = palette[k % palette.len()];
                    let to = palette[(k + 1) % palette.len()];
                    *o = from.lerp(to, (cycle.fract() as f32 * 1.5 - 0.25).clamp(0.0, 1.0));
                }
            }
            _ => {
                // Thinking: warm waves that swell and move.
                let speed_factor = speed_rate(speed, 0.15, 0.6);
                for (i, o) in out.iter_mut().enumerate() {
                    let wave = (0.5 + 0.5 * ((position(i) * 6.0 - t * speed_factor * 6.0).sin())) as f32;
                    *o = cyclic(palette, position(i) * 0.5 + t * speed_factor * 0.3).scale(0.25 + 0.75 * wave);
                }
            }
        },
        Drawn::Music => {
            let level = fake_loudness(t);
            if zones_keyboard {
                for (i, o) in out.iter_mut().enumerate() {
                    *o = palette[i * palette.len() / n].scale(level);
                }
            } else {
                // A meter from the middle.
                let center = (n as f32 - 1.0) / 2.0;
                for (i, o) in out.iter_mut().enumerate() {
                    let distance = (i as f32 - center).abs() / (center + 0.5);
                    if distance <= level {
                        *o = gradient(palette, distance);
                    }
                }
            }
        }
        Drawn::Spectrum => {
            if zones_keyboard {
                for (i, o) in out.iter_mut().enumerate() {
                    let level = fake_band(t, i, n);
                    *o = palette[i * palette.len() / n].scale(level * level);
                }
            } else {
                let center = (n as f32 - 1.0) / 2.0;
                let half = (n / 2 + 1).max(1);
                for (i, o) in out.iter_mut().enumerate() {
                    let d = ((i as f32 - center).abs()).round() as usize;
                    let level = fake_band(t, d, half);
                    *o = gradient(palette, d as f32 / half as f32).scale(level * level);
                }
            }
        }
        Drawn::StarSpark => {
            let rate = speed_rate(speed, 1.5, 6.0);
            for (i, o) in out.iter_mut().enumerate() {
                let phase = t * rate + hash(i as u64) as f64 * 10.0;
                let a = hash(phase as u64 * 31 + i as u64);
                let b = hash((phase as u64 + 1) * 31 + i as u64);
                let level = a + (b - a) * phase.fract() as f32;
                *o = palette[i % palette.len()].scale(level * level);
            }
        }
        Drawn::StarHitting => {
            let velocity = speed_rate(speed, 0.25, 1.0);
            for (k, color) in palette.iter().enumerate() {
                // A triangle wave: the ball goes to the end and back.
                let phase = (t * velocity / 2.0 + k as f64 / palette.len() as f64).rem_euclid(1.0);
                let x = if phase < 0.5 { phase * 2.0 } else { 2.0 - phase * 2.0 } * (n - 1) as f64;
                for (i, o) in out.iter_mut().enumerate() {
                    let glow = 1.0 - ((i as f64 - x).abs() / (n as f64 / 12.0).max(1.0)) as f32;
                    if glow > 0.0 {
                        *o = o.max(color.scale(glow));
                    }
                }
            }
        }
        Drawn::Typing => {
            // Ripples from made-up key presses.
            let interval = 0.45;
            for back in 0..3 {
                let press = (t / interval).floor() - back as f64;
                let age = t - press * interval;
                let origin = hash(press as u64) as f64 * (n - 1) as f64;
                let color = palette[(press as usize) % palette.len()];
                let radius = age * n as f64 * 0.9;
                let fade = (1.0 - age / 1.1).max(0.0) as f32;
                for (i, o) in out.iter_mut().enumerate() {
                    let edge = 1.0 - (((i as f64 - origin).abs() - radius).abs() / (n as f64 / 10.0).max(1.0)) as f32;
                    if edge > 0.0 {
                        *o = o.max(color.scale(edge * fade));
                    }
                }
            }
        }
        Drawn::Battery => {
            let charge = 0.72;
            let filled = charge * n as f32;
            let pulse = (t * 0.8).fract() as f32 * (filled + 2.0) - 1.0;
            for (i, o) in out.iter_mut().enumerate() {
                let inside = (filled - i as f32).clamp(0.0, 1.0);
                if inside > 0.0 {
                    let glow = (1.0 - (i as f32 - pulse).abs() / (n as f32 / 10.0).max(1.0)).max(0.0);
                    *o = Rgb::new(110, 255, 0).scale(0.35 + 0.65 * inside).lerp(Rgb::WHITE, glow * 0.6);
                }
            }
        }
        Drawn::CpuLoad | Drawn::CpuTemperature | Drawn::GpuTemperature => {
            let value = (0.45 + 0.35 * (t * 0.35).sin() + 0.1 * (t * 1.3).sin()) as f32;
            let color = gradient(&HEAT_SCALE, value);
            if zones_keyboard {
                out.fill(color);
            } else {
                let center = (n as f32 - 1.0) / 2.0;
                for (i, o) in out.iter_mut().enumerate() {
                    let distance = (i as f32 - center).abs() / (center + 0.5);
                    let lit = ((value - distance) * n as f32 / 2.0 + 1.0).clamp(0.0, 1.0);
                    *o = color.scale(lit);
                }
            }
        }
    }
    out
}

/// How bright the preview draws a firmware brightness (0-50): perceived brightness isn't linear.
pub fn brightness_scale(light: u8) -> f32 {
    if light == 0 { 0.0 } else { 0.25 + 0.75 * (light as f32 / 50.0).powf(0.8) }
}

/// The Tulpar's keyboard for the drawing, in key widths: the main block (15 wide) and the numpad (4), with their zones.
pub fn keyboard_layout() -> Vec<KeyCap> {
    let mut keys = Vec::new();
    let mut add = |x: f32, y: f32, w: f32, h: f32, label: &str| {
        let center = x + w / 2.0;
        let zone = if center >= 15.0 { 3 } else { ((center / 5.0) as i32).min(2) };
        keys.push(KeyCap { x, y, w, h, zone, label: label.into() });
    };
    // Function row (short keys).
    let f_labels = ["Esc", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "PrtSc", "Ins"];
    for (i, label) in f_labels.iter().enumerate() {
        add(i as f32, 0.0, 1.0, 0.6, label);
    }
    for (i, label) in ["Del", "Home", "End", "PgUp"].iter().enumerate() {
        add(15.25 + i as f32, 0.0, 1.0, 0.6, label);
    }
    let rows = [0.6, 1.6, 2.6, 3.6, 4.6];
    // Number row.
    let mut x = 0.0;
    for label in ["`", "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "-", "="] {
        add(x, rows[0], 1.0, 1.0, label);
        x += 1.0;
    }
    add(x, rows[0], 2.0, 1.0, "Bksp");
    // Q row.
    add(0.0, rows[1], 1.5, 1.0, "Tab");
    x = 1.5;
    for label in ["Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P", "[", "]"] {
        add(x, rows[1], 1.0, 1.0, label);
        x += 1.0;
    }
    add(x, rows[1], 1.5, 1.0, "\\");
    // A row.
    add(0.0, rows[2], 1.75, 1.0, "Caps");
    x = 1.75;
    for label in ["A", "S", "D", "F", "G", "H", "J", "K", "L", ";", "'"] {
        add(x, rows[2], 1.0, 1.0, label);
        x += 1.0;
    }
    add(x, rows[2], 2.25, 1.0, "Enter");
    // Z row, with the up arrow (the right Shift is short on this laptop).
    add(0.0, rows[3], 2.25, 1.0, "Shift");
    x = 2.25;
    for label in ["Z", "X", "C", "V", "B", "N", "M", ",", ".", "/"] {
        add(x, rows[3], 1.0, 1.0, label);
        x += 1.0;
    }
    add(x, rows[3], 0.75, 1.0, "⇧");
    add(13.0, rows[3], 1.0, 1.0, "↑");
    add(14.0, rows[3], 1.0, 1.0, "PgDn");
    // Bottom row with the arrows.
    add(0.0, rows[4], 1.25, 1.0, "Ctrl");
    add(1.25, rows[4], 1.0, 1.0, "Fn");
    add(2.25, rows[4], 1.0, 1.0, "⊞");
    add(3.25, rows[4], 1.25, 1.0, "Alt");
    add(4.5, rows[4], 5.25, 1.0, "");
    add(9.75, rows[4], 1.25, 1.0, "Alt");
    add(11.0, rows[4], 1.0, 1.0, "Ctrl");
    add(12.0, rows[4], 1.0, 1.0, "←");
    add(13.0, rows[4], 1.0, 1.0, "↓");
    add(14.0, rows[4], 1.0, 1.0, "→");
    // Numpad.
    let nx = 15.25;
    for (i, label) in ["Num", "/", "*", "-"].iter().enumerate() {
        add(nx + i as f32, rows[0], 1.0, 1.0, label);
    }
    for (row, labels) in [["7", "8", "9"], ["4", "5", "6"], ["1", "2", "3"]].iter().enumerate() {
        for (i, label) in labels.iter().enumerate() {
            add(nx + i as f32, rows[1 + row], 1.0, 1.0, label);
        }
    }
    add(nx + 3.0, rows[1], 1.0, 2.0, "+");
    add(nx + 3.0, rows[3], 1.0, 2.0, "Ent");
    add(nx, rows[4], 2.0, 1.0, "0");
    add(nx + 2.0, rows[4], 1.0, 1.0, ".");
    keys
}
