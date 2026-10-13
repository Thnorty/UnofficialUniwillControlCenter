//! The app: the two lighting devices and what they show, the window, the tray, the Fn keys, the idle timer, the
//! timer and the indicators; the performance mode, fans and battery care (power.rs) and the readings (monitor.rs).
//! Everything here runs on the UI thread; the effects, EC, hotkey, monitor and brightness-watching threads post back
//! to it with slint::invoke_from_event_loop.

mod monitor;
mod power;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SW_RESTORE, SetForegroundWindow, ShowWindow};

use crate::color::Rgb;
use crate::lighting::device::{Device, Kind, MAX_LIGHT};
use crate::lighting::effects::{self, Drawn, Effect, EffectColors, Group};
use crate::lighting::engine::{Engine, Inputs, Target};
use crate::hardware::battery::ChargeMode;
use crate::hardware::hotkeys::{self, Hotkeys};
use crate::hardware::performance::Mode;
use crate::lighting::keyhook::KeyboardHook;
use crate::shell::osd::{self, OsdWindow};
use crate::lighting::preview;
use crate::lighting::scenes::{self, Live, LightBarRenderer, Overlay, OverlayStyle, Scene};
use crate::settings::{LightingSettings, Settings};
use crate::hardware::system::{self, startup, touchpad};
use crate::shell::tray::{self, MenuItem, Tray, TrayEvent};
use crate::ui::{AppWindow, DeviceView, Theme, TileGroup};

// Control Center's "Idle Timer" (KeyboardCloseTimer): switch the lights off after this long without input.
const IDLE_TIMEOUTS: &[(&str, u32)] = &[
    ("Never", 0),
    ("15 seconds", 15),
    ("30 seconds", 30),
    ("1 minute", 60),
    ("2 minutes", 120),
    ("5 minutes", 300),
    ("10 minutes", 600),
    ("15 minutes", 900),
    ("30 minutes", 1800),
    ("1 hour", 3600),
];

/// The keyboard brightness the Fn keys step through, in percent. From a value in between they go to the next one.
const FN_KEY_STOPS: [u8; 7] = [0, 5, 20, 40, 60, 80, 100];

/// The controllers have 51 brightness levels (0-50). The app works in percent and sends the nearest level, so two
/// neighbouring percentages can look the same.
fn to_level(percent: u8) -> u8 {
    ((percent.min(100) as u16 * MAX_LIGHT as u16 + 50) / 100) as u8
}

fn to_percent(level: u8) -> u8 {
    (level.min(MAX_LIGHT) as u16 * 100 / MAX_LIGHT as u16) as u8
}
const FADE_TIME: Duration = Duration::from_millis(180);
const TIMER_MINUTES: &[u32] = &[5, 10, 15, 25, 30, 45, 60];
const TIMER_COLOR: Rgb = Rgb::new(255, 110, 0);
const CAPS_COLOR: Rgb = Rgb::new(255, 255, 255);
const NUM_COLOR: Rgb = Rgb::new(0, 140, 255);
const TILE_LEDS: usize = 8;

const PRESETS: &[Rgb] = &[
    Rgb::new(255, 0, 0),
    Rgb::new(255, 80, 0),
    Rgb::new(255, 200, 0),
    Rgb::new(0, 255, 0),
    Rgb::new(0, 255, 160),
    Rgb::new(0, 200, 255),
    Rgb::new(0, 40, 255),
    Rgb::new(140, 0, 255),
    Rgb::new(255, 0, 160),
    Rgb::new(255, 255, 255),
];

