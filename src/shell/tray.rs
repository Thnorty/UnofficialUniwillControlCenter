//! The tray icon and a hidden window for system messages: the tray's menu and clicks, notifications, resume from
//! sleep, theme changes, and a second launch asking this one to show itself.

use std::cell::RefCell;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateBitmap, CreateDIBSection, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, HGDIOBJ};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_STATE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIS_HIDDEN,
    NOTIFY_ICON_STATE, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, AppendMenuW, ChangeWindowMessageFilterEx, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu, GetCursorPos, HICON, HMENU,
    FindWindowW, ICONINFO, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSGFLT_ALLOW, PostMessageW, RegisterClassW, RegisterWindowMessageW,
    SetForegroundWindow, TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WINDOW_EX_STYLE, WM_APP, WM_CONTEXTMENU,
    WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NULL, WM_POWERBROADCAST, WM_SETTINGCHANGE, WNDCLASSW, WS_POPUP,
};
use windows::core::{HSTRING, PCSTR, PCWSTR, w};

pub use super::tray_common::{MenuItem, TrayEvent, zone_pixels};

const CALLBACK: u32 = WM_APP + 1;
const NIN_SELECT: u32 = 0x400; // WM_USER: a click on the icon (NOTIFYICON_VERSION_4)
const NIN_KEYSELECT: u32 = 0x401;
const PBT_APMRESUMEAUTOMATIC: usize = 0x12;
const PBT_APMPOWERSTATUSCHANGE: usize = 0x0A;

struct Handlers {
    menu: Box<dyn Fn() -> Vec<MenuItem>>,
    event: Box<dyn Fn(TrayEvent)>,
}

thread_local! {
    static HANDLERS: RefCell<Option<Handlers>> = const { RefCell::new(None) };
    static MESSAGES: RefCell<(u32, u32, u32)> = const { RefCell::new((0, 0, 0)) }; // TaskbarCreated, show, exit
}

pub struct Tray {
    hwnd: HWND,
    icon: HICON,
    tip: String,
    /// The icon is kept but hidden: notifications still show through it, and a second launch still opens the window.
    hidden: bool,
}

/// The message a second launch broadcasts so the running instance shows itself.
pub fn show_message() -> u32 {
    // The names from when the app was called Tulpar Control, so an installer can still close a copy of then.
    unsafe { RegisterWindowMessageW(w!("TulparControl.Show")) }
}

/// The message `--exit` broadcasts so the running instance quits.
pub fn exit_message() -> u32 {
    unsafe { RegisterWindowMessageW(w!("TulparControl.Exit")) }
}

const CLASS: windows::core::PCWSTR = w!("KeyboardRgbTray");

/// Asks a running instance to show its window.
pub fn signal_running_instance() {
    allow_foreground();
    broadcast(show_message());
}

/// Asks the running instance, if there is one, to show its window; also from a process without administrator rights,
/// as the instance lets its show message through. False if none is running.
pub fn show_running() -> bool {
    unsafe {
        let Ok(hwnd) = FindWindowW(CLASS, None) else { return false };
        allow_foreground();
        PostMessageW(Some(hwnd), show_message(), WPARAM(0), LPARAM(0)).is_ok()
    }
}

/// Lets the running instance bring its window to the front: Windows allows that to the process the user just started.
fn allow_foreground() {
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
}

/// Asks a running instance to quit.
pub fn signal_exit() {
    broadcast(exit_message());
}

fn broadcast(message: u32) {
    unsafe {
        let _ = PostMessageW(Some(HWND(0xFFFF as _)), message, WPARAM(0), LPARAM(0)); // HWND_BROADCAST
    }
}

impl Tray {
    pub fn create(tip: &str, menu: impl Fn() -> Vec<MenuItem> + 'static, event: impl Fn(TrayEvent) + 'static) -> windows::core::Result<Tray> {
        HANDLERS.with(|h| *h.borrow_mut() = Some(Handlers { menu: Box::new(menu), event: Box::new(event) }));
        unsafe {
            MESSAGES.with(|m| *m.borrow_mut() = (RegisterWindowMessageW(w!("TaskbarCreated")), show_message(), exit_message()));
            allow_dark_menus();
            let instance = GetModuleHandleW(None)?;
            let wc = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: instance.into(), lpszClassName: CLASS, ..Default::default() };
            RegisterClassW(&wc);
            // A hidden top-level window rather than a message-only one, so it gets broadcasts.
            let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), CLASS, w!("Unofficial Uniwill Control Center tray"), WS_POPUP, 0, 0, 0, 0, None, None, Some(instance.into()), None)?;
            // A launch without administrator rights may ask this window to show the app (not to quit it).
            let _ = ChangeWindowMessageFilterEx(hwnd, show_message(), MSGFLT_ALLOW, None);
            let tray = Tray { hwnd, icon: HICON::default(), tip: tip.to_string(), hidden: false };
            Ok(tray)
        }
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_STATE,
            uCallbackMessage: CALLBACK,
            hIcon: self.icon,
            dwState: if self.hidden { NIS_HIDDEN } else { NOTIFY_ICON_STATE(0) },
            dwStateMask: NIS_HIDDEN,
            ..Default::default()
        };
        copy_wide(&mut data.szTip, &self.tip);
        data
    }

    fn add(&self) {
        unsafe {
            let mut data = self.data();
            let _ = Shell_NotifyIconW(NIM_ADD, &data);
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
        }
    }

    /// Shows the icon, or changes it.
    pub fn set_icon(&mut self, icon: HICON) {
        let old = std::mem::replace(&mut self.icon, icon);
        unsafe {
            if old.is_invalid() {
                self.add();
            } else {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &self.data());
                let _ = DestroyIcon(old);
            }
        }
    }

    /// Hides or shows the icon in the notification area.
    pub fn set_hidden(&mut self, hidden: bool) {
        if self.hidden == hidden {
            return;
        }
        self.hidden = hidden;
        if !self.icon.is_invalid() {
            unsafe {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &self.data());
            }
        }
    }

    /// Re-adds the icon after Explorer restarted.
    pub fn restore(&self) {
        if !self.icon.is_invalid() {
            self.add();
        }
    }

    /// A Windows notification from the tray icon.
    pub fn notify(&self, title: &str, text: &str) {
        let mut data = self.data();
        data.uFlags = NIF_INFO;
        data.dwInfoFlags = NIIF_INFO;
        copy_wide(&mut data.szInfoTitle, title);
        copy_wide(&mut data.szInfo, text);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data());
            if !self.icon.is_invalid() {
                let _ = DestroyIcon(self.icon);
            }
        }
    }
}

