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

    const MEDIA_KEYS: &str = "org.gnome.settings-daemon.plugins.media-keys";
    /// What the laptop's touchpad key sends: Windows 11's touchpad toggle shortcut.
    const TOUCHPAD_KEY: &str = "['<Ctrl><Super>F24']";

    /// The touchpad key sends Ctrl+Super+F24. KDE takes that as the touchpad toggle; GNOME only knows the dedicated
    /// touchpad key, so its own "Toggle touchpad" shortcut gets this combination, unless someone chose one already.
    pub fn teach_gnome() {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        if gnome_shortcut_to_set(&desktop, "[]").is_none() {
            return;
        }
        let _ = std::thread::Builder::new().name("Touchpad shortcut".into()).spawn(move || {
            let gsettings = |args: &[&str]| std::process::Command::new("gsettings").args(args).output().ok().filter(|o| o.status.success());
            let Some(current) = gsettings(&["get", MEDIA_KEYS, "touchpad-toggle"]) else { return };
            if let Some(shortcut) = gnome_shortcut_to_set(&desktop, &String::from_utf8_lossy(&current.stdout)) {
                gsettings(&["set", MEDIA_KEYS, "touchpad-toggle", shortcut]);
            }
        });
    }

    /// The shortcut to give GNOME's "Toggle touchpad", given the desktop and the setting as `gsettings get` prints it.
    pub(super) fn gnome_shortcut_to_set(desktop: &str, current: &str) -> Option<&'static str> {
        let gnome = desktop.split(':').any(|name| name.eq_ignore_ascii_case("GNOME"));
        let empty = matches!(current.trim(), "['']" | "[]" | "@as []");
        (gnome && empty).then_some(TOUCHPAD_KEY)
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

    pub fn enable() -> Result<()> {
        enable_in(&config())
    }

    pub fn disable() -> Result<()> {
        disable_in(&config())
    }

    /// Nothing to move over on Linux.
    pub fn migrate() {}

    /// Turns starting at login on the first time the app runs, as the Windows installer does by default (a package
    /// installs as root, and the entry is the user's). `done` remembers it, so turning it off in Settings stays off.
    pub fn enable_first_time(done: &mut bool) -> Result<()> {
        enable_first_time_in(&config(), done)
    }

    pub(super) fn enable_first_time_in(config: &Path, done: &mut bool) -> Result<()> {
        if !*done {
            enable_in(config)?;
            *done = true;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnome_learns_the_touchpad_key_while_its_shortcut_is_empty() {
        let shortcut = Some("['<Ctrl><Super>F24']");
        assert_eq!(touchpad::gnome_shortcut_to_set("ubuntu:GNOME", "['']"), shortcut);
        assert_eq!(touchpad::gnome_shortcut_to_set("GNOME", "@as []"), shortcut);
        assert_eq!(touchpad::gnome_shortcut_to_set("GNOME", "[]\n"), shortcut);
    }

    #[test]
    fn a_shortcut_someone_chose_is_kept() {
        assert_eq!(touchpad::gnome_shortcut_to_set("GNOME", "['<Super>t']"), None);
        assert_eq!(touchpad::gnome_shortcut_to_set("GNOME", "['<Ctrl><Super>F24']"), None); // already there
    }

    #[test]
    fn other_desktops_are_left_alone() {
        assert_eq!(touchpad::gnome_shortcut_to_set("KDE", "['']"), None); // KDE knows the key by itself
        assert_eq!(touchpad::gnome_shortcut_to_set("", "['']"), None);
    }

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
        assert!(startup::is_enabled_in(&dir));
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

    #[test]
    fn start_at_login_is_turned_on_only_the_first_time() {
        let dir = config("first");
        let mut done = false;
        startup::enable_first_time_in(&dir, &mut done).unwrap();
        assert!(startup::is_enabled_in(&dir) && done); // as the Windows installer does by default
        // Turned off in Settings: the next start leaves it off.
        startup::disable_in(&dir).unwrap();
        startup::enable_first_time_in(&dir, &mut done).unwrap();
        assert!(!startup::is_enabled_in(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }
}
