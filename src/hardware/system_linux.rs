//! Small pieces of the Linux desktop: the laptop's model, the theme, and starting at login.

use anyhow::{Result, bail};

/// The laptop's maker and model as its firmware reports them ("MONSTER", "TULPAR T5 V21.4"), empty if unknown.
pub fn laptop_model() -> (String, String) {
    let read = |name: &str| std::fs::read_to_string(format!("/sys/class/dmi/id/{name}")).map(|s| s.trim().to_string()).unwrap_or_default();
    (read("sys_vendor"), read("product_name"))
}

/// A setting from the desktop's settings portal (org.freedesktop.portal.Settings), which KDE and GNOME both offer.
fn portal_setting(namespace: &str, key: &str) -> Option<zbus::zvariant::OwnedValue> {
    use zbus::zvariant::{OwnedValue, Value};
    let connection = zbus::blocking::Connection::session().ok()?;
    let call = |method: &str| {
        connection
            .call_method(
                Some("org.freedesktop.portal.Desktop"),
                "/org/freedesktop/portal/desktop",
                Some("org.freedesktop.portal.Settings"),
                method,
                &(namespace, key),
            )
            .ok()?
            .body()
            .deserialize::<OwnedValue>()
            .ok()
    };
    // ReadOne since version 2; the older Read wraps the value in one more variant.
    let value = call("ReadOne").or_else(|| call("Read"))?;
    match &*value {
        Value::Value(inner) => OwnedValue::try_from(inner.as_ref().try_clone().ok()?).ok(),
        _ => Some(value),
    }
}

/// Whether apps use the dark theme: the portal's color scheme, 1 for dark (0 no preference, 2 light).
pub fn dark_mode() -> bool {
    portal_setting("org.freedesktop.appearance", "color-scheme").and_then(|v| u32::try_from(v).ok()) == Some(1)
}

/// Whether the desktop's panels use the dark theme; Linux desktops have one setting for both.
pub fn system_dark_mode() -> bool {
    dark_mode()
}

/// The desktop's accent color, from the portal's `accent-color` (red, green, blue from 0 to 1) where the desktop
/// sets one.
pub fn accent_color() -> Option<crate::color::Rgb> {
    use zbus::zvariant::Value;
    let value = portal_setting("org.freedesktop.appearance", "accent-color")?;
    let Value::Structure(color) = &*value else { return None };
    let channel = |i: usize| match color.fields().get(i)? {
        Value::F64(c) if (0.0..=1.0).contains(c) => Some((c * 255.0).round() as u8),
        _ => None,
    };
    Some(crate::color::Rgb::new(channel(0)?, channel(1)?, channel(2)?))
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

/// Starting the app at login, through an XDG autostart entry, which KDE and GNOME both read.
pub mod startup {
    use super::*;
    use anyhow::Context;
    use std::path::{Path, PathBuf};

    const FILE: &str = "autostart/unofficial-uniwill-control-center.desktop";

    /// `$XDG_CONFIG_HOME`, or ~/.config.
    fn config() -> PathBuf {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
    }

    /// The program the entry starts.
    fn program(config: &Path) -> Option<String> {
        let text = std::fs::read_to_string(config.join(FILE)).ok()?;
        let exec = text.lines().find_map(|l| l.strip_prefix("Exec="))?;
        Some(match exec.strip_prefix('"') {
            Some(quoted) => quoted.split('"').next()?.to_string(),
            None => exec.split(' ').next()?.to_string(),
        })
    }

    pub(super) fn is_enabled_in(config: &Path) -> bool {
        program(config).is_some()
    }

    pub(super) fn starts_this_exe_in(config: &Path) -> bool {
        let exe = std::env::current_exe().ok();
        program(config).is_some_and(|p| exe.is_some_and(|e| Path::new(&p) == e))
    }

    pub(super) fn enable_in(config: &Path) -> Result<()> {
        let exe = std::env::current_exe().context("Couldn't find the app's program")?;
        let exe = exe.display().to_string();
        // The Exec key's quoting: a path with spaces is quoted.
        let exe = if exe.contains(' ') { format!("\"{exe}\"") } else { exe };
        let path = config.join(FILE);
        std::fs::create_dir_all(path.parent().unwrap()).context("Couldn't make the autostart folder")?;
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName=Unofficial Uniwill Control Center\nExec={exe} --tray\n\
             Icon=unofficial-uniwill-control-center\nX-GNOME-Autostart-enabled=true\n"
        );
        std::fs::write(&path, entry).with_context(|| format!("Couldn't write {}", path.display()))
    }

    pub(super) fn disable_in(config: &Path) -> Result<()> {
        match std::fs::remove_file(config.join(FILE)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e).context("Couldn't remove the autostart entry"),
            _ => Ok(()),
        }
    }

    pub fn is_enabled() -> bool {
        is_enabled_in(&config())
    }

    /// Whether the login entry starts this program, and not another copy of the app.
    pub fn starts_this_exe() -> bool {
        starts_this_exe_in(&config())
    }

    /// Starts the app now as it starts at login: nothing to do, as this launch is already running.
    pub fn run_now() -> bool {
        true
    }

    pub fn enable() -> Result<()> {
        enable_in(&config())
    }

    pub fn disable() -> Result<()> {
        disable_in(&config())
    }

    /// Nothing to move over on Linux.
    pub fn migrate() {}

    /// Nothing to remove on Linux.
    pub fn remove_legacy_entries() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("uucc-autostart-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn autostart_file_has_tray_flag() {
        let dir = config("enable");
        startup::enable_in(&dir).unwrap();
        let text = std::fs::read_to_string(dir.join("autostart/unofficial-uniwill-control-center.desktop")).unwrap();
        let exe = std::env::current_exe().unwrap();
        assert!(text.contains(&format!("Exec={} --tray", exe.display())), "{text}");
        assert!(text.starts_with("[Desktop Entry]\n") && text.contains("Type=Application\n"));
        assert!(startup::is_enabled_in(&dir) && startup::starts_this_exe_in(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn disable_removes_autostart() {
        let dir = config("disable");
        startup::enable_in(&dir).unwrap();
        startup::disable_in(&dir).unwrap();
        assert!(!dir.join("autostart/unofficial-uniwill-control-center.desktop").exists());
        assert!(!startup::is_enabled_in(&dir));
        startup::disable_in(&dir).unwrap(); // already gone is fine
        let _ = std::fs::remove_dir_all(dir);
    }
}
