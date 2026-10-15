//! What the tray looks like on every platform: its menu items, the events it reports, and its icon.

use crate::color::Rgb;

pub enum MenuItem {
    /// `checkable`: made by `check`, so it may show a check mark (Linux menus give such items their own kind).
    Item {
        id: u32,
        text: String,
        checked: bool,
        #[cfg_attr(windows, allow(dead_code))]
        checkable: bool,
        enabled: bool,
    },
    Submenu { text: String, enabled: bool, items: Vec<MenuItem> },
    Separator,
}

impl MenuItem {
    pub fn item(id: u32, text: impl Into<String>) -> MenuItem {
        MenuItem::Item { id, text: text.into(), checked: false, checkable: false, enabled: true }
    }

    pub fn check(id: u32, text: impl Into<String>, checked: bool, enabled: bool) -> MenuItem {
        MenuItem::Item { id, text: text.into(), checked, checkable: true, enabled }
    }
}

pub enum TrayEvent {
    /// Open the window: a click on the icon, or a second launch.
    Open,
    /// Quit, as the menu's Exit does: asked by `--exit` (the installer, before replacing or removing the app).
    Exit,
    Command(u32),
    Resumed,
    /// The power adapter was plugged in or out, or the battery changed.
    PowerChanged,
    ThemeChanged,
}

/// Draws the tray icon: the keyboard as four glowing zones in the current colors, on a dark key plate.
pub fn zone_pixels(colors: &[Rgb], size: usize) -> Vec<[u8; 4]> {
    let mut pixels = vec![[0u8; 4]; size * size]; // RGBA
    let s = size as f32 / 32.0;
    let plate = (1.0 * s, 6.0 * s, 31.0 * s, 26.0 * s);
    let radius = 4.0 * s;
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // Coverage of the rounded plate, antialiased over one pixel.
            let dx = (plate.0 + radius - px).max(px - (plate.2 - radius)).max(0.0);
            let dy = (plate.1 + radius - py).max(py - (plate.3 - radius)).max(0.0);
            let outside = (dx * dx + dy * dy).sqrt() - radius;
            let inside_box = px > plate.0 - 1.0 && px < plate.2 + 1.0 && py > plate.1 - 1.0 && py < plate.3 + 1.0;
            let coverage = if inside_box { (0.5 - outside).clamp(0.0, 1.0) } else { 0.0 };
            if coverage <= 0.0 {
                continue;
            }
            let mut color = Rgb::new(32, 32, 36);
            // Four zones of keys inside the plate.
            let inner = (plate.0 + 2.5 * s, plate.1 + 3.0 * s, plate.2 - 2.5 * s, plate.3 - 3.0 * s);
            if px >= inner.0 && px < inner.2 && py >= inner.1 && py < inner.3 {
                let zone_width = (inner.2 - inner.0) / 4.0;
                let zone = (((px - inner.0) / zone_width) as usize).min(3);
                let within = px - inner.0 - zone as f32 * zone_width;
                if within >= 0.6 * s && within <= zone_width - 0.6 * s {
                    let c = colors.get(zone % colors.len().max(1)).copied().unwrap_or(Rgb::WHITE);
                    color = if c.is_black() { Rgb::new(60, 60, 66) } else { c };
                }
            }
            pixels[y * size + x] = [color.r, color.g, color.b, (coverage * 255.0) as u8];
        }
    }
    pixels
}
