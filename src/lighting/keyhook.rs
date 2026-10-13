//! Reports where on the keyboard each key press is, from 0 (left edge) to 1 (right edge, the numpad), for the light
//! bar's Typing effect. Only key positions are looked at, nothing is recorded, and the hook is only installed while
//! an effect needs it. It must be installed on a thread with a message loop (the UI thread).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP,
    WM_SYSKEYDOWN, WM_SYSKEYUP,
};

const EXTENDED: u32 = 0x100;
// The Tulpar's layout in key widths: the main block is 15 wide (arrows and navigation keys at its right edge), the
// numpad four more.
const WIDTH: f32 = 19.0;

struct HookState {
    pressed: Box<dyn Fn(f32)>,
    down: HashSet<u32>,
    positions: HashMap<u32, f32>,
}

thread_local! {
    static STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

pub struct KeyboardHook(HHOOK);

impl KeyboardHook {
    /// Installs the hook, or returns None if Windows refused it.
    pub fn install(pressed: impl Fn(f32) + 'static) -> Option<KeyboardHook> {
        STATE.with(|s| *s.borrow_mut() = Some(HookState { pressed: Box::new(pressed), down: HashSet::new(), positions: positions() }));
        let module = unsafe { GetModuleHandleW(None) }.ok().map(|m| HINSTANCE(m.0));
        match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(on_key), module, 0) } {
            Ok(hook) => Some(KeyboardHook(hook)),
            Err(_) => {
                STATE.with(|s| *s.borrow_mut() = None);
                None
            }
        }
    }
}

impl Drop for KeyboardHook {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWindowsHookEx(self.0);
        }
        STATE.with(|s| *s.borrow_mut() = None);
    }
}

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
