//! The laptop's embedded controller, BIOS and Windows: Fn keys, performance, fans, battery, readings and system
//! settings.

pub mod battery;
pub mod ec;
pub mod fans;
pub mod gpu;
pub mod hotkeys;
pub mod performance;
pub mod status;
pub mod system;
pub mod uefi;

/// Gives the Fn keys and the fans back to the EC; for crash and exit handlers.
pub fn release_current() {
    fans::release_current();
    hotkeys::release_current();
}
