//! Reports where on the keyboard each key press is, from 0 (left edge) to 1 (right edge, the numpad), for the light
//! bar's Typing effect. Only key positions are looked at, nothing is recorded, and the hook is only installed while
//! an effect needs it.
//!
//! The hook runs on a thread of its own with its own message loop, so a busy UI thread can't make Windows skip it.
//!
//! Windows doesn't call low-level keyboard hooks at all while the foreground window belongs to a process that also
//! registered for raw keyboard input, which winit does for its device events. Slint doesn't use those, so
//! `drop_raw_keyboard_input` takes the registration back; without it, Typing went dark whenever this window had
//! focus.

#[cfg(windows)]
use std::cell::RefCell;
#[cfg(windows)]
use std::collections::{HashMap, HashSet};
#[cfg(windows)]
use std::sync::mpsc;
#[cfg(windows)]
use std::thread::JoinHandle;

#[cfg(windows)]
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
#[cfg(windows)]
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
#[cfg(windows)]
use windows::Win32::System::Threading::GetCurrentThreadId;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

#[cfg(windows)]
const EXTENDED: u32 = 0x100;
// The Tulpar's layout in key widths: the main block is 15 wide (arrows and navigation keys at its right edge), the
// numpad four more.
const WIDTH: f32 = 19.0;

#[cfg(windows)]
struct HookState {
    pressed: Box<dyn Fn(f32)>,
    down: HashSet<u32>,
    positions: HashMap<u32, f32>,
}

