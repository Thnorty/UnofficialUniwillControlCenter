//! The tray icon on Linux: a StatusNotifierItem (ksni), which KDE Plasma shows natively and GNOME with the AppIndicator
//! extension. The tray runs on ksni's own thread; its clicks, and the watchers for sleep, the charger, the theme and
//! later launches, reach the app on the UI thread through Slint's event loop, as the window messages do on Windows.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use ksni::blocking::TrayMethods;

pub use super::tray_common::{MenuItem, TrayEvent, zone_pixels};
use super::instance::{self, Control};

/// A tray icon's pixels: `size` × `size`, ARGB32 in network byte order as StatusNotifierItem takes them.
pub struct Icon {
    pub size: usize,
    pub argb: Vec<u8>,
}

/// A tray icon from RGBA pixels.
pub fn make_icon(pixels: &[[u8; 4]], size: usize) -> Icon {
    Icon { size, argb: pixels.iter().flat_map(|p| [p[3], p[0], p[1], p[2]]).collect() }
}

struct Handlers {
    menu: Box<dyn Fn() -> Vec<MenuItem>>,
    event: Box<dyn Fn(TrayEvent)>,
}

thread_local! {
    static HANDLERS: RefCell<Option<Handlers>> = const { RefCell::new(None) };
    static HANDLE: RefCell<Option<ksni::blocking::Handle<SniTray>>> = const { RefCell::new(None) };
}

/// Hands an event to the app on the UI thread.
fn post(event: TrayEvent) {
    let _ = slint::invoke_from_event_loop(move || {
        HANDLERS.with(|h| {
            if let Some(handlers) = &*h.borrow() {
                (handlers.event)(event);
            }
        })
    });
}

/// A menu item as the tray thread keeps it: the app's `MenuItem`, which stays on the UI thread, copied.
#[derive(Clone, Debug, PartialEq)]
enum Entry {
    Item { id: u32, text: String, checked: bool, checkable: bool, enabled: bool },
    Submenu { text: String, enabled: bool, items: Vec<Entry> },
    Separator,
}

fn snapshot(items: &[MenuItem]) -> Vec<Entry> {
    items
        .iter()
        .map(|item| match item {
            MenuItem::Item { id, text, checked, checkable, enabled } => {
                Entry::Item { id: *id, text: text.clone(), checked: *checked, checkable: *checkable, enabled: *enabled }
            }
            MenuItem::Submenu { text, enabled, items } => Entry::Submenu { text: text.clone(), enabled: *enabled, items: snapshot(items) },
            MenuItem::Separator => Entry::Separator,
        })
        .collect()
}

fn ksni_menu(entries: &[Entry]) -> Vec<ksni::MenuItem<SniTray>> {
    use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};
    entries
        .iter()
        .map(|entry| match entry {
            &Entry::Item { id, ref text, checked, checkable, enabled } => {
                let activate = Box::new(move |_: &mut SniTray| post(TrayEvent::Command(id)));
                if checkable {
                    CheckmarkItem { label: text.clone(), checked, enabled, activate, ..Default::default() }.into()
                } else {
                    StandardItem { label: text.clone(), enabled, activate, ..Default::default() }.into()
                }
            }
            Entry::Submenu { text, enabled, items } => {
                SubMenu { label: text.clone(), enabled: *enabled, submenu: ksni_menu(items), ..Default::default() }.into()
            }
            Entry::Separator => ksni::MenuItem::Separator,
        })
        .collect()
}

/// What ksni shows.
struct SniTray {
    tip: String,
    icon: Option<ksni::Icon>,
    hidden: bool,
    menu: Vec<Entry>,
}

impl ksni::Tray for SniTray {
    fn id(&self) -> String {
        "unofficial-uniwill-control-center".into()
    }

    fn title(&self) -> String {
        self.tip.clone()
    }

    fn status(&self) -> ksni::Status {
        // Passive: the panel moves it among the hidden icons.
        if self.hidden { ksni::Status::Passive } else { ksni::Status::Active }
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.iter().map(|i| ksni::Icon { width: i.width, height: i.height, data: i.data.clone() }).collect()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: self.tip.clone(), ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        post(TrayEvent::Open);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        ksni_menu(&self.menu)
    }

    fn menu_about_to_show(&mut self) {
        // The app's menu lives on the UI thread: rebuild it there, so its checks are current.
        let _ = slint::invoke_from_event_loop(update_menu);
    }
}

/// Copies the app's menu to the tray.
fn update_menu() {
    let menu = HANDLERS.with(|h| h.borrow().as_ref().map(|handlers| snapshot(&(handlers.menu)())));
    if let Some(menu) = menu {
        update(move |tray| tray.menu = menu);
    }
}

fn update(change: impl FnOnce(&mut SniTray) + Send + 'static) {
    HANDLE.with(|h| {
        if let Some(handle) = &*h.borrow() {
            handle.update(change);
        }
    });
}

/// Whether the power adapter's state changed since the last reading; the first reading isn't a change.
fn ac_changed(last: &mut Option<bool>, now: bool) -> bool {
    let changed = last.is_some_and(|l| l != now);
    *last = Some(now);
    changed
}

