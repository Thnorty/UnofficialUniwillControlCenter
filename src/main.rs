//! Tulpar Control: the Monster Tulpar T5 V21.4's keyboard and light bar lighting, performance modes, fans, battery
//! protection and a hardware monitor, replacing Control
//! Center's lighting pages, with Fn keys, pop-ups, effects drawn by the app, a timer and indicators.
#![windows_subsystem = "windows"]

mod app;
mod color;
mod hardware;
mod lighting;
mod settings;
mod shell;

pub mod ui {
    slint::include_modules!();
}

use hardware::hotkeys;
use shell::tray;
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
use windows::core::{HSTRING, w};

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    let has = |flag: &str| arguments.iter().any(|a| a.eq_ignore_ascii_case(flag));
    let demo = has("--demo");
    let start_hidden = has("--tray");

    // Two instances would send interleaved commands to the same controllers; bring the running one forward instead.
    if !demo {
        let _mutex = unsafe { CreateMutexW(None, true, w!("Local\\TulparControl")) };
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            tray::signal_running_instance();
            return;
        }
        std::mem::forget(_mutex); // held until the process ends
    }

    // Never leave the firmware's hotkeys pointed at a process that is gone.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        hotkeys::release_current();
        let message = format!("{info}");
        unsafe {
            MessageBoxW(None, &HSTRING::from(message), w!("Tulpar Control crashed"), MB_OK | MB_ICONERROR);
        }
        default_hook(info);
    }));

    if let Err(e) = run(demo, start_hidden) {
        hotkeys::release_current();
        unsafe {
            MessageBoxW(None, &HSTRING::from(format!("Tulpar Control couldn't start: {e}")), w!("Tulpar Control"), MB_OK | MB_ICONERROR);
        }
    }
    hotkeys::release_current();
}

fn run(demo: bool, start_hidden: bool) -> Result<(), slint::PlatformError> {
    // Skia draws through Direct3D. The OpenGL renderer (femtovg) shows a blank white window on some drivers, so it
    // isn't used; the software renderer is the fallback (it has no shadows).
    let renderer = std::env::var("TULPAR_RENDERER").unwrap_or_else(|_| "skia".into());
    let select = |name: &str| slint::BackendSelector::new().backend_name("winit".into()).renderer_name(name.into()).select();
    if select(&renderer).is_err() {
        select("software")?;
    }
    let window = ui::AppWindow::new()?;
    if let Some(page) = std::env::args().skip_while(|a| a != "--page").nth(1).and_then(|p| p.parse().ok()) {
        window.set_page(page);
    }
    app::App::start(&window, demo, start_hidden);
    // Closing the window only hides it; the app lives in the tray until Exit.
    slint::run_event_loop_until_quit()
}
