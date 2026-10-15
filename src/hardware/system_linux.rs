//! Small pieces of the Linux desktop: the laptop's model, the theme, and starting at login.

use anyhow::{Result, bail};

/// The laptop's maker and model as its firmware reports them ("MONSTER", "TULPAR T5 V21.4"), empty if unknown.
pub fn laptop_model() -> (String, String) {
    let read = |name: &str| std::fs::read_to_string(format!("/sys/class/dmi/id/{name}")).map(|s| s.trim().to_string()).unwrap_or_default();
    (read("sys_vendor"), read("product_name"))
}

/// Whether apps use the dark theme.
pub fn dark_mode() -> bool {
    false
}

/// Whether the desktop's panels use the dark theme.
pub fn system_dark_mode() -> bool {
    false
}

/// The desktop's accent color.
pub fn accent_color() -> Option<crate::color::Rgb> {
    None
}

/// The touchpad toggle, which comes with the Fn keys.
pub mod touchpad {
    use super::*;

    pub fn is_enabled() -> Option<bool> {
        None
    }

    pub fn toggle() -> Result<()> {
        bail!("Not available on Linux yet")
    }
}

/// Whether Caps Lock and Num Lock are on.
pub fn lock_keys() -> (bool, bool) {
    (false, false)
}

/// Control Center's service; it doesn't exist on Linux.
pub fn control_center_running() -> bool {
    false
}

/// The file names of the running programs, for app profiles (not on Linux yet).
pub fn running_programs() -> std::collections::HashSet<String> {
    std::collections::HashSet::new()
}

/// Starting the app at login.
pub mod startup {
    use super::*;

    pub fn is_enabled() -> bool {
        false
    }

    /// Whether the login entry starts this program, and not another copy of the app.
    pub fn starts_this_exe() -> bool {
        false
    }

    /// Starts the app now as it starts at login.
    pub fn run_now() -> bool {
        false
    }

    pub fn enable() -> Result<()> {
        bail!("Not available on Linux yet")
    }

    pub fn disable() -> Result<()> {
        Ok(())
    }

    /// Nothing to move over on Linux.
    pub fn migrate() {}

    /// Nothing to remove on Linux.
    pub fn remove_legacy_entries() {}
}