// Tray menu commands.
const CMD_KEYBOARD_EFFECT: u32 = 100;
const CMD_KEYBOARD_LIGHT: u32 = 200;
const CMD_LIGHT_BAR_POWER: u32 = 300;
const CMD_LIGHT_BAR_LINK: u32 = 301;
const CMD_LIGHT_BAR_EFFECT: u32 = 400;
const CMD_TIMER: u32 = 500;
const CMD_TIMER_CANCEL: u32 = 599;
const CMD_MODE: u32 = 600;
const CMD_FAN_BOOST: u32 = 610;
const CMD_OPEN: u32 = 900;
const CMD_EXIT: u32 = 999;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` with the app, unless it's already busy further up the stack (then it runs right after).
pub fn with_app(f: impl FnOnce(&mut App) + 'static) {
    let pending = APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut app) => {
            if let Some(app) = app.as_mut() {
                f(app);
            }
            None
        }
        Err(_) => Some(f),
    });
    if let Some(f) = pending {
        slint::Timer::single_shot(Duration::ZERO, move || with_app(f));
    }
}

/// Runs `f` with the app from another thread.
fn post(f: impl FnOnce(&mut App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with_app(f));
}

struct Fade {
    from: u8,
    target: u8,
    start: Instant,
    duration: Duration,
}

/// What a device should show: its own choices, or the keyboard's translated for the light bar.
struct Plan {
    effect: &'static Effect,
    colors: Vec<Rgb>,
    direction: u8,
    speed: u8,
}

/// One device's lighting: the settings shown in its page, and the state of what's sent.
struct Lighting {
    effects: &'static [Effect],
    device: Option<Arc<Device>>,
    on: bool,
    effect: usize,
    zones: [Rgb; 4],
    palettes: Vec<Vec<Rgb>>, // by effect index; empty for effects without a palette
    direction: u8,           // LM_ITE_RGB.Translate_ITE_DirectionValue: 1 = left to right, 2 = right to left
    percent: u8, // brightness, 0-100
    speed: u8,
    linked: bool,

    shown_light: u8, // the brightness the device is showing, including partway through a fade
    fade: Option<Fade>,
    idle_off: bool, // faded out by the idle timer until the next input
    renderer: Option<u64>, // the engine target drawing the light bar, if any
    flash: bool,           // that target is a flash shown over a firmware effect
    overlays: Arc<Mutex<Vec<Overlay>>>,
    live: Live,
    changed: bool, // something was sent since the last save to the controller
    pending: bool, // a change waits to be sent
    send_timer: slint::Timer,
}

impl Lighting {
    fn new(kind: Kind, settings: &LightingSettings) -> Lighting {
        let effects = match kind {
            Kind::Keyboard => effects::KEYBOARD,
            Kind::LightBar => effects::LIGHT_BAR,
        };
        let zones = if settings.zone_colors.len() == 4 {
            std::array::from_fn(|i| Rgb::from_argb(settings.zone_colors[i]))
        } else {
            [Rgb::new(255, 0, 0); 4]
        };
        let palettes = effects
            .iter()
            .map(|effect| match effect.colors {
                EffectColors::Palette => match settings.palettes.get(effect.key) {
                    Some(saved) if (effect.min_colors..=effect.max_colors).contains(&saved.len()) => saved.iter().map(|&c| Rgb::from_argb(c)).collect(),
                    _ => effect.default_colors.to_vec(),
                },
                _ => Vec::new(),
            })
            .collect();
        Lighting {
            effects,
            device: None,
            on: kind == Kind::Keyboard || settings.on,
            effect: effects.iter().position(|e| e.key == settings.effect).unwrap_or(0),
            zones,
            palettes,
            direction: if settings.wave_direction == 2 { 2 } else { 1 },
            percent: match settings.brightness_percent {
                Some(percent) if to_level(percent) == settings.light.min(MAX_LIGHT) => percent.min(100),
                _ => to_percent(settings.light),
            },
            speed: settings.speed.min(4),
            linked: kind == Kind::LightBar && settings.linked,
            shown_light: settings.light.min(MAX_LIGHT),
            fade: None,
            idle_off: false,
            renderer: None,
            flash: false,
            overlays: Arc::new(Mutex::new(Vec::new())),
            live: scenes::new_live(),
            changed: false,
            pending: false,
            send_timer: slint::Timer::default(),
        }
    }

    fn selected(&self) -> &'static Effect {
        &self.effects[self.effect]
    }

    fn colors_for(&self, index: usize) -> Vec<Rgb> {
        match self.effects[index].colors {
            EffectColors::Zones => self.zones.to_vec(),
            EffectColors::Palette => self.palettes[index].clone(),
            EffectColors::None => Vec::new(),
        }
    }

    fn store(&self, settings: &mut LightingSettings) {
        settings.on = self.on;
        settings.effect = self.selected().key.to_string();
        settings.zone_colors = self.zones.iter().map(|c| c.to_argb()).collect();
        settings.palettes = self
            .effects
            .iter()
            .zip(&self.palettes)
            .filter(|(e, _)| e.colors == EffectColors::Palette)
            .map(|(e, p)| (e.key.to_string(), p.iter().map(|c| c.to_argb()).collect()))
            .collect::<HashMap<_, _>>();
        settings.wave_direction = self.direction;
        settings.light = to_level(self.percent);
        settings.brightness_percent = Some(self.percent);
        settings.speed = self.speed;
        settings.linked = self.linked;
    }

    /// The colors the tray icon shows for the selected effect.
    fn icon_colors(&self) -> Vec<Rgb> {
        let effect = self.selected();
        match effect.colors {
            EffectColors::Palette => self.palettes[self.effect].clone(),
            _ if effect.is_rainbow() => vec![Rgb::new(255, 0, 0), Rgb::new(0, 255, 0), Rgb::new(0, 0, 255), Rgb::new(255, 0, 255)],
            EffectColors::None => effects::LIGHT_BAR_RAINBOW[..4].to_vec(),
            EffectColors::Zones => self.zones.to_vec(),
        }
    }
}

/// Whether this app has to draw the effect: one of its own, or the keyboard's zones on the light bar.
fn needs_app(kind: Kind, effect: &Effect) -> bool {
    effect.drawn_by_app() || smooth_wave(kind, effect) || (kind == Kind::LightBar && effect.colors == EffectColors::Zones)
}

/// The keyboard firmware's Wave moves the colors a whole zone at a time. While the app runs it draws Wave itself,
/// blending the colors across the zones as the preview shows; on exit the firmware's Wave takes over again.
fn smooth_wave(kind: Kind, effect: &Effect) -> bool {
    kind == Kind::Keyboard && effect.id == effects::WAVE_ID && effect.drawn == Drawn::Firmware
}

/// The colors the app flows across the keyboard's zones for Wave.
fn wave_colors(kind: Kind, plan: &Plan) -> Option<Vec<Rgb>> {
    smooth_wave(kind, plan.effect).then(|| plan.colors.clone())
}

/// What the light bar shows for an effect this app draws.
fn scene_for(plan: &Plan) -> Box<dyn Scene> {
    let (colors, speed) = (plan.colors.clone(), plan.speed);
    match plan.effect.drawn {
        Drawn::Music => Box::new(scenes::MusicMeterScene::new(colors, speed)),
        Drawn::Spectrum => Box::new(scenes::SpectrumScene::new(colors, speed)),
        Drawn::StarSpark => Box::new(scenes::StarSparkScene::new(colors, speed)),
        Drawn::StarHitting => Box::new(scenes::StarHittingScene::new(colors, speed)),
        Drawn::Typing => Box::new(scenes::TypingScene::new(colors, speed)),
        Drawn::Battery => Box::new(scenes::BatteryScene),
        Drawn::CpuLoad => Box::new(scenes::GaugeScene::new(Inputs::CPU_LOAD)),
        Drawn::CpuTemperature => Box::new(scenes::GaugeScene::new(Inputs::CPU_TEMPERATURE)),
        Drawn::GpuTemperature => Box::new(scenes::GaugeScene::new(Inputs::GPU_TEMPERATURE)),
        Drawn::Firmware => Box::new(scenes::StaticScene(scenes::zone_sections(&colors))), // the keyboard's zones, linked
    }
}

/// The models the window reads every frame, kept so they can be updated in place.
struct Models {
    kb_tile_leds: Rc<VecModel<slint::Color>>,
    lb_tile_leds: Rc<VecModel<slint::Color>>,
    kb_glow: Rc<VecModel<slint::Color>>,
    lb_leds: Rc<VecModel<slint::Color>>,
    kb_zones: Rc<VecModel<slint::Color>>,
    kb_palette: Rc<VecModel<slint::Color>>,
    lb_palette: Rc<VecModel<slint::Color>>,
}

/// Shared with the thread that watches for brightness changes the firmware makes on its own.
struct Watch {
    keyboard: Mutex<Option<Arc<Device>>>,
    busy: AtomicBool,
    stop: AtomicBool,
}

pub struct App {
    ui: slint::Weak<AppWindow>,
    demo: bool,
    settings: Settings,
    keyboard: Lighting,
    light_bar: Option<Lighting>,
    engine: Engine,
    osd: OsdWindow,
    tray: Option<Tray>,
    hotkeys: Option<Hotkeys>,
    hotkeys_starting: bool,
    fn_mode: String,
    idle_off: bool,
    last_hotkey_activity: Instant,
    idle_index: usize,
    timer_end: Option<Instant>,
    timer_length: u32,
    key_hook: Option<KeyboardHook>,
    lock_keys: Option<(bool, bool)>,
    last_touchpad: Option<bool>,
    winkey_locked: bool,
    models: Models,
    timers: Vec<slint::Timer>,
    fade_timer: slint::Timer,
    watch: Arc<Watch>,
    started: Instant,
    settings_dirty: bool,
    exiting: bool,
    power: power::Power,
    monitor: monitor::Monitor,
}

fn colors_model(colors: &[Rgb]) -> Rc<VecModel<slint::Color>> {
    Rc::new(VecModel::from(colors.iter().map(|c| c.to_slint()).collect::<Vec<_>>()))
}

fn set_colors(model: &VecModel<slint::Color>, colors: &[Rgb]) {
    if model.row_count() != colors.len() {
        model.set_vec(colors.iter().map(|c| c.to_slint()).collect::<Vec<_>>());
        return;
    }
    for (i, c) in colors.iter().enumerate() {
        let color = c.to_slint();
        if model.row_data(i) != Some(color) {
            model.set_row_data(i, color);
        }
    }
}

fn tile_names_and_groups(effects: &[Effect]) -> (Vec<SharedString>, Vec<TileGroup>) {
    let names = effects.iter().map(|e| SharedString::from(e.name)).collect();
    let mut groups: Vec<TileGroup> = Vec::new();
    let mut previous: Option<Group> = None;
    for (i, effect) in effects.iter().enumerate() {
        if previous != Some(effect.group) {
            groups.push(TileGroup { title: effect.group.title().into(), first: i as i32, count: 0 });
            previous = Some(effect.group);
        }
        groups.last_mut().unwrap().count += 1;
    }
    (names, groups)
}

impl App {
    /// Builds the app around the window and starts it. `demo` shows the window with simulated devices only.
    pub fn start(ui: &AppWindow, demo: bool, start_hidden: bool) {
        let settings = Settings::load();
        let keyboard = Lighting::new(Kind::Keyboard, settings.keyboard.as_ref().expect("loaded with a keyboard"));
        let idle_index = IDLE_TIMEOUTS.iter().position(|t| t.1 == settings.idle_timeout_seconds).unwrap_or(0);

        let models = Models {
            kb_tile_leds: colors_model(&vec![Rgb::BLACK; effects::KEYBOARD.len() * TILE_LEDS]),
            lb_tile_leds: colors_model(&vec![Rgb::BLACK; effects::LIGHT_BAR.len() * TILE_LEDS]),
            kb_glow: colors_model(&[Rgb::BLACK; 4]),
            lb_leds: colors_model(&[Rgb::BLACK; crate::lighting::device::LIGHT_BAR_LEDS]),
            kb_zones: colors_model(&keyboard.zones),
            kb_palette: colors_model(&[]),
            lb_palette: colors_model(&[]),
        };
        ui.set_kb_tile_leds(ModelRc::from(models.kb_tile_leds.clone()));
        ui.set_lb_tile_leds(ModelRc::from(models.lb_tile_leds.clone()));
        ui.set_kb_glow(ModelRc::from(models.kb_glow.clone()));
        ui.set_lb_leds(ModelRc::from(models.lb_leds.clone()));
        ui.set_kb_zones(ModelRc::from(models.kb_zones.clone()));
        ui.set_kb_palette(ModelRc::from(models.kb_palette.clone()));
        ui.set_lb_palette(ModelRc::from(models.lb_palette.clone()));
        let (names, groups) = tile_names_and_groups(effects::KEYBOARD);
        ui.set_kb_tiles(ModelRc::new(VecModel::from(names)));
        ui.set_kb_groups(ModelRc::new(VecModel::from(groups)));
        let (names, groups) = tile_names_and_groups(effects::LIGHT_BAR);
        ui.set_lb_tiles(ModelRc::new(VecModel::from(names)));
        ui.set_lb_groups(ModelRc::new(VecModel::from(groups)));
        ui.set_keys(ModelRc::new(VecModel::from(preview::keyboard_layout())));
        ui.set_presets(ModelRc::from(colors_model(PRESETS)));
        ui.set_idle_options(ModelRc::new(VecModel::from(IDLE_TIMEOUTS.iter().map(|t| SharedString::from(t.0)).collect::<Vec<_>>())));
        ui.set_idle_index(idle_index as i32);
        ui.set_flash_caps(settings.flash_caps_lock);
        ui.set_flash_num(settings.flash_num_lock);
        if !demo {
            startup::migrate();
        }
        ui.set_start_with_windows(!demo && startup::is_enabled());
        apply_theme(ui);

        let watch = Arc::new(Watch { keyboard: Mutex::new(None), busy: AtomicBool::new(false), stop: AtomicBool::new(false) });
        let power = power::Power::new(settings.power.as_ref(), demo);
        let app = App {
            ui: ui.as_weak(),
            demo,
            settings,
            keyboard,
            light_bar: None,
            engine: Engine::new(),
            osd: OsdWindow::new(),
            tray: None,
            hotkeys: None,
            hotkeys_starting: false,
            fn_mode: String::new(),
            idle_off: false,
            last_hotkey_activity: Instant::now() - Duration::from_secs(3600),
            idle_index,
            timer_end: None,
            timer_length: 0,
            key_hook: None,
            lock_keys: None,
            last_touchpad: None,
            winkey_locked: false,
            models,
            timers: Vec::new(),
            fade_timer: slint::Timer::default(),
            watch,
            started: Instant::now(),
            settings_dirty: false,
            exiting: false,
            power,
            monitor: monitor::Monitor::new(),
        };
        APP.with(|cell| *cell.borrow_mut() = Some(app));
        connect_callbacks(ui);

        with_app(move |app| {
            app.create_tray();
            app.refresh_device(Kind::Keyboard);
            app.refresh_settings_page();
            app.start_timers();
            app.start_monitor();
            if let Some(ui) = app.ui() {
                ui.set_curve(ModelRc::from(app.power.curve_model.clone()));
            }
            app.refresh_power();
            if !demo {
                app.start_brightness_watcher();
                app.connect_ec();
            }
        });
        crate::lighting::keyhook::drop_raw_keyboard_input();
        if !start_hidden {
            show_window(ui);
        }
        // Open the devices once the window is up.
        slint::Timer::single_shot(Duration::from_millis(30), || with_app(|app| app.open_devices()));
        // For trying the pop-up: --osd-test shows the brightness one, then a text one.
        if std::env::args().any(|a| a == "--osd-test") {
            slint::Timer::single_shot(Duration::from_millis(600), || with_app(|app| app.osd.show_brightness(0.6)));
            slint::Timer::single_shot(Duration::from_millis(1100), || with_app(|app| app.osd.show_brightness(0.8)));
            slint::Timer::single_shot(Duration::from_millis(4500), || with_app(|app| app.osd.show_light_bar(true)));
        }
    }

    fn ui(&self) -> Option<AppWindow> {
        self.ui.upgrade()
    }

    fn lighting(&self, kind: Kind) -> Option<&Lighting> {
        match kind {
            Kind::Keyboard => Some(&self.keyboard),
            Kind::LightBar => self.light_bar.as_ref(),
        }
    }

    fn lighting_mut(&mut self, kind: Kind) -> Option<&mut Lighting> {
        match kind {
            Kind::Keyboard => Some(&mut self.keyboard),
            Kind::LightBar => self.light_bar.as_mut(),
        }
    }

    // ---- Devices ------------------------------------------------------------------------------------------------

    fn open_devices(&mut self) {
        self.set_status("", false, false);
        if self.demo {
            if self.light_bar.is_none() {
                let settings = self.settings.light_bar.clone().unwrap_or_default();
                self.light_bar = Some(Lighting::new(Kind::LightBar, &settings));
                self.refresh_device(Kind::LightBar);
            }
            self.fn_mode = "Demo: no devices are used.".into();
            self.refresh_settings_page();
            self.set_status("Demo mode: nothing is sent to the keyboard or the light bar.", false, false);
            return;
        }
        if self.keyboard.device.is_none() {
            match Device::open(Kind::Keyboard) {
                Ok(Some(device)) => {
                    // Keep a nonzero brightness the keyboard reports, since the Fn keys may have changed it while this
                    // app wasn't running; a zero is more likely the lights having been reset.
                    if let Ok(light) = device.read_light()
                        && light > 0
                    {
                        if to_level(self.keyboard.percent) != light {
                            self.keyboard.percent = to_percent(light);
                        }
                        self.keyboard.shown_light = light;
                    }
                    self.keyboard.device = Some(Arc::new(device));
                    self.apply(Kind::Keyboard, false, None); // restores the saved lighting
                    if self.keyboard.device.is_none() {
                        return; // the send failed and was reported
                    }
                }
                Ok(None) => {
                    self.set_status("No ITE four-zone keyboard controller was found.", true, true);
                    return;
                }
                Err(e) => {
                    self.set_status(&format!("Can't talk to the lighting controller: {e:#}"), true, true);
                    return;
                }
            }
        }
        *self.watch.keyboard.lock().unwrap_or_else(|e| e.into_inner()) = self.keyboard.device.clone();
        match &self.light_bar {
            None => self.open_light_bar(),
            Some(light_bar) if light_bar.device.is_none() => {
                if let Ok(Some(device)) = Device::open(Kind::LightBar) {
                    self.light_bar.as_mut().unwrap().device = Some(Arc::new(device));
                    self.apply(Kind::LightBar, false, None);
                }
            }
            _ => {}
        }
        self.set_connected_status();
        self.start_hotkeys();
        self.refresh_device(Kind::Keyboard);
    }

    fn open_light_bar(&mut self) {
        let Ok(Some(device)) = Device::open(Kind::LightBar) else { return }; // this laptop has none
        if self.settings.light_bar.is_none() {
            // First run: start from what the light bar is showing now.
            let (on, effect_id) = device.read_state().unwrap_or((true, effects::STATIC_ID));
            let effect = effects::LIGHT_BAR.iter().find(|e| e.id == effect_id && !e.drawn_by_app()).map_or("static", |e| e.key);
            self.settings.light_bar = Some(LightingSettings { on, effect: effect.into(), ..Default::default() });
        }
        let mut light_bar = Lighting::new(Kind::LightBar, self.settings.light_bar.as_ref().unwrap());
        light_bar.device = Some(Arc::new(device));
        self.light_bar = Some(light_bar);
        self.apply(Kind::LightBar, false, None);
        self.refresh_device(Kind::LightBar);
    }

    fn set_connected_status(&mut self) {
        self.set_status("", false, false);
        self.refresh_settings_page();
    }

    fn device_failed(&mut self, kind: Kind, error: anyhow::Error) {
        self.engine.pause(kind);
        if let Some(lighting) = self.lighting_mut(kind) {
            lighting.renderer = None;
            lighting.flash = false;
            lighting.fade = None;
            lighting.device = None;
        }
        if kind == Kind::Keyboard {
            *self.watch.keyboard.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
        let name = if kind == Kind::Keyboard { "keyboard" } else { "light bar" };
        self.set_status(&format!("Sending to the {name} failed: {error:#}"), true, true);
    }

    /// What `kind` should show: its own choices, or the keyboard's when the light bar follows it.
    fn plan(&self, kind: Kind) -> Plan {
        let lighting = self.lighting(kind).unwrap_or(&self.keyboard);
        if !(kind == Kind::LightBar && lighting.linked) {
            return Plan {
                effect: lighting.selected(),
                colors: lighting.colors_for(lighting.effect),
                direction: lighting.direction,
                speed: lighting.speed,
            };
        }
        let source = &self.keyboard;
        let source_effect = source.selected();
        let mut colors = source.colors_for(source.effect);
        // Zone colors (the keyboard's Static) have no firmware effect on the light bar; the app draws them.
        let effect = if source_effect.colors == EffectColors::Zones && !source_effect.drawn_by_app() {
            source_effect
        } else {
            lighting.effects.iter().find(|e| e.key == source_effect.key).unwrap_or(&lighting.effects[0])
        };
        if effect.colors == EffectColors::Palette {
            colors.truncate(effect.max_colors);
            if colors.is_empty() {
                colors = effect.default_colors.to_vec();
            }
        }
        Plan { effect, colors, direction: source.direction, speed: source.speed }
    }

    /// Sends the whole lighting state to the device. Live changes skip the controller's own save (a flash write);
    /// `save` commits the final state when the app exits.
    fn apply(&mut self, kind: Kind, save: bool, light_override: Option<u8>) {
        let plan = self.plan(kind);
        let engine = &self.engine;
        let Some(lighting) = (match kind {
            Kind::Keyboard => Some(&mut self.keyboard),
            Kind::LightBar => self.light_bar.as_mut(),
        }) else {
            return;
        };
        let Some(device) = lighting.device.clone() else { return };
        lighting.pending = false;
        lighting.send_timer.stop();
        let mut light = light_override.unwrap_or(to_level(lighting.percent));
        lighting.fade = None; // a fade in progress would undo this
        engine.pause(kind); // the running effect must not change the lights while the new one is set
        lighting.renderer = None;
        lighting.flash = false;
        lighting.overlays.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let draw = needs_app(kind, plan.effect);
        let result = (|| -> Result<()> {
            if !lighting.on {
                device.switch_off()?;
                light = 0;
            } else if kind == Kind::LightBar && draw && save {
                // These need this app; what stays afterwards is the first color, steady.
                let color = plan.colors.first().copied().unwrap_or(effects::LIGHT_BAR[0].default_colors[0]);
                device.set_effect(&effects::LIGHT_BAR[0], &[color], 0, light, plan.speed, true)?;
            } else if kind == Kind::LightBar && draw {
                device.enter_user_mode(light)?;
                if !lighting.idle_off {
                    let renderer = LightBarRenderer::new(device.clone(), scene_for(&plan), lighting.overlays.clone(), lighting.live.clone());
                    lighting.renderer = Some(engine.play(kind, Box::new(renderer)));
                }
            } else if let Some(colors) = wave_colors(kind, &plan) {
                if save {
                    // The firmware's Wave keeps it moving without the app, a zone at a time.
                    device.set_effect(effects::keyboard_wave(), &colors, plan.direction, light, plan.speed, true)?;
                } else {
                    // Drawn on the static effect, starting from the first frame so nothing jumps.
                    let first = scenes::KeyboardWave::frame(&colors, plan.direction, plan.speed, 0.0);
                    device.set_effect(&effects::KEYBOARD[0], &first, 0, light, plan.speed, false)?;
                    if !lighting.idle_off {
                        let wave = scenes::KeyboardWave::new(device.clone(), colors, plan.direction, plan.speed, lighting.live.clone());
                        engine.play(kind, Box::new(wave));
                    }
                }
            } else {
                // The keyboard's effects drawn by this app run on the static zones (what stays once the app exits);
                // those without colors start from the zone colors too.
                let sent = match plan.effect.drawn {
                    // Dark until a key is pressed; once the app exits, the static zone colors.
                    Drawn::Typing if save => lighting.zones.to_vec(),
                    Drawn::Typing => vec![Rgb::BLACK; 4],
                    _ if plan.effect.drawn_by_app() && plan.colors.is_empty() => lighting.zones.to_vec(),
                    _ => plan.colors.clone(),
                };
                device.set_effect(plan.effect, &sent, plan.direction, light, plan.speed, save)?;
                if plan.effect.drawn_by_app() && !lighting.idle_off && !save {
                    let live = lighting.live.clone();
                    let target: Option<Box<dyn Target>> = match plan.effect.drawn {
                        Drawn::Music => Some(Box::new(scenes::KeyboardMusic::new(device.clone(), lighting.zones.to_vec(), light, plan.speed, live))),
                        Drawn::Spectrum if plan.colors.len() == 4 => {
                            Some(Box::new(scenes::KeyboardSpectrum::new(device.clone(), plan.colors.clone(), plan.speed, live)))
                        }
                        Drawn::Typing => Some(Box::new(scenes::KeyboardTyping::new(device.clone(), plan.colors.clone(), plan.speed, live))),
                        Drawn::CpuTemperature => Some(Box::new(scenes::KeyboardTemperature::new(device.clone(), false, live))),
                        Drawn::GpuTemperature => Some(Box::new(scenes::KeyboardTemperature::new(device.clone(), true, live))),
                        _ => None,
                    };
                    if let Some(target) = target {
                        engine.play(kind, target);
                    }
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                lighting.shown_light = light;
                lighting.changed = true;
                if !save && kind == Kind::Keyboard && self.light_bar.as_ref().is_some_and(|l| l.linked) {
                    self.queue_send(Kind::LightBar);
                }
            }
            Err(e) => self.device_failed(kind, e),
        }
        self.update_busy();
    }

    /// Sends after a short pause, so dragging through colors doesn't flood the controller.
    fn queue_send(&mut self, kind: Kind) {
        self.settings_dirty = true;
        let Some(lighting) = self.lighting_mut(kind) else { return };
        lighting.pending = true;
        lighting.send_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(80), move || {
            with_app(move |app| app.apply(kind, false, None));
        });
    }

    /// Moves the brightness to `target` smoothly with the brightness-only command.
    fn fade_to(&mut self, kind: Kind, target: u8, duration: Duration) {
        let music = self.plan(kind).effect.is_music();
        let Some(lighting) = self.lighting_mut(kind) else { return };
        let Some(device) = lighting.device.clone() else {
            lighting.shown_light = target;
            return;
        };
        if !lighting.on {
            return;
        }
        if music && kind == Kind::Keyboard && !lighting.idle_off {
            // Music owns the keyboard's brightness; the slider sets its peak.
            if self.engine.drives_brightness(kind) {
                let lighting = &self.keyboard;
                let target_effect = scenes::KeyboardMusic::new(device, lighting.zones.to_vec(), target, lighting.speed, lighting.live.clone());
                self.engine.play(kind, Box::new(target_effect));
            } else {
                self.apply(kind, false, None);
            }
            return;
        }
        lighting.fade = Some(Fade { from: lighting.shown_light, target, start: Instant::now(), duration });
        self.update_busy();
        if !self.fade_timer.running() {
            self.fade_timer.start(slint::TimerMode::Repeated, Duration::from_millis(15), || with_app(|app| app.fade_tick()));
        }
    }

    fn fade_tick(&mut self) {
        let mut any = false;
        for kind in [Kind::Keyboard, Kind::LightBar] {
            let Some(lighting) = self.lighting_mut(kind) else { continue };
            let Some(fade) = &lighting.fade else { continue };
            let Some(device) = lighting.device.clone() else {
                lighting.fade = None;
                continue;
            };
            let t = fade.start.elapsed().as_secs_f32() / fade.duration.as_secs_f32().max(0.001);
            let (from, target) = (fade.from, fade.target);
            if t >= 1.0 || from == target {
                lighting.fade = None;
                lighting.changed = true;
                match device.try_set_light(target) {
                    Ok(true) => lighting.shown_light = target,
                    Ok(false) => self.apply(kind, false, Some(target)), // switched off by something else, or no 09h
                    Err(e) => self.device_failed(kind, e),
                }
                continue;
            }
            any = true;
            let eased = t * (2.0 - t); // ease out: quick to respond, gentle to settle
            lighting.shown_light = (from as f32 + (target as f32 - from as f32) * eased).round() as u8;
            if let Err(e) = device.set_raw_light(lighting.shown_light) {
                self.device_failed(kind, e);
            }
        }
        if !any {
            self.fade_timer.stop();
        }
        self.update_busy();
    }

    fn set_light(&mut self, kind: Kind, percent: u8) {
        let Some(lighting) = self.lighting_mut(kind) else { return };
        let percent = percent.min(100);
        if percent == lighting.percent && lighting.fade.is_none() {
            return;
        }
        lighting.percent = percent;
        self.settings_dirty = true;
        self.fade_to(kind, to_level(percent), FADE_TIME);
        self.refresh_device(kind);
    }

    /// HIDKeyboard.CloseAnimation: fade the lights out until wake.
    fn fade_out(&mut self, kind: Kind) {
        let Some(lighting) = self.lighting_mut(kind) else { return };
        if lighting.idle_off || !lighting.on {
            return;
        }
        lighting.idle_off = true;
        self.engine.pause(kind);
        self.fade_to(kind, 0, Duration::from_millis(600));
    }

    fn wake(&mut self, kind: Kind) {
        let needs = needs_app(kind, self.plan(kind).effect);
        let Some(lighting) = self.lighting_mut(kind) else { return };
        if !lighting.idle_off {
            return;
        }
        lighting.idle_off = false;
        // Effects this app draws were paused, and so was a flash shown over a firmware effect.
        if needs || lighting.flash {
            self.apply(kind, false, None);
        } else {
            let light = to_level(lighting.percent);
            self.fade_to(kind, light, Duration::from_millis(300));
        }
    }

    /// Shows an indicator animation on the light bar: over the effect if this app is drawing it, otherwise on a dark
    /// bar for a moment before the effect comes back. Nothing while the lights are faded out for idle.
    fn flash(&mut self, overlay: Overlay) {
        let current = self.engine.current(Kind::LightBar);
        let Some(lighting) = self.light_bar.as_mut() else { return };
        let Some(device) = lighting.device.clone() else { return };
        if lighting.idle_off {
            return;
        }
        if lighting.renderer.is_some() && lighting.renderer == current {
            lighting.overlays.lock().unwrap_or_else(|e| e.into_inner()).push(overlay);
            return;
        }
        lighting.pending = false;
        lighting.send_timer.stop();
        lighting.fade = None;
        // The bar may be off or at zero brightness; flash at a moderate brightness then.
        let light = if lighting.on && lighting.percent > 0 { to_level(lighting.percent) } else { MAX_LIGHT * 3 / 5 };
        if let Err(e) = device.enter_user_mode(light) {
            self.device_failed(Kind::LightBar, e);
            return;
        }
        lighting.shown_light = light;
        {
            let mut overlays = lighting.overlays.lock().unwrap_or_else(|e| e.into_inner());
            overlays.clear();
            overlays.push(overlay);
        }
        let id_cell = Arc::new(AtomicU64::new(0));
        let id_for_done = id_cell.clone();
        let renderer = LightBarRenderer::new(device, Box::new(scenes::DarkScene), lighting.overlays.clone(), lighting.live.clone())
            .on_overlays_done(move || {
                let id = id_for_done.load(Ordering::Acquire);
                post(move |app| app.flash_done(id));
            });
        let id = self.engine.play(Kind::LightBar, Box::new(renderer));
        id_cell.store(id, Ordering::Release);
        lighting.renderer = Some(id);
        lighting.flash = true;
    }

    fn flash_done(&mut self, id: u64) {
        if self.light_bar.as_ref().is_some_and(|l| l.flash && l.renderer == Some(id)) {
            self.apply(Kind::LightBar, false, None); // put the effect back
        }
    }

    fn update_busy(&self) {
        // Fades, Music and the idle timer change the keyboard's brightness on purpose.
        let busy = self.keyboard.fade.is_some() || self.idle_off || self.engine.drives_brightness(Kind::Keyboard);
        self.watch.busy.store(busy, Ordering::Release);
    }

    /// Without this app owning the Fn keys, the laptop's firmware handles the keyboard-light keys itself and tells
    /// nobody, so a thread polls the controller and reports when its brightness changes.
    fn start_brightness_watcher(&self) {
        let watch = self.watch.clone();
        let _ = std::thread::Builder::new().name("Brightness watcher".into()).spawn(move || {
            while !watch.stop.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(100));
                if watch.busy.load(Ordering::Acquire) {
                    continue;
                }
                let device = watch.keyboard.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let Some(device) = device else { continue };
                if let Ok(Some(light)) = device.poll_external_light_change() {
                    post(move |app| app.on_external_brightness(light));
                }
            }
        });
    }

    fn on_external_brightness(&mut self, light: u8) {
        self.update_busy();
        if self.watch.busy.load(Ordering::Acquire) {
            return;
        }
        self.last_hotkey_activity = Instant::now();
        if to_level(self.keyboard.percent) != light {
            self.keyboard.percent = to_percent(light);
        }
        self.keyboard.shown_light = light;
        self.keyboard.changed = true;
        self.settings_dirty = true;
        self.refresh_device(Kind::Keyboard);
        if !system::control_center_running() {
            self.osd.show_brightness(self.keyboard.percent as f32 / 100.0);
        }
    }

    // ---- Fn keys --------------------------------------------------------------------------------------------------

    /// Takes the Fn hotkeys over from the firmware so the keyboard-light keys apply instantly and the Win-lock key
    /// reports its state. Without admin rights the firmware keeps handling them.
    fn start_hotkeys(&mut self) {
        if self.demo || self.keyboard.device.is_none() || self.hotkeys.is_some() || self.hotkeys_starting {
            return;
        }
        if system::control_center_running() {
            self.set_fn_mode("Fn keys: handled by Control Center");
            return;
        }
        let Some(ec) = self.power.ec.clone() else {
            self.connect_ec(); // starts the hotkeys once connected
            return;
        };
        self.hotkeys_starting = true;
        // WMI takes a moment to start listening; do it off the UI thread.
        let _ = std::thread::Builder::new().name("Hotkey start".into()).spawn(|| {
            let result = Hotkeys::start(ec, |code| post(move |app| app.on_hotkey(code)));
            let result = result.map_err(|e| format!("{e:#}"));
            post(move |app| app.hotkeys_started(result));
        });
    }

    fn hotkeys_started(&mut self, result: std::result::Result<Hotkeys, String>) {
        self.hotkeys_starting = false;
        match result {
            Ok(hotkeys) if !self.exiting => {
                self.hotkeys = Some(hotkeys);
                self.set_fn_mode("Fn keys: handled instantly by this app");
                self.refresh_win_key();
            }
            Ok(hotkeys) => hotkeys.release(),
            Err(reason) => self.set_fn_mode(&format!("Fn keys: handled by the firmware ({reason})")),
        }
    }

    fn set_fn_mode(&mut self, text: &str) {
        self.fn_mode = text.to_string();
        self.refresh_settings_page();
    }

    fn check_hotkeys(&mut self) {
        let Some(hotkeys) = &self.hotkeys else {
            self.start_hotkeys(); // e.g. Control Center was stopped
            return;
        };
        if system::control_center_running() {
            // Its service owns the bit now; step back without clearing it.
            if let Some(hotkeys) = self.hotkeys.take() {
                hotkeys.abandon();
            }
            self.set_fn_mode("Fn keys: handled by Control Center");
            self.refresh_settings_page();
            return;
        }
        hotkeys.ensure_ap_exist();
        self.refresh_win_key();
    }

    fn refresh_win_key(&mut self) {
        let Some(hotkeys) = &self.hotkeys else {
            self.refresh_settings_page();
            return;
        };
        let answer = hotkeys.query_win_key_lock();
        let _ = std::thread::Builder::new().spawn(move || {
            if let Ok(Ok(locked)) = answer.recv_timeout(Duration::from_secs(3)) {
                post(move |app| app.show_win_key_state(locked));
            }
        });
    }

    fn show_win_key_state(&mut self, locked: bool) {
        self.winkey_locked = locked;
        self.refresh_settings_page();
    }

    fn on_hotkey(&mut self, code: u32) {
        match code {
            hotkeys::KEYBOARD_LIGHT_DOWN | hotkeys::KEYBOARD_LIGHT_UP | hotkeys::KEYBOARD_LIGHT_CYCLE => {
                self.last_hotkey_activity = Instant::now();
                if self.idle_off {
                    self.wake_from_idle();
                }
                // The keys step through FN_KEY_STOPS from wherever the slider is.
                let current = self.keyboard.percent;
                let next_up = FN_KEY_STOPS.iter().copied().find(|&stop| stop > current);
                let percent = match code {
                    hotkeys::KEYBOARD_LIGHT_DOWN => FN_KEY_STOPS.iter().rev().copied().find(|&stop| stop < current).unwrap_or(0),
                    hotkeys::KEYBOARD_LIGHT_UP => next_up.unwrap_or(100),
                    _ => next_up.unwrap_or(0), // past the top wraps to off
                };
                self.set_light(Kind::Keyboard, percent);
                self.osd.show_brightness(percent as f32 / 100.0);
            }
            hotkeys::LIGHT_BAR_TOGGLE => {
                self.last_hotkey_activity = Instant::now();
                let Some(light_bar) = self.light_bar.as_mut() else { return };
                light_bar.on = !light_bar.on;
                let on = light_bar.on;
                self.queue_send(Kind::LightBar);
                self.refresh_device(Kind::LightBar);
                self.osd.show_light_bar(on);
            }
            hotkeys::WIN_KEY_LOCKED | hotkeys::WIN_KEY_UNLOCKED => {
                let locked = code == hotkeys::WIN_KEY_LOCKED;
                self.show_win_key_state(locked);
                if !self.window_is_active() {
                    self.osd.show_win_key(locked); // no pop-up when the click came from this window
                }
            }
            _ => self.on_power_hotkey(code),
        }
    }

    // ---- Idle, timer, indicators, touchpad ------------------------------------------------------------------------

    fn start_timers(&mut self) {
        let mut every = |ms: u64, f: fn(&mut App)| {
            let timer = slint::Timer::default();
            timer.start(slint::TimerMode::Repeated, Duration::from_millis(ms), move || with_app(f));
            self.timers.push(timer);
        };
        every(100, |app| {
            app.check_idle();
            app.check_timer();
            app.update_keyboard_hook();
            app.update_busy();
        });
        every(250, |app| {
            app.check_indicators();
            app.refresh_touchpad();
            app.update_monitor_wanted();
        });
        every(33, |app| app.update_preview());
        every(5000, |app| {
            if !app.demo {
                app.check_hotkeys();
                app.check_power_owner();
            }
            if app.settings_dirty && !app.demo {
                app.store_settings();
                app.settings.save();
                app.settings_dirty = false;
            }
        });
    }

    fn check_idle(&mut self) {
        if self.keyboard.device.is_none() && !self.demo {
            return;
        }
        let idle = milliseconds_since_input().min(self.last_hotkey_activity.elapsed().as_millis() as u64);
        if self.idle_off {
            if idle < 1000 {
                self.wake_from_idle();
            }
            return;
        }
        let timeout = IDLE_TIMEOUTS[self.idle_index].1;
        if timeout > 0 && idle >= timeout as u64 * 1000 {
            self.idle_off = true;
            self.fade_out(Kind::Keyboard);
            self.fade_out(Kind::LightBar);
            self.update_busy();
        }
    }

    fn wake_from_idle(&mut self) {
        self.idle_off = false;
        self.wake(Kind::Keyboard);
        self.wake(Kind::LightBar);
        self.update_busy();
    }

    fn start_timer(&mut self, minutes: u32) {
        self.timer_length = minutes;
        self.timer_end = Some(Instant::now() + Duration::from_secs(minutes as u64 * 60));
        self.osd.show_text(osd::TIMER_GLYPH, &format!("Timer: {minutes} minutes"));
        self.refresh_timer();
    }

    fn cancel_timer(&mut self) {
        self.timer_end = None;
        self.refresh_timer();
    }

    fn time_left(&self) -> String {
        let left = self.timer_end.map_or(Duration::ZERO, |end| end.saturating_duration_since(Instant::now()));
        let seconds = left.as_secs() + u64::from(left.subsec_millis() > 0);
        if seconds >= 3600 {
            format!("{}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60)
        } else {
            format!("{}:{:02}", seconds / 60, seconds % 60)
        }
    }

    fn refresh_timer(&self) {
        let Some(ui) = self.ui() else { return };
        ui.set_timer_running(self.timer_end.is_some());
        if let Some(end) = self.timer_end {
            let total = self.timer_length as f32 * 60.0;
            let left = end.saturating_duration_since(Instant::now()).as_secs_f32();
            ui.set_timer_left(self.time_left().into());
            ui.set_timer_progress((left / total.max(1.0)).clamp(0.0, 1.0));
        }
    }

    fn check_timer(&mut self) {
        let Some(end) = self.timer_end else { return };
        if Instant::now() < end {
            self.refresh_timer();
            return;
        }
        self.timer_end = None;
        self.refresh_timer();
        // Wake the lights to show it, and count it as activity so the idle timer doesn't fade them again at once.
        self.last_hotkey_activity = Instant::now();
        if self.idle_off {
            self.wake_from_idle();
        }
        self.flash(Overlay::new(TIMER_COLOR, OverlayStyle::Blink, 4.0));
        self.osd.show_text(osd::TIMER_GLYPH, "Time's up");
        if let Some(tray) = &self.tray {
            tray.notify("Timer", &format!("{} minutes are up.", self.timer_length));
        }
    }

    fn check_indicators(&mut self) {
        let now = system::lock_keys();
        let Some((caps, num)) = self.lock_keys.replace(now) else { return }; // the first reading is only remembered
        if self.light_bar.is_none() {
            return;
        }
        let style = |on: bool| if on { OverlayStyle::Open } else { OverlayStyle::Close };
        if self.settings.flash_caps_lock && caps != now.0 {
            self.flash(Overlay::new(CAPS_COLOR, style(now.0), 0.9));
        }
        if self.settings.flash_num_lock && num != now.1 {
            self.flash(Overlay::new(NUM_COLOR, style(now.1), 0.9));
        }
    }

    /// The keyboard hook is only installed while an effect reacts to typing.
    fn update_keyboard_hook(&mut self) {
        let needs = self.engine.needs_keys();
        if needs && self.key_hook.is_none() {
            self.key_hook = KeyboardHook::install(self.engine.key_sink());
        } else if !needs && self.key_hook.is_some() {
            self.key_hook = None;
        }
    }

    fn refresh_touchpad(&mut self) {
        let state = touchpad::is_enabled();
        if let (Some(enabled), Some(previous)) = (state, self.last_touchpad)
            && enabled != previous
            && !self.window_is_active()
            && !system::control_center_running()
        {
            self.osd.show_touchpad(enabled);
        }
        if self.last_touchpad != state {
            self.last_touchpad = state;
            self.refresh_settings_page();
        }
    }

    // ---- The window ---------------------------------------------------------------------------------------------

    fn window_hwnd(&self) -> Option<HWND> {
        self.ui().and_then(|ui| hwnd(&ui))
    }

    fn window_is_active(&self) -> bool {
        self.window_hwnd().is_some_and(|h| unsafe { GetForegroundWindow() } == h)
    }

    fn set_status(&self, text: &str, error: bool, retry: bool) {
        let Some(ui) = self.ui() else { return };
        ui.set_status_text(text.into());
        ui.set_status_error(error);
        ui.set_show_retry(retry);
    }

    fn refresh_device(&mut self, kind: Kind) {
        let Some(ui) = self.ui() else { return };
        let Some(lighting) = self.lighting(kind) else { return };
        let plan_effect = self.plan(kind).effect;
        let effect = lighting.selected();
        let view = DeviceView {
            on: lighting.on,
            linked: lighting.linked,
            effect: lighting.effect as i32,
            effect_name: if lighting.linked { plan_effect.name.into() } else { effect.name.into() },
            hint: effect.hint.into(),
            colors_kind: match effect.colors {
                EffectColors::None => 0,
                EffectColors::Zones => 1,
                EffectColors::Palette => 2,
            },
            min_colors: effect.min_colors as i32,
            max_colors: effect.max_colors as i32,
            has_speed: effect.has_speed,
            has_direction: effect.has_direction,
            direction: if lighting.direction == 2 { 1 } else { 0 },
            music: effect.is_music(),
        };
        let palette = lighting.palettes[lighting.effect].clone();
        let (light, speed) = (lighting.percent as f32, lighting.speed as f32);
        match kind {
            Kind::Keyboard => {
                ui.set_kb_view(view);
                set_colors(&self.models.kb_zones, &lighting.zones);
                set_colors(&self.models.kb_palette, &palette);
                if ui.get_kb_light() != light {
                    ui.set_kb_light(light);
                }
                ui.set_kb_speed(speed);
            }
            Kind::LightBar => {
                ui.set_has_light_bar(true);
                ui.set_lb_view(view);
                set_colors(&self.models.lb_palette, &palette);
                if ui.get_lb_light() != light {
                    ui.set_lb_light(light);
                }
                ui.set_lb_speed(speed);
            }
        }
        if kind == Kind::Keyboard {
            self.update_tray_icon();
            if self.light_bar.as_ref().is_some_and(|l| l.linked) {
                self.refresh_device(Kind::LightBar);
            }
        }
    }

    fn refresh_settings_page(&self) {
        let Some(ui) = self.ui() else { return };
        ui.set_touchpad_state(match self.last_touchpad.or_else(touchpad::is_enabled) {
            None => 0,
            Some(true) => 1,
            Some(false) => 2,
        });
        ui.set_winkey_available(self.hotkeys.is_some());
        ui.set_winkey_locked(self.winkey_locked);
        let mut about = Vec::new();
        if let Some(device) = &self.keyboard.device {
            about.push(format!("Keyboard firmware {:02X}.{:02X}", device.firmware.0, device.firmware.1));
        }
        if let Some(device) = self.light_bar.as_ref().and_then(|l| l.device.as_ref()) {
            about.push(format!("light bar firmware {:02X}.{:02X}", device.firmware.0, device.firmware.1));
        }
        if about.is_empty() {
            about.push(if self.demo { "Demo mode".into() } else { "Not connected".into() });
        }
        ui.set_about(about.join(" · ").into());
        ui.set_fn_keys(self.fn_mode.clone().into());
    }

    /// Updates the previews: the keyboard drawing, the light bar and the effect tiles.
    fn update_preview(&mut self) {
        let Some(ui) = self.ui() else { return };
        if !ui.window().is_visible() {
            return;
        }
        let t = self.started.elapsed().as_secs_f64();

        // The keyboard.
        let plan = self.plan(Kind::Keyboard);
        let keyboard = &self.keyboard;
        let running = self.engine.current(Kind::Keyboard).is_some() && !self.demo;
        let mut zones = if running && needs_app(Kind::Keyboard, plan.effect) {
            let live = keyboard.live.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if live.len() == 4 { live } else { preview::simulate(plan.effect, &plan.colors, plan.direction, plan.speed, t, 4, true) }
        } else {
            preview::simulate(plan.effect, &plan.colors, plan.direction, plan.speed, t, 4, true)
        };
        let shown = if self.demo { if keyboard.idle_off { 0 } else { to_level(keyboard.percent) } } else { keyboard.shown_light };
        let scale = if keyboard.on { preview::brightness_scale(shown) } else { 0.0 };
        for z in &mut zones {
            *z = z.scale(scale);
        }
        set_colors(&self.models.kb_glow, &zones);

        // The light bar.
        if let Some(light_bar) = &self.light_bar {
            let plan = self.plan(Kind::LightBar);
            let drawn = light_bar.renderer.is_some() && self.engine.current(Kind::LightBar) == light_bar.renderer && !self.demo;
            let mut leds = if drawn {
                light_bar.live.lock().unwrap_or_else(|e| e.into_inner()).clone()
            } else {
                preview::simulate(plan.effect, &plan.colors, plan.direction, plan.speed, t, crate::lighting::device::LIGHT_BAR_LEDS, false)
            };
            leds.resize(crate::lighting::device::LIGHT_BAR_LEDS, Rgb::BLACK);
            let shown = if self.demo { if light_bar.idle_off { 0 } else { to_level(light_bar.percent) } } else { light_bar.shown_light };
            let scale = if light_bar.on { preview::brightness_scale(shown) } else { 0.0 };
            for led in &mut leds {
                *led = led.scale(scale);
            }
            set_colors(&self.models.lb_leds, &leds);
        }

        // The tiles of the page showing.
        let page = ui.get_page();
        let (lighting, model, zones_keyboard) = match page {
            0 => (Some(&self.keyboard), &self.models.kb_tile_leds, true),
            1 => (self.light_bar.as_ref(), &self.models.lb_tile_leds, false),
            _ => (None, &self.models.kb_tile_leds, true),
        };
        if let Some(lighting) = lighting {
            let mut all = Vec::with_capacity(lighting.effects.len() * TILE_LEDS);
            for (i, effect) in lighting.effects.iter().enumerate() {
                let colors = lighting.colors_for(i);
                all.extend(preview::simulate(effect, &colors, lighting.direction, 2, t + i as f64 * 0.37, TILE_LEDS, zones_keyboard));
            }
            set_colors(model, &all);
        }
    }

    fn update_tray_icon(&mut self) {
        let colors = self.keyboard.icon_colors();
        if let Some(tray) = &mut self.tray {
            let pixels = tray::zone_pixels(&colors, 32);
            tray.set_icon(tray::make_icon(&pixels, 32));
        }
    }

    fn create_tray(&mut self) {
        let tray = Tray::create(
            "Tulpar Control",
            || {
                let mut items = Vec::new();
                APP.with(|cell| {
                    if let Ok(app) = cell.try_borrow()
                        && let Some(app) = app.as_ref()
                    {
                        items = app.tray_menu();
                    }
                });
                items
            },
            |event| match event {
                TrayEvent::Open => with_app(|app| app.show_window()),
                TrayEvent::Command(id) => with_app(move |app| app.tray_command(id)),
                TrayEvent::Resumed => with_app(|app| {
                    if !app.demo {
                        app.check_hotkeys();
                        app.apply_power(); // the EC forgets some of it in sleep
                    }
                }),
                TrayEvent::PowerChanged => with_app(|app| app.power_source_changed()),
                TrayEvent::ThemeChanged => with_app(|app| {
                    if let Some(ui) = app.ui() {
                        apply_theme(&ui);
                        if let Some(h) = hwnd(&ui) {
                            style_title_bar(h);
                        }
                    }
                    if let Some(tray) = &app.tray {
                        tray.restore();
                    }
                }),
            },
        );
        match tray {
            Ok(tray) => {
                self.tray = Some(tray);
                self.update_tray_icon();
            }
            Err(e) => self.set_status(&format!("Can't create the tray icon: {e}"), true, false),
        }
    }

    /// Quick settings in the tray icon's menu, built fresh each time it opens.
    fn tray_menu(&self) -> Vec<MenuItem> {
        let mut menu = Vec::new();
        let effect_items = |lighting: &Lighting, base: u32, enabled: bool| -> Vec<MenuItem> {
            lighting
                .effects
                .iter()
                .enumerate()
                .map(|(i, e)| MenuItem::check(base + i as u32, e.name, i == lighting.effect, enabled))
                .collect()
        };
        let mut keyboard = effect_items(&self.keyboard, CMD_KEYBOARD_EFFECT, true);
        keyboard.push(MenuItem::Separator);
        let brightness = FN_KEY_STOPS
            .iter()
            .enumerate()
            .map(|(i, &percent)| {
                let label = if percent == 0 { "Off".to_string() } else { format!("{percent}%") };
                MenuItem::check(CMD_KEYBOARD_LIGHT + i as u32, label, self.keyboard.percent == percent, true)
            })
            .collect();
        keyboard.push(MenuItem::Submenu { text: "Brightness".into(), enabled: true, items: brightness });
        menu.push(MenuItem::Submenu { text: "Keyboard".into(), enabled: true, items: keyboard });

        if let Some(light_bar) = &self.light_bar {
            let mut items = vec![
                MenuItem::check(CMD_LIGHT_BAR_POWER, "On", light_bar.on, true),
                MenuItem::check(CMD_LIGHT_BAR_LINK, "Same as keyboard", light_bar.linked, light_bar.on),
                MenuItem::Separator,
            ];
            items.extend(effect_items(light_bar, CMD_LIGHT_BAR_EFFECT, light_bar.on && !light_bar.linked));
            menu.push(MenuItem::Submenu { text: "Light bar".into(), enabled: true, items });
        }

        let mut timer = Vec::new();
        if self.timer_end.is_some() {
            timer.push(MenuItem::item(CMD_TIMER_CANCEL, format!("Cancel ({} left)", self.time_left())));
            timer.push(MenuItem::Separator);
        }
        for (i, minutes) in TIMER_MINUTES.iter().enumerate() {
            timer.push(MenuItem::check(CMD_TIMER + i as u32, format!("{minutes} minutes"), self.timer_end.is_some() && self.timer_length == *minutes, true));
        }
        menu.push(MenuItem::Submenu { text: "Timer".into(), enabled: true, items: timer });
        if let Some((modes, fan_boost)) = self.power_menu() {
            let mut items: Vec<MenuItem> = modes.into_iter().map(|(i, name, on)| MenuItem::check(CMD_MODE + i, name, on, true)).collect();
            items.push(MenuItem::Separator);
            items.push(MenuItem::check(CMD_FAN_BOOST, "Fan Boost", fan_boost, true));
            menu.push(MenuItem::Submenu { text: "Performance".into(), enabled: true, items });
        }
        menu.push(MenuItem::Separator);
        menu.push(MenuItem::item(CMD_OPEN, "Open Tulpar Control"));
        menu.push(MenuItem::item(CMD_EXIT, "Exit"));
        menu
    }

    fn tray_command(&mut self, id: u32) {
        match id {
            CMD_OPEN => self.show_window(),
            CMD_EXIT => self.exit(),
            CMD_LIGHT_BAR_POWER => self.set_light_bar_on(!self.light_bar.as_ref().is_some_and(|l| l.on)),
            CMD_LIGHT_BAR_LINK => self.set_linked(!self.light_bar.as_ref().is_some_and(|l| l.linked)),
            CMD_TIMER_CANCEL => self.cancel_timer(),
            CMD_FAN_BOOST => self.set_fan_boost(!self.power.fan_boost),
            _ if (CMD_MODE..CMD_MODE + 3).contains(&id) => self.set_mode(Mode::from_index((id - CMD_MODE) as usize)),
            _ if (CMD_KEYBOARD_EFFECT..CMD_KEYBOARD_EFFECT + 100).contains(&id) => self.select_effect(Kind::Keyboard, (id - CMD_KEYBOARD_EFFECT) as usize),
            _ if (CMD_KEYBOARD_LIGHT..CMD_KEYBOARD_LIGHT + FN_KEY_STOPS.len() as u32).contains(&id) => {
                self.set_light(Kind::Keyboard, FN_KEY_STOPS[(id - CMD_KEYBOARD_LIGHT) as usize]);
            }
            _ if (CMD_LIGHT_BAR_EFFECT..CMD_LIGHT_BAR_EFFECT + 100).contains(&id) => self.select_effect(Kind::LightBar, (id - CMD_LIGHT_BAR_EFFECT) as usize),
            _ if (CMD_TIMER..CMD_TIMER + TIMER_MINUTES.len() as u32).contains(&id) => self.start_timer(TIMER_MINUTES[(id - CMD_TIMER) as usize]),
            _ => {}
        }
    }

    fn show_window(&mut self) {
        if let Some(ui) = self.ui() {
            show_window(&ui);
        }
    }

    // ---- What the window changes ----------------------------------------------------------------------------------

    fn select_effect(&mut self, kind: Kind, index: usize) {
        let Some(lighting) = self.lighting_mut(kind) else { return };
        if index >= lighting.effects.len() {
            return;
        }
        lighting.effect = index;
        self.refresh_device(kind);
        self.queue_send(kind);
    }

    fn set_speed(&mut self, kind: Kind, speed: u8) {
        let Some(lighting) = self.lighting_mut(kind) else { return };
        lighting.speed = speed.min(4);
        self.refresh_device(kind);
        self.queue_send(kind);
    }

    fn set_direction(&mut self, kind: Kind, index: i32) {
        let Some(lighting) = self.lighting_mut(kind) else { return };
        lighting.direction = if index == 1 { 2 } else { 1 };
        self.refresh_device(kind);
        self.queue_send(kind);
    }

    fn set_light_bar_on(&mut self, on: bool) {
        let Some(light_bar) = self.light_bar.as_mut() else { return };
        light_bar.on = on;
        self.refresh_device(Kind::LightBar);
        self.queue_send(Kind::LightBar);
    }

    fn set_linked(&mut self, linked: bool) {
        let Some(light_bar) = self.light_bar.as_mut() else { return };
        light_bar.linked = linked;
        self.refresh_device(Kind::LightBar);
        self.queue_send(Kind::LightBar);
    }

    fn edit_color(&mut self, kind: Kind, target: i32, index: usize, color: Rgb) {
        let Some(lighting) = self.lighting_mut(kind) else { return };
        match target {
            0 if index < 4 => lighting.zones[index] = color,
            2 => lighting.zones = [color; 4],
            1 => {
                let effect = lighting.effect;
                if let Some(slot) = lighting.palettes[effect].get_mut(index) {
                    *slot = color;
                }
            }
            _ => return,
        }
        self.refresh_device(kind);
        self.queue_send(kind);
    }

    fn add_color(&mut self, kind: Kind) -> i32 {
        let Some(lighting) = self.lighting_mut(kind) else { return 0 };
        let effect = lighting.effect;
        let max = lighting.effects[effect].max_colors;
        let palette = &mut lighting.palettes[effect];
        if palette.len() < max {
            let last = palette.last().copied().unwrap_or(Rgb::WHITE);
            palette.push(last);
        }
        let index = palette.len() as i32 - 1;
        self.refresh_device(kind);
        self.queue_send(kind);
        index
    }

    fn remove_color(&mut self, kind: Kind, index: usize) {
        let Some(lighting) = self.lighting_mut(kind) else { return };
        let effect = lighting.effect;
        let min = lighting.effects[effect].min_colors;
        let palette = &mut lighting.palettes[effect];
        if palette.len() > min && index < palette.len() {
            palette.remove(index);
        }
        self.refresh_device(kind);
        self.queue_send(kind);
    }

    fn reset_palette(&mut self, kind: Kind) {
        let Some(lighting) = self.lighting_mut(kind) else { return };
        let effect = lighting.effect;
        lighting.palettes[effect] = lighting.effects[effect].default_colors.to_vec();
        self.refresh_device(kind);
        self.queue_send(kind);
    }

    fn set_idle(&mut self, index: usize) {
        self.idle_index = index.min(IDLE_TIMEOUTS.len() - 1);
        self.settings_dirty = true;
    }

    fn set_start_with_windows(&mut self, enabled: bool) {
        if self.demo {
            return;
        }
        let result = if enabled { startup::enable() } else { startup::disable() };
        if let Err(e) = result {
            self.set_status(&format!("Couldn't change the startup setting: {e:#}"), true, false);
        }
        if let Some(ui) = self.ui() {
            ui.set_start_with_windows(startup::is_enabled());
        }
    }

    fn store_settings(&mut self) {
        let mut keyboard = self.settings.keyboard.clone().unwrap_or_default();
        self.keyboard.store(&mut keyboard);
        self.settings.keyboard = Some(keyboard);
        if let Some(light_bar) = &self.light_bar {
            let mut stored = self.settings.light_bar.clone().unwrap_or_default();
            light_bar.store(&mut stored);
            self.settings.light_bar = Some(stored);
        }
        self.settings.idle_timeout_seconds = IDLE_TIMEOUTS[self.idle_index].1;
        self.power.store(&mut self.settings.power);
    }

    // ---- Exit -----------------------------------------------------------------------------------------------------

    /// Saves the lighting into the controllers, gives the Fn keys back and quits.
    pub fn exit(&mut self) {
        if self.exiting {
            return;
        }
        self.exiting = true;
        if !self.demo {
            crate::shell::instance::note_exiting(); // a launch from now on waits for this one to finish
        }
        self.timers.clear();
        self.fade_timer.stop();
        self.watch.stop.store(true, Ordering::Release);
        if self.idle_off {
            self.wake_from_idle();
        }
        self.engine.shutdown();
        self.monitor.stop();
        self.key_hook = None;
        if !self.demo {
            // Live changes skip the controllers' non-volatile save to avoid a flash write per slider step; commit
            // the final state once so it survives without this app running. The effects this app draws can't run
            // without it: on the keyboard their static zone colors stay, on the light bar their first color.
            if self.keyboard.pending || self.keyboard.changed {
                self.apply(Kind::Keyboard, true, None);
            }
            if self.light_bar.as_ref().is_some_and(|l| l.pending || l.changed) {
                self.apply(Kind::LightBar, true, None);
            }
            self.release_fans();
            if let Some(hotkeys) = self.hotkeys.take() {
                if let Some(ui) = self.ui() {
                    let _ = ui.hide();
                }
                self.tray = None;
                hotkeys.release(); // hands the Fn keys back to the firmware
                self.restore_brightness_after_hand_over();
            }
            self.store_settings();
            self.settings.save();
        }
        self.tray = None;
        self.keyboard.device = None;
        if let Some(light_bar) = &mut self.light_bar {
            light_bar.device = None;
        }
        let _ = slint::quit_event_loop();
    }

    /// About a second after the Fn keys go back to the firmware, the EC applies its own internal keyboard level, which
    /// isn't the level shown and isn't in any EC register we can see (it switched the lights off in testing; Control
    /// Center has the same behaviour). Wait for that and put the user's brightness back.
    fn restore_brightness_after_hand_over(&mut self) {
        let Some(keyboard) = self.keyboard.device.clone() else { return };
        let light = to_level(self.keyboard.percent);
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match keyboard.read_light() {
                Ok(current) if current != light => {
                    if !keyboard.try_set_light(light).unwrap_or(false) {
                        self.apply(Kind::Keyboard, false, None);
                        if self.keyboard.device.is_none() {
                            return;
                        }
                    }
                }
                Err(_) => return, // exiting anyway
                _ => {}
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn milliseconds_since_input() -> u64 {
    let mut info = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return 0;
    }
    let now = unsafe { windows::Win32::System::SystemInformation::GetTickCount() };
    now.wrapping_sub(info.dwTime) as u64 // both wrap at 2^32 ms
}

fn kind_of(device: i32) -> Kind {
    if device == 1 { Kind::LightBar } else { Kind::Keyboard }
}

fn connect_callbacks(ui: &AppWindow) {
    ui.on_select_effect(|device, index| with_app(move |app| app.select_effect(kind_of(device), index as usize)));
    ui.on_light_changed(|device, value| with_app(move |app| app.set_light(kind_of(device), value.round() as u8)));
    ui.on_speed_changed(|device, value| with_app(move |app| app.set_speed(kind_of(device), value.round() as u8)));
    ui.on_direction_changed(|device, index| with_app(move |app| app.set_direction(kind_of(device), index)));
    ui.on_set_on(|on| with_app(move |app| app.set_light_bar_on(on)));
    ui.on_set_linked(|on| with_app(move |app| app.set_linked(on)));
    ui.on_color_edited(|device, target, index, color| {
        with_app(move |app| app.edit_color(kind_of(device), target, index.max(0) as usize, Rgb::from_slint(color)))
    });
    ui.on_add_color(|device| {
        // Needs an answer now, so it can't wait for a busy app; the window only calls this from a click.
        let mut index = 0;
        APP.with(|cell| {
            if let Ok(mut app) = cell.try_borrow_mut()
                && let Some(app) = app.as_mut()
            {
                index = app.add_color(kind_of(device));
            }
        });
        index
    });
    ui.on_remove_color(|device, index| with_app(move |app| app.remove_color(kind_of(device), index.max(0) as usize)));
    ui.on_reset_palette(|device| with_app(move |app| app.reset_palette(kind_of(device))));
    ui.on_start_timer(|minutes| with_app(move |app| app.start_timer(minutes.max(1) as u32)));
    ui.on_cancel_timer(|| with_app(|app| app.cancel_timer()));
    ui.on_set_flash_caps(|on| {
        with_app(move |app| {
            app.settings.flash_caps_lock = on;
            app.settings_dirty = true;
            if let Some(ui) = app.ui() {
                ui.set_flash_caps(on);
            }
        })
    });
    ui.on_set_flash_num(|on| {
        with_app(move |app| {
            app.settings.flash_num_lock = on;
            app.settings_dirty = true;
            if let Some(ui) = app.ui() {
                ui.set_flash_num(on);
            }
        })
    });
    ui.on_set_idle(|index| with_app(move |app| app.set_idle(index.max(0) as usize)));
    ui.on_toggle_touchpad(|| {
        with_app(|app| {
            if let Err(e) = touchpad::toggle() {
                app.set_status(&format!("{e:#}"), true, false);
            }
        })
    });
    ui.on_toggle_winkey(|| {
        with_app(|app| {
            if let Some(hotkeys) = &app.hotkeys {
                hotkeys.toggle_win_key_lock(); // the EC answers with a 64/65 event
            }
        })
    });
    ui.on_set_start_with_windows(|on| with_app(move |app| app.set_start_with_windows(on)));
    ui.on_set_mode(|index| with_app(move |app| app.set_mode(Mode::from_index(index.max(0) as usize))));
    ui.on_set_fan_boost(|on| with_app(move |app| app.set_fan_boost(on)));
    ui.on_set_curve_enabled(|on| with_app(move |app| app.set_curve_enabled(on)));
    ui.on_select_curve_fan(|fan| with_app(move |app| app.select_curve_fan(fan.max(0) as usize)));
    ui.on_curve_duty(|step, duty| with_app(move |app| app.set_curve_duty(step.max(0) as usize, duty)));
    ui.on_reset_curve(|| with_app(|app| app.reset_curve()));
    ui.on_set_charge_mode(|index| with_app(move |app| app.set_charge_mode(ChargeMode::from_index(index.max(0) as usize))));
    ui.on_retry(|| with_app(|app| app.open_devices()));
    ui.on_format_hex(|color| Rgb::from_slint(color).hex().into());
    ui.on_parse_hex(|text| match Rgb::parse_hex(&text) {
        Some(c) => c.to_slint(),
        None => slint::Color::from_argb_u8(0, 0, 0, 0),
    });
    ui.window().on_close_requested(|| slint::CloseRequestResponse::HideWindow);
}

/// The window's native handle.
pub fn hwnd(ui: &AppWindow) -> Option<HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = ui.window().window_handle();
    match handle.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as _)),
        _ => None,
    }
}

/// Follows the Windows theme and accent color.
fn apply_theme(ui: &AppWindow) {
    let dark = system::dark_mode();
    let theme = ui.global::<Theme>();
    theme.set_dark(dark);
    theme.set_accent(osd::accent_fill(dark).to_slint());
    ui.set_dark_mode(dark);
}

/// Gives the title bar the window's own background, as Windows 11 apps do.
fn style_title_bar(hwnd: HWND) {
    use windows::Win32::Graphics::Dwm::{DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DwmSetWindowAttribute};
    let dark = system::dark_mode();
    let caption: u32 = if dark { 0x0020_2020 } else { 0x00F3_F3F3 }; // COLORREF: 0x00BBGGRR
    let dark_flag: i32 = dark as i32;
    let round: i32 = 2; // DWMWCP_ROUND
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark_flag as *const _ as _, 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR, &caption as *const _ as _, 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as _, 4);
    }
}

/// A first size that fits the screen with the mouse pointer: at most 1100 × 800, and no more than about 90% of
/// the screen.
fn fit_to_screen(ui: &AppWindow) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    unsafe {
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return;
        }
        let (mut dpi, mut dpi_y) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y);
        let scale = dpi as f32 / 96.0;
        let work_width = (info.rcWork.right - info.rcWork.left) as f32 / scale;
        let work_height = (info.rcWork.bottom - info.rcWork.top) as f32 / scale;
        let width = 1100f32.min(work_width * 0.9).max(880f32.min(work_width));
        let height = 800f32.min(work_height * 0.9 - 32.0).max(560f32.min(work_height - 40.0));
        ui.window().set_size(slint::LogicalSize::new(width, height));
    }
}

fn show_window(ui: &AppWindow) {
    let first_show = !ui.window().is_visible();
    if first_show && hwnd(ui).is_none() {
        fit_to_screen(ui);
    }
    let _ = ui.show();
    if let Some(h) = hwnd(ui) {
        if first_show {
            style_title_bar(h);
        }
        unsafe {
            let _ = ShowWindow(h, SW_RESTORE);
            let _ = SetForegroundWindow(h);
        }
    } else {
        // The native window appears after the event loop runs once.
        let weak = ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(50), move || {
            if let Some(ui) = weak.upgrade()
                && let Some(h) = hwnd(&ui)
            {
                style_title_bar(h);
            }
        });
    }
}