#[cfg(windows)]
thread_local! {
    static STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

#[cfg(windows)]
pub struct KeyboardHook {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

#[cfg(windows)]
impl KeyboardHook {
    /// Installs the hook on its own thread, or returns None if Windows refused it. `pressed` is called on that thread.
    pub fn install(pressed: impl Fn(f32) + Send + 'static) -> Option<KeyboardHook> {
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("Keyboard hook".into())
            .spawn(move || {
                STATE.with(|s| *s.borrow_mut() = Some(HookState { pressed: Box::new(pressed), down: HashSet::new(), positions: positions() }));
                let module = unsafe { GetModuleHandleW(None) }.ok().map(|m| HINSTANCE(m.0));
                let Ok(hook) = (unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(on_key), module, 0) }) else {
                    ready_tx.send(None).ok();
                    return;
                };
                ready_tx.send(Some(unsafe { GetCurrentThreadId() })).ok();
                let mut message = MSG::default();
                // Until WM_QUIT (0) or an error (-1).
                while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
                    unsafe {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                unsafe {
                    let _ = UnhookWindowsHookEx(hook);
                }
            })
            .ok()?;
        match ready_rx.recv() {
            Ok(Some(thread_id)) => Some(KeyboardHook { thread_id, thread: Some(thread) }),
            _ => {
                let _ = thread.join();
                None
            }
        }
    }
}

#[cfg(windows)]
impl Drop for KeyboardHook {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn on_key(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code >= 0 {
        let key = unsafe { &*(data.0 as *const KBDLLHOOKSTRUCT) };
        let id = (key.scanCode & 0xFF) | if key.flags.0 & LLKHF_EXTENDED.0 != 0 { EXTENDED } else { 0 };
        let message = message.0 as u32;
        // try_borrow: never let an effect get in the way of typing.
        STATE.with(|s| {
            if let Ok(mut state) = s.try_borrow_mut()
                && let Some(state) = state.as_mut()
            {
                if message == WM_KEYDOWN || message == WM_SYSKEYDOWN {
                    if state.down.insert(id) {
                        // not auto-repeat
                        let x = state.positions.get(&id).map_or(0.5, |x| x / WIDTH);
                        (state.pressed)(x);
                    }
                } else if message == WM_KEYUP || message == WM_SYSKEYUP {
                    state.down.remove(&id);
                }
            }
        });
    }
    unsafe { CallNextHookEx(None, code, message, data) }
}

#[cfg(windows)]
/// Removes this process's raw keyboard input registration (winit's, for device events). Call after the event loop is
/// created.
pub fn drop_raw_keyboard_input() {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Input::{RAWINPUTDEVICE, RIDEV_REMOVE, RegisterRawInputDevices};
    // Generic desktop page, keyboard usage; removing needs no target window.
    let keyboard = RAWINPUTDEVICE { usUsagePage: 1, usUsage: 6, dwFlags: RIDEV_REMOVE, hwndTarget: HWND::default() };
    unsafe {
        let _ = RegisterRawInputDevices(&[keyboard], size_of::<RAWINPUTDEVICE>() as u32);
    }
}

/// Which of the keyboard's four zones a position from the hook is in (the zones are 5, 5, 5.25 and 3.75 keys wide).
pub fn zone_of(position: f32) -> usize {
    match position * WIDTH {
        x if x < 5.0 => 0,
        x if x < 10.0 => 1,
        x if x < 15.25 => 2,
        _ => 3,
    }
}

#[cfg(windows)]
fn positions() -> HashMap<u32, f32> {
    let mut map = HashMap::new();
    let mut row = |first: u32, count: u32, first_x: f32, step: f32| {
        for i in 0..count {
            map.insert(first + i, first_x + i as f32 * step);
        }
    };
    // Function row: F1-F10 (Esc, F11 and F12 below).
    row(0x3B, 10, 1.6, 1.05);
    // Number row: 1-0, -, =.
    row(0x02, 12, 1.5, 1.0);
    // Q row: Q-].
    row(0x10, 12, 2.0, 1.0);
    // A row: A-'.
    row(0x1E, 11, 2.25, 1.0);
    // Z row: Z-/.
    row(0x2C, 10, 2.75, 1.0);
    // Numpad digits: 7 8 9, 4 5 6, 1 2 3.
    row(0x47, 3, 15.5, 1.0);
    row(0x4B, 3, 15.5, 1.0);
    row(0x4F, 3, 15.5, 1.0);
    let singles: &[(u32, f32)] = &[
        (0x01, 0.5),             // Esc
        (0x57, 12.1),            // F11
        (0x58, 13.15),           // F12
        (0x37 | EXTENDED, 14.2), // Print Screen
        (0x29, 0.5),             // `
        (0x0E, 14.0),            // Backspace
        (0x0F, 0.75),            // Tab
        (0x2B, 14.25),           // \
        (0x3A, 0.9),             // Caps Lock
        (0x1C, 14.0),            // Enter
        (0x2A, 1.1),             // left Shift
        (0x36, 13.25),           // right Shift
        (0x1D, 0.6),             // left Ctrl
        (0x5B | EXTENDED, 2.4),  // Windows
        (0x38, 3.5),             // left Alt
        (0x39, 7.25),            // Space
        (0x38 | EXTENDED, 10.5), // right Alt
        (0x5D | EXTENDED, 11.25), // Menu
        (0x1D | EXTENDED, 11.75), // right Ctrl
        (0x4B | EXTENDED, 12.75), // Left
        (0x48 | EXTENDED, 13.75), // Up
        (0x50 | EXTENDED, 13.75), // Down
        (0x4D | EXTENDED, 14.75), // Right
        (0x52 | EXTENDED, 14.5), // Insert
        (0x53 | EXTENDED, 14.5), // Delete
        (0x47 | EXTENDED, 14.5), // Home
        (0x4F | EXTENDED, 14.5), // End
        (0x49 | EXTENDED, 14.5), // Page Up
        (0x51 | EXTENDED, 14.5), // Page Down
        (0x45, 15.5),            // Num Lock
        (0x35 | EXTENDED, 16.5), // numpad /
        (0x37, 17.5),            // numpad *
        (0x4A, 18.5),            // numpad -
        (0x4E, 18.5),            // numpad +
        (0x1C | EXTENDED, 18.5), // numpad Enter
        (0x52, 16.0),            // numpad 0
        (0x53, 17.5),            // numpad .
    ];
    for &(id, x) in singles {
        map.insert(id, x);
    }
    map
}

/// The Typing effect comes later on Linux: no hook yet.
#[cfg(target_os = "linux")]
pub struct KeyboardHook;

#[cfg(target_os = "linux")]
impl KeyboardHook {
    pub fn install(_pressed: impl Fn(f32) + Send + 'static) -> Option<KeyboardHook> {
        None
    }
}

#[cfg(target_os = "linux")]
pub fn drop_raw_keyboard_input() {}
