//! The laptop's embedded controller, BIOS and Windows: Fn keys, performance, fans, battery, readings and system
//! settings.

pub mod battery;
#[cfg(windows)]
pub mod ec;
#[cfg(target_os = "linux")]
#[path = "ec_linux.rs"]
pub mod ec;
pub mod fans;
#[cfg(windows)]
pub mod gpu;
#[cfg(target_os = "linux")]
#[path = "gpu_linux.rs"]
pub mod gpu;
pub mod hotkeys;
pub mod performance;
pub mod status;
#[cfg(windows)]
pub mod system;
#[cfg(target_os = "linux")]
#[path = "system_linux.rs"]
pub mod system;
pub mod uefi;

/// Gives the Fn keys and the fans back to the EC; for crash and exit handlers.
pub fn release_current() {
    fans::release_current();
    hotkeys::release_current();
}
