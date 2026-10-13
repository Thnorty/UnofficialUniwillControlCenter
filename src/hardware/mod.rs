//! The laptop's embedded controller and Windows: Fn keys, performance, fans, battery, readings and system settings.

pub mod battery;
pub mod ec;
pub mod fans;
pub mod hotkeys;
pub mod performance;
pub mod status;
pub mod system;

/// Gives the Fn keys and the fans back to the EC; for crash and exit handlers.
pub fn release_current() {
    fans::release_current();
    hotkeys::release_current();
}
