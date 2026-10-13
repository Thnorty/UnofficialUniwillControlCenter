//! What plugins show on the light bar: one layer per plugin, the most recently started on top, drawn over the
//! effect by the light bar renderer and the window's preview.

use std::sync::Mutex;

use crate::color::Rgb;

/// A plugin's picture on the light bar. The renderer animates it, so a plugin only says what to show.
#[derive(Clone, Debug, PartialEq)]
pub enum Layer {
    /// A bar filling from the left, 0-1, over a faint track.
    Progress { value: f32, color: Rgb },
    /// Something running with no known end: a light sweeping back and forth.
    Activity(Rgb),
    /// The whole bar breathing: something waits for you.
    Pulse(Rgb),
    Solid(Rgb),
    /// Every LED, left to right.
    Leds(Vec<Rgb>),
}

const TRACK: f32 = 0.08; // how bright the unlit part of a progress bar or sweep is

impl Layer {
    pub fn draw(&self, leds: &mut [Rgb], time: f64) {
        let n = leds.len();
        if n == 0 {
            return;
        }
        match self {
            Layer::Progress { value, color } => {
                let filled = value.clamp(0.0, 1.0) * n as f32;
                // A slow shimmer along the filled part shows it's live.
                let shimmer_at = (time * 0.6).fract() as f32 * (filled + 4.0) - 2.0;
                for (i, led) in leds.iter_mut().enumerate() {
                    let lit = (filled - i as f32).clamp(0.0, 1.0);
                    let shimmer = 1.0 - 0.25 * (1.0 - ((i as f32 - shimmer_at).abs() / 2.0).min(1.0));
                    *led = color.scale(TRACK + (1.0 - TRACK) * lit * shimmer);
                }
            }
            Layer::Activity(color) => {
                // Back and forth every 1.6 s, eased at the ends, with a soft tail.
                let phase = (time / 1.6).fract() as f32;
                let swing = if phase < 0.5 { phase * 2.0 } else { 2.0 - phase * 2.0 };
                let eased = 0.5 - 0.5 * (swing * std::f32::consts::PI).cos();
                let center = eased * (n - 1) as f32;
                for (i, led) in leds.iter_mut().enumerate() {
                    let distance = (i as f32 - center).abs();
                    let glow = (1.0 - distance / 3.5).max(0.0);
                    *led = color.scale(TRACK + (1.0 - TRACK) * glow * glow);
                }
            }
            Layer::Pulse(color) => {
                let wave = 0.5 - 0.5 * ((time * std::f64::consts::TAU / 1.4).cos() as f32);
                leds.fill(color.scale(0.2 + 0.8 * wave));
            }
            Layer::Solid(color) => leds.fill(*color),
            Layer::Leds(colors) => {
                for (i, led) in leds.iter_mut().enumerate() {
                    *led = colors.get(i).copied().unwrap_or(Rgb::BLACK);
                }
            }
        }
    }
}

/// The plugins' layers, shared between the plugin thread, the effects thread and the UI.
#[derive(Default)]
pub struct Layers {
    entries: Mutex<Vec<(String, Layer)>>,
}

impl Layers {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(String, Layer)>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Shows `layer` for `plugin`; a plugin showing something new goes on top. Returns true if nothing was showing
    /// before.
    pub fn set(&self, plugin: &str, layer: Layer) -> bool {
        let mut entries = self.lock();
        let was_empty = entries.is_empty();
        match entries.iter_mut().find(|(id, _)| id == plugin) {
            Some(entry) => entry.1 = layer,
            None => entries.push((plugin.to_string(), layer)),
        }
        was_empty
    }

    /// Removes a plugin's layer. Returns true if that left nothing showing.
    pub fn clear(&self, plugin: &str) -> bool {
        let mut entries = self.lock();
        let before = entries.len();
        entries.retain(|(id, _)| id != plugin);
        before > 0 && entries.is_empty()
    }

    pub fn is_active(&self) -> bool {
        !self.lock().is_empty()
    }

    /// The plugin on top and what it shows.
    #[cfg(test)]
    pub fn top_layer(&self) -> Option<(String, Layer)> {
        self.lock().last().cloned()
    }

    /// Draws the top layer over `leds`; false if there is none.
    pub fn render(&self, leds: &mut [Rgb], time: f64) -> bool {
        match self.lock().last() {
            Some((_, layer)) => {
                layer.draw(leds, time);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_layer_and_activity() {
        let layers = Layers::default();
        assert!(layers.set("a", Layer::Solid(Rgb::new(255, 0, 0))));
        assert!(!layers.set("b", Layer::Progress { value: 0.5, color: Rgb::new(0, 255, 0) }));
        assert!(!layers.set("a", Layer::Solid(Rgb::new(0, 0, 255)))); // updating keeps its place
        assert_eq!(layers.top_layer().map(|(id, _)| id).as_deref(), Some("b"));
        let mut leds = vec![Rgb::BLACK; 10];
        assert!(layers.render(&mut leds, 0.0));
        assert!(leds[0].g > 200 && leds[9].g < 40);
        assert!(!layers.clear("b"));
        assert!(layers.clear("a"));
        assert!(!layers.is_active());
    }
}
