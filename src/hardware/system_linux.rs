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
    /// What the laptop's touchpad key sends: Windows 11's touchpad toggle shortcut. The usual keyboard layouts give F24
    /// no name GNOME matches, so it's given by its key code (evdev 194 + 8), which GNOME's shortcuts take as `0x..`.
    const TOUCHPAD_KEY: &str = "['<Ctrl><Super>0xca']";

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

/// The GNOME Shell extension the package installs so the app's pop-ups can show on GNOME (`shell::osd`).
pub mod gnome_extension {
    pub const UUID: &str = "uucc-osd@thnorty.github.io";

    /// Turns the extension on in GNOME's settings the first time, unless the user turned it off. GNOME loads extensions
    /// at sign-in, so the pop-ups start with the next one.
    pub fn enable() {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        if !desktop.split(':').any(|name| name.eq_ignore_ascii_case("GNOME")) {
            return;
        }
        let _ = std::thread::Builder::new().name("GNOME extension".into()).spawn(|| {
            let gsettings = |args: &[&str]| {
                let output = std::process::Command::new("gsettings").args(args).output().ok().filter(|o| o.status.success())?;
                Some(String::from_utf8_lossy(&output.stdout).into_owned())
            };
            let (Some(enabled), Some(disabled)) =
                (gsettings(&["get", "org.gnome.shell", "enabled-extensions"]), gsettings(&["get", "org.gnome.shell", "disabled-extensions"]))
            else {
                return;
            };
            if let Some(list) = enabled_with_it(&enabled, &disabled) {
                gsettings(&["set", "org.gnome.shell", "enabled-extensions", &list]);
            }
        });
    }

    /// GNOME's enabled list with the extension added, as `gsettings set` takes it; None when it's listed already, the
    /// user turned it off (it's in the disabled list), or the list isn't one this understands.
    pub(super) fn enabled_with_it(enabled: &str, disabled: &str) -> Option<String> {
        let quoted = format!("'{UUID}'");
        let (enabled, disabled) = (enabled.trim(), disabled.trim());
        if enabled.contains(&quoted) || disabled.contains(&quoted) {
            return None;
        }
        if enabled == "@as []" || enabled == "[]" {
            return Some(format!("[{quoted}]"));
        }
        let items = enabled.strip_prefix('[')?.strip_suffix(']')?;
        Some(format!("[{items}, {quoted}]"))
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
        let shortcut = Some("['<Ctrl><Super>0xca']");
        assert_eq!(touchpad::gnome_shortcut_to_set("ubuntu:GNOME", "['']"), shortcut);
        assert_eq!(touchpad::gnome_shortcut_to_set("GNOME", "@as []"), shortcut);
        assert_eq!(touchpad::gnome_shortcut_to_set("GNOME", "[]\n"), shortcut);
    }

    #[test]
    fn a_shortcut_someone_chose_is_kept() {
        assert_eq!(touchpad::gnome_shortcut_to_set("GNOME", "['<Super>t']"), None);
        assert_eq!(touchpad::gnome_shortcut_to_set("GNOME", "['<Ctrl><Super>0xca']"), None); // already there
    }

    #[test]
    fn the_pop_up_extension_is_added_to_gnomes_enabled_list() {
        let uuid = gnome_extension::UUID;
        assert_eq!(gnome_extension::enabled_with_it("@as []", "@as []"), Some(format!("['{uuid}']")));
        assert_eq!(gnome_extension::enabled_with_it("['ubuntu-dock@ubuntu.com']\n", "@as []"), Some(format!("['ubuntu-dock@ubuntu.com', '{uuid}']")));
    }

    #[test]
    fn the_pop_up_extension_is_left_as_it_is_once_listed_or_turned_off() {
        let uuid = gnome_extension::UUID;
        assert_eq!(gnome_extension::enabled_with_it(&format!("['a@b', '{uuid}']"), "@as []"), None);
        assert_eq!(gnome_extension::enabled_with_it("@as []", &format!("['{uuid}']")), None); // turned off by the user
        assert_eq!(gnome_extension::enabled_with_it("not a list", "@as []"), None);
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
