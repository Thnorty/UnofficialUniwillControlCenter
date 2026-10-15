//! The keyboard's and the light bar's lighting: the controllers, the effects and the app's own animations.

pub mod audio;
pub mod device;
pub mod effects;
pub mod engine;
#[cfg(windows)]
mod hid;
#[cfg(target_os = "linux")]
#[path = "hid_linux.rs"]
mod hid;
pub mod keyhook;
pub mod preview;
pub mod scenes;