fn copy_wide(destination: &mut [u16], text: &str) {
    let wide: Vec<u16> = text.encode_utf16().take(destination.len() - 1).collect();
    destination[..wide.len()].copy_from_slice(&wide);
    destination[wide.len()] = 0;
}

/// Lets popup menus follow the dark theme (uxtheme's SetPreferredAppMode, which Explorer and Settings use too).
unsafe fn allow_dark_menus() {
    unsafe {
        let Ok(uxtheme) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        if let Some(set_preferred_app_mode) = GetProcAddress(uxtheme, PCSTR(135 as *const u8)) {
            let set: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(set_preferred_app_mode);
            set(1); // AllowDark: follow the system setting
        }
        if let Some(flush) = GetProcAddress(uxtheme, PCSTR(136 as *const u8)) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(flush);
            flush();
        }
    }
}

fn build_menu(items: &[MenuItem]) -> HMENU {
    unsafe {
        let menu = CreatePopupMenu().unwrap_or_default();
        for item in items {
            match item {
                MenuItem::Separator => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
                MenuItem::Item { id, text, checked, enabled, .. } => {
                    let mut flags = MF_STRING;
                    if *checked {
                        flags |= MF_CHECKED;
                    }
                    if !*enabled {
                        flags |= MF_GRAYED;
                    }
                    let _ = AppendMenuW(menu, flags, *id as usize, &HSTRING::from(text.as_str()));
                }
                MenuItem::Submenu { text, enabled, items } => {
                    let submenu = build_menu(items);
                    let mut flags = MF_STRING | MF_POPUP;
                    if !*enabled {
                        flags |= MF_GRAYED;
                    }
                    let _ = AppendMenuW(menu, flags, submenu.0 as usize, &HSTRING::from(text.as_str()));
                }
            }
        }
        menu
    }
}

fn emit(event: TrayEvent) {
    // Take the handlers out while calling them, so a handler can't re-enter.
    let handlers = HANDLERS.with(|h| h.borrow_mut().take());
    if let Some(handlers) = handlers {
        (handlers.event)(event);
        HANDLERS.with(|h| {
            let mut slot = h.borrow_mut();
            if slot.is_none() {
                *slot = Some(handlers);
            }
        });
    }
}

fn show_menu(hwnd: HWND) {
    let items = HANDLERS.with(|h| h.borrow().as_ref().map(|handlers| (handlers.menu)()));
    let Some(items) = items else { return };
    unsafe {
        let menu = build_menu(&items);
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        // The window must be in front, or the menu doesn't close when clicking elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let command = TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN).0, point.x, point.y, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        if command.0 != 0 {
            emit(TrayEvent::Command(command.0 as u32));
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let (taskbar_created, show, exit) = MESSAGES.with(|m| *m.borrow());
    match message {
        CALLBACK => {
            match (lparam.0 as u32) & 0xFFFF {
                NIN_SELECT | NIN_KEYSELECT | WM_LBUTTONUP | WM_LBUTTONDBLCLK => emit(TrayEvent::Open),
                WM_CONTEXTMENU => show_menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            if wparam.0 == PBT_APMRESUMEAUTOMATIC {
                emit(TrayEvent::Resumed);
            } else if wparam.0 == PBT_APMPOWERSTATUSCHANGE {
                emit(TrayEvent::PowerChanged);
            }
            LRESULT(1)
        }
        WM_SETTINGCHANGE => {
            let area = if lparam.0 != 0 { unsafe { PCWSTR(lparam.0 as *const u16).to_string().unwrap_or_default() } } else { String::new() };
            if area == "ImmersiveColorSet" {
                emit(TrayEvent::ThemeChanged);
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        m if m != 0 && m == show => {
            emit(TrayEvent::Open);
            LRESULT(0)
        }
        m if m != 0 && m == exit => {
            emit(TrayEvent::Exit);
            LRESULT(0)
        }
        m if m != 0 && m == taskbar_created => {
            emit(TrayEvent::ThemeChanged); // the app re-adds the icon (with the current colors) when this arrives
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// A Windows icon from RGBA pixels.
pub fn make_icon(pixels: &[[u8; 4]], size: usize) -> HICON {
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32),
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(color) = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) else { return HICON::default() };
        let out = std::slice::from_raw_parts_mut(bits as *mut [u8; 4], size * size);
        for (o, p) in out.iter_mut().zip(pixels) {
            *o = [p[2], p[1], p[0], p[3]]; // BGRA
        }
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, None);
        let icon_info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&icon_info).unwrap_or_default();
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        icon
    }
}
