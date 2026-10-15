//! Unofficial Uniwill Control Center: keyboard and light bar lighting, performance modes, fans, battery care and a hardware monitor for laptops
//! built by Uniwill, replacing their Control Center, with Fn keys, pop-ups, effects drawn by the app, a timer and
//! plugins. Made on a Monster Tulpar T5 V21.4.
#![windows_subsystem = "windows"]

mod app;
mod cli;
mod color;
mod hardware;
mod lighting;
mod plugins;
mod settings;
mod shell;

pub mod ui {
    slint::include_modules!();
}

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    let has = |flag: &str| arguments.iter().any(|a| a.eq_ignore_ascii_case(flag));
    let demo = has("--demo");
    let start_hidden = has("--tray");

    // For the installer: no window, just the job, and an exit code.
    if has("--exit") {
        std::process::exit(if shell::instance::close_running(std::time::Duration::from_secs(30)) { 0 } else { 1 });
    }
    // When the uninstaller is asked to remove everything. It closes the app first ("--exit"), which would write them again.
    if has("--remove-data") {
        let hooks = plugins::claude::remove_all().is_ok();
        let folders = settings::remove_folders();
        std::process::exit(if hooks && folders { 0 } else { 1 });
    }
    if has("--enable-startup") || has("--disable-startup") {
        let result = if has("--enable-startup") { hardware::system::startup::enable() } else { hardware::system::startup::disable() };
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }

    // The manifest doesn't ask for administrator rights, so opening the app while it runs needs no UAC prompt.
    if !demo && !shell::instance::is_elevated() {
        shell::instance::open_elevated(&arguments[1..]);
        return;
    }
    if !demo && !shell::instance::acquire() {
        return;
    }
    if !demo {
        settings::migrate_folder();
        if !accept_risk() {
            return;
        }
    }

    // Never leave the firmware's hotkeys pointed at a process that is gone, or the fans on a curve nothing watches.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        hardware::release_current();
        shell::error_box("Unofficial Uniwill Control Center crashed", &format!("{info}"));
        default_hook(info);
    }));

    if let Err(e) = run(demo, start_hidden) {
        hardware::release_current();
        shell::error_box("Unofficial Uniwill Control Center", &format!("Unofficial Uniwill Control Center couldn't start: {e}"));
    }
    hardware::release_current();
}

/// The laptop the app was made and tested on.
const TESTED: (&str, &str) = ("MONSTER", "TULPAR T5 V21.4");

/// Asks once, before anything is sent to the laptop, whether to go on: the app is unofficial, and on laptops other
/// than the one it was tested on, its writes to the embedded controller are untested.
fn accept_risk() -> bool {
    let mut settings = settings::Settings::load();
    if settings.accepted_risk {
        return true;
    }
    let (maker, model) = hardware::system::laptop_model();
    let tested = is_tested(&maker, &model);
    if !shell::warning_ok_cancel("Unofficial Uniwill Control Center", &risk_text(&maker, &model), !tested) {
        return false;
    }
    settings.accepted_risk = true;
    settings.save();
    true
}

fn is_tested(maker: &str, model: &str) -> bool {
    maker.eq_ignore_ascii_case(TESTED.0) && model.eq_ignore_ascii_case(TESTED.1)
}

/// The question `accept_risk` asks, for the laptop it runs on.
fn risk_text(maker: &str, model: &str) -> String {
    let text = if is_tested(maker, model) {
        "This app is unofficial: it isn't made or supported by Uniwill or by your laptop's maker.\n\n\
         It changes the fans, the performance modes, the power limits and battery care by writing to the laptop's \
         embedded controller. It was made and tested on this laptop, the Monster Tulpar T5 V21.4.\n\n\
         Use it at your own risk. Start it?"
            .to_string()
    } else {
        let laptop = if model.is_empty() { "This laptop".to_string() } else { format!("This laptop ({maker} {model})") };
        format!(
            "This app is unofficial: it isn't made or supported by Uniwill or by your laptop's maker.\n\n\
             It changes the fans, the performance modes, the power limits and battery care by writing to the laptop's \
             embedded controller. It was only tested on a Monster Tulpar T5 V21.4. {laptop} is untested: it may be \
             built differently, and wrong writes to the embedded controller could make it misbehave or damage it.\n\n\
             Use it at your own risk. Start it anyway?"
        )
    };
    if cfg!(target_os = "linux") {
        text.replacen("embedded controller. ", "embedded controller. On Linux it reaches the embedded controller through a small helper that runs as root. ", 1)
    } else {
        text
    }
}

/// The window's Wayland app id (X11 class): the desktop entry's name, so the desktop shows the app's icon for it.
#[cfg(target_os = "linux")]
const LINUX_APP_ID: &str = "unofficial-uniwill-control-center";

fn run(demo: bool, start_hidden: bool) -> Result<(), slint::PlatformError> {
    // Skia draws through Direct3D. The OpenGL renderer (femtovg) shows a blank white window on some drivers, so it
    // isn't used; the software renderer is the fallback (it has no shadows).
    let renderer = std::env::var("UUCC_RENDERER").unwrap_or_else(|_| "skia".into());
    let select = |name: &str| slint::BackendSelector::new().backend_name("winit".into()).renderer_name(name.into()).select();
    if select(&renderer).is_err() {
        select("software")?;
    }
    #[cfg(target_os = "linux")]
    slint::set_xdg_app_id(LINUX_APP_ID)?;
    let window = ui::AppWindow::new()?;
    if let Some(page) = std::env::args().skip_while(|a| a != "--page").nth(1).and_then(|p| p.parse().ok()) {
        window.set_page(page);
    }
    app::App::start(&window, demo, start_hidden);
    // Closing the window only hides it; the app lives in the tray until Exit.
    slint::run_event_loop_until_quit()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn risk_text_mentions_the_helper() {
        for (maker, model) in [("MONSTER", "TULPAR T5 V21.4"), ("Other", "Laptop")] {
            let text = super::risk_text(maker, model);
            assert!(
                text.contains("embedded controller. On Linux it reaches the embedded controller through a small helper that runs as root. "),
                "{text}"
            );
        }
    }

    #[test]
    fn app_id_names_the_desktop_entry() {
        // Plasma finds the window's icon (Alt+Tab, the task bar) through the desktop entry the app id names.
        let entry = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("packaging/linux/{}.desktop", super::LINUX_APP_ID));
        assert!(entry.exists(), "{}", entry.display());
    }
}
