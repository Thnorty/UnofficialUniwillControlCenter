//! The tray icon on Linux. Not shown yet.

use anyhow::Result;

pub use super::tray_common::{MenuItem, TrayEvent, zone_pixels};

/// A tray icon's pixels: `size` × `size`, ARGB32 in network byte order as StatusNotifierItem takes them.
pub struct Icon {
    pub size: usize,
    pub argb: Vec<u8>,
}

pub struct Tray {
    _private: (),
}

impl Tray {
    pub fn create(_tip: &str, _menu: impl Fn() -> Vec<MenuItem> + 'static, _event: impl Fn(TrayEvent) + 'static) -> Result<Tray> {
        Ok(Tray { _private: () })
    }

    pub fn set_icon(&mut self, _icon: Icon) {}

    pub fn set_hidden(&mut self, _hidden: bool) {}

    pub fn restore(&self) {}

    pub fn notify(&self, _title: &str, _text: &str) {}
}

/// A tray icon from RGBA pixels.
pub fn make_icon(pixels: &[[u8; 4]], size: usize) -> Icon {
    Icon { size, argb: pixels.iter().flat_map(|p| [p[3], p[0], p[1], p[2]]).collect() }
}