/// Starts the threads that turn later launches, sleep, the charger and the theme into tray events.
fn watch() {
    let messages = instance::control_messages();
    spawn("Instance messages", move || {
        for control in messages {
            post(match control {
                Control::Show => TrayEvent::Open,
                Control::Exit => TrayEvent::Exit,
            });
        }
    });
    spawn("Sleep", || {
        let watch = || -> zbus::Result<()> {
            let connection = zbus::blocking::Connection::system()?;
            let proxy = zbus::blocking::Proxy::new(&connection, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager")?;
            for signal in proxy.receive_signal("PrepareForSleep")? {
                // false: the laptop woke up.
                if signal.body().deserialize::<bool>() == Ok(false) {
                    post(TrayEvent::Resumed);
                }
            }
            Ok(())
        };
        let _ = watch();
    });
    spawn("Power", || {
        let mut last = None;
        loop {
            if ac_changed(&mut last, crate::hardware::status::on_ac()) {
                post(TrayEvent::PowerChanged);
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
    spawn("Theme", || {
        let watch = || -> zbus::Result<()> {
            let connection = zbus::blocking::Connection::session()?;
            let proxy = zbus::blocking::Proxy::new(&connection, "org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop", "org.freedesktop.portal.Settings")?;
            for signal in proxy.receive_signal("SettingChanged")? {
                let Ok((namespace, key, _)) = signal.body().deserialize::<(String, String, zbus::zvariant::OwnedValue)>() else { continue };
                if namespace == "org.freedesktop.appearance" && (key == "color-scheme" || key == "accent-color") {
                    post(TrayEvent::ThemeChanged);
                }
            }
            Ok(())
        };
        let _ = watch();
    });
}

fn spawn(name: &str, job: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name(name.into()).spawn(job);
}

pub struct Tray {
    _private: (),
}

impl Tray {
    /// Starts the tray (it shows once it has an icon) and the event watchers. Without a StatusNotifierItem host
    /// (GNOME without the AppIndicator extension), there's no icon, but the events still arrive.
    pub fn create(tip: &str, menu: impl Fn() -> Vec<MenuItem> + 'static, event: impl Fn(TrayEvent) + 'static) -> Result<Tray> {
        let items = snapshot(&menu());
        HANDLERS.with(|h| *h.borrow_mut() = Some(Handlers { menu: Box::new(menu), event: Box::new(event) }));
        let tray = SniTray { tip: tip.to_string(), icon: None, hidden: false, menu: items };
        match tray.spawn() {
            Ok(handle) => HANDLE.with(|h| *h.borrow_mut() = Some(handle)),
            Err(error) => eprintln!("No tray icon: {error}"),
        }
        watch();
        Ok(Tray { _private: () })
    }

    /// Shows the icon, or changes it.
    pub fn set_icon(&mut self, icon: Icon) {
        let icon = ksni::Icon { width: icon.size as i32, height: icon.size as i32, data: icon.argb };
        update(move |tray| tray.icon = Some(icon));
        update_menu();
    }

    /// Hides or shows the icon in the panel.
    pub fn set_hidden(&mut self, hidden: bool) {
        update(move |tray| tray.hidden = hidden);
    }

    /// Brings the tray up to date (on Windows, re-adds the icon after Explorer restarted).
    pub fn restore(&self) {
        update_menu();
    }

    /// A desktop notification (org.freedesktop.Notifications), sent from a thread so the UI doesn't wait for it.
    pub fn notify(&self, title: &str, text: &str) {
        let (title, text) = (title.to_string(), text.to_string());
        spawn("Notification", move || {
            let send = || -> zbus::Result<()> {
                let connection = zbus::blocking::Connection::session()?;
                let hints: HashMap<&str, zbus::zvariant::Value> = HashMap::new();
                connection.call_method(
                    Some("org.freedesktop.Notifications"),
                    "/org/freedesktop/Notifications",
                    Some("org.freedesktop.Notifications"),
                    "Notify",
                    &("Unofficial Uniwill Control Center", 0u32, "unofficial-uniwill-control-center", title.as_str(), text.as_str(), Vec::<&str>::new(), hints, 5000i32),
                )?;
                Ok(())
            };
            let _ = send();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_pixels_become_argb() {
        let icon = make_icon(&[[255, 0, 0, 255]], 1);
        assert_eq!((icon.size, icon.argb), (1, vec![255, 255, 0, 0]));
    }

    #[test]
    fn menu_items_map_to_ksni() {
        let items = [MenuItem::item(1, "Open"), MenuItem::check(2, "Quiet", true, true), MenuItem::check(3, "Turbo", false, false)];
        let menu = ksni_menu(&snapshot(&items));
        assert_eq!(menu.len(), 3);
        match &menu[0] {
            ksni::MenuItem::Standard(item) => assert_eq!((item.label.as_str(), item.enabled), ("Open", true)),
            _ => panic!("item 0 should be a standard item"),
        }
        match &menu[1] {
            ksni::MenuItem::Checkmark(item) => assert_eq!((item.label.as_str(), item.checked, item.enabled), ("Quiet", true, true)),
            _ => panic!("item 1 should be a checkmark"),
        }
        match &menu[2] {
            ksni::MenuItem::Checkmark(item) => assert_eq!((item.label.as_str(), item.checked, item.enabled), ("Turbo", false, false)),
            _ => panic!("item 2 should be a checkmark"),
        }
    }

    #[test]
    fn ac_change_is_reported_once() {
        let mut last = None;
        assert!(!ac_changed(&mut last, true));
        assert!(!ac_changed(&mut last, true));
        assert!(ac_changed(&mut last, false));
        assert!(!ac_changed(&mut last, false));
    }
}
