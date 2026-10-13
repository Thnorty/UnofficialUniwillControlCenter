use crate::color::Rgb;

/// Which colors an effect lets you choose.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EffectColors {
    None,
    /// The keyboard's four zones.
    Zones,
    /// A list of colors the effect cycles through.
    Palette,
}

/// Who animates an effect: the controller's firmware, or this app (and how).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Drawn {
    Firmware,
    Music,
    Spectrum,
    StarSpark,
    StarHitting,
    Typing,
    Battery,
    CpuLoad,
    CpuTemperature,
    GpuTemperature,
}

/// How the effects are grouped in the window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Lighting,
    Animated,
    Audio,
    Status,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Lighting => "Lighting",
            Group::Animated => "Animated by this app",
            Group::Audio => "Sound",
            Group::Status => "System status",
        }
    }
}

/// A lighting effect: one from Control Center's lighting pages with the firmware effect ID it sends
/// (LM_ITE_RGB.Translate_ITE_EffectIndex, and HIDLightbar.RunEffct for the light bar), or one drawn by this app.
#[derive(Debug)]
pub struct Effect {
    pub key: &'static str,
    pub name: &'static str,
    pub id: u8,
    pub colors: EffectColors,
    pub has_speed: bool,
    pub has_direction: bool,
    pub min_colors: usize,
    pub max_colors: usize,
    pub hint: &'static str,
    pub drawn: Drawn,
    pub group: Group,
    pub default_colors: &'static [Rgb],
}

pub const STATIC_ID: u8 = 1;
pub const WAVE_ID: u8 = 3;
pub const RAINBOW_ID: u8 = 5;
pub const MUSIC_ID: u8 = 34;
const NO_FIRMWARE_ID: u8 = 0; // drawn by this app on top of the static effect

/// Control Center's default 7-color palette for animated effects (from its RGBKeyboard.reg defaults).
pub const DEFAULT_PALETTE: &[Rgb] = &[
    Rgb::new(255, 0, 0),
    Rgb::new(255, 165, 0),
    Rgb::new(255, 255, 0),
    Rgb::new(0, 255, 0),
    Rgb::new(0, 0, 255),
    Rgb::new(0, 255, 255),
    Rgb::new(139, 0, 255),
];

/// The palette Control Center saved for the light bar's Thinking (HKLM\SOFTWARE\OEM\GamingCenter2\RGBKeyboard\MEZone_Lighbar).
const THINKING_PALETTE: &[Rgb] = &[
    Rgb::new(255, 59, 0),
    Rgb::new(255, 100, 0),
    Rgb::new(255, 158, 0),
    Rgb::new(255, 190, 0),
    Rgb::new(255, 155, 0),
    Rgb::new(255, 118, 0),
    Rgb::new(255, 77, 0),
    Rgb::new(0, 0, 255),
    Rgb::new(136, 0, 255),
];

const CYAN: &[Rgb] = &[Rgb::new(0, 255, 255)];

impl Effect {
    const fn firmware(key: &'static str, name: &'static str, id: u8, colors: EffectColors, has_speed: bool, hint: &'static str) -> Effect {
        Effect {
            key,
            name,
            id,
            colors,
            has_speed,
            has_direction: false,
            min_colors: 1,
            max_colors: 7,
            hint,
            drawn: Drawn::Firmware,
            group: Group::Lighting,
            default_colors: DEFAULT_PALETTE,
        }
    }

    const fn app(key: &'static str, name: &'static str, drawn: Drawn, group: Group, colors: EffectColors, has_speed: bool, hint: &'static str) -> Effect {
        Effect {
            key,
            name,
            id: if matches!(drawn, Drawn::Music) { MUSIC_ID } else { NO_FIRMWARE_ID },
            colors,
            has_speed,
            has_direction: false,
            min_colors: 1,
            max_colors: 7,
            hint,
            drawn,
            group,
            default_colors: DEFAULT_PALETTE,
        }
    }

    /// Music is driven by this app: the keyboard shows the static zones and the brightness follows the sound.
    pub fn is_music(&self) -> bool {
        self.drawn == Drawn::Music
    }

    /// Animated by this app rather than the controller, so it only runs while the app does.
    pub fn drawn_by_app(&self) -> bool {
        self.drawn != Drawn::Firmware
    }

    pub fn is_rainbow(&self) -> bool {
        self.id == RAINBOW_ID && self.drawn == Drawn::Firmware
    }
}

/// The keyboard's Wave, which the app draws smoothly.
pub fn keyboard_wave() -> &'static Effect {
    &KEYBOARD[2]
}

pub static KEYBOARD: &[Effect] = &[
    Effect::firmware("static", "Static", STATIC_ID, EffectColors::Zones, false, "Each of the four zones glows in its own color."),
    Effect::firmware("breathing", "Breathing", 2, EffectColors::Palette, true, "Fades in and out, one color at a time."),
    Effect { has_direction: true, ..Effect::firmware("wave", "Wave", WAVE_ID, EffectColors::Palette, true, "The colors flow smoothly across the zones.") },
    Effect::firmware("rainbow", "Rainbow", RAINBOW_ID, EffectColors::None, false,
        "Red, green, blue and magenta, one per zone. The keyboard's Rainbow doesn't move."),
    Effect::firmware("flash", "Flash", 18, EffectColors::Palette, true, "Flashes each color five times, then moves on to the next."),
    Effect { min_colors: 2, ..Effect::firmware("mix", "Mix", 19, EffectColors::Palette, true,
        "The whole keyboard goes through the colors in turn, blending from one into the next.") },
    Effect::app("typing", "Typing", Drawn::Typing, Group::Animated, EffectColors::Palette, true,
        "Each key press lights its zone, taking the colors in turn, and the light spreads to the zones beside it as it fades. Dark while you're not typing."),
    Effect::app("music", "Music", Drawn::Music, Group::Audio, EffectColors::Zones, true,
        "Pulses with the sound this PC plays. Brightness sets the peak; speed sets how fast it falls back."),
    Effect::app("spectrum", "Spectrum", Drawn::Spectrum, Group::Audio, EffectColors::Zones, true,
        "Each zone follows a range of the sound this PC plays: bass on the left, treble on the right. Speed sets how fast the zones fall back."),
    Effect::app("cputemp", "CPU temperature", Drawn::CpuTemperature, Group::Status, EffectColors::None, false,
        "Blue when cool, then green, yellow and red as the CPU heats up (35–95 °C)."),
    Effect::app("gputemp", "GPU temperature", Drawn::GpuTemperature, Group::Status, EffectColors::None, false,
        "Blue when cool, then green, yellow and red as the NVIDIA GPU heats up (35–90 °C). Dim gray while the GPU is switched off."),
];

// The firmware effects HIDLightbar.RunEffct handles for the light bar (Wave is its "Colorwave", 32, which has no
// direction), and the effects this app draws LED by LED.
pub static LIGHT_BAR: &[Effect] = &[
    Effect { max_colors: 1, default_colors: CYAN, ..Effect::firmware("static", "Static", STATIC_ID, EffectColors::Palette, false, "The whole bar in one color.") },
    Effect::firmware("breathing", "Breathing", 2, EffectColors::Palette, true, "Fades in and out, one color at a time."),
    Effect::firmware("wave", "Wave", 32, EffectColors::Palette, true, "The colors flow along the bar."),
    Effect::firmware("rainbow", "Rainbow", RAINBOW_ID, EffectColors::None, true, "Every color of the rainbow, flowing along the bar."),
    Effect::firmware("flash", "Flash", 18, EffectColors::Palette, true, "Flashes on and off, one color at a time."),
    Effect { min_colors: 2, ..Effect::firmware("mix", "Mix", 19, EffectColors::Palette, true, "Mixes the colors along the bar.") },
    Effect { max_colors: 9, default_colors: THINKING_PALETTE, ..Effect::firmware("thinking", "Thinking", 33, EffectColors::Palette, true, "Control Center's Thinking: waves of warm light.") },
    Effect::app("starspark", "Star Spark", Drawn::StarSpark, Group::Animated, EffectColors::Palette, true,
        "Every LED twinkles in its color (Control Center's Star Spark)."),
    Effect::app("starhitting", "Star Hitting", Drawn::StarHitting, Group::Animated, EffectColors::Palette, true,
        "A ball of light per color bounces along the bar and off the others (Control Center's Star Hitting)."),
    Effect::app("typing", "Typing", Drawn::Typing, Group::Animated, EffectColors::Palette, true,
        "Each key press sends a ripple out from the matching spot on the bar, taking the colors in turn."),
    Effect::app("music", "Music", Drawn::Music, Group::Audio, EffectColors::Palette, true,
        "A level meter that grows from the middle with the sound this PC plays: the first color in the middle, the last at the ends. Speed sets how fast it falls back."),
    Effect::app("spectrum", "Spectrum", Drawn::Spectrum, Group::Audio, EffectColors::Palette, true,
        "A spectrum analyzer of the sound this PC plays: bass in the middle, treble at the ends, colored from the first color to the last. Speed sets how fast it falls back."),
    Effect::app("battery", "Battery", Drawn::Battery, Group::Status, EffectColors::None, false,
        "The battery level as a bar from the left: green when full, yellow, then red when low. A pulse runs along it while charging."),
    Effect::app("cpuload", "CPU load", Drawn::CpuLoad, Group::Status, EffectColors::None, false,
        "A gauge from the middle: wider and redder as the CPU gets busier."),
    Effect::app("cputemp", "CPU temperature", Drawn::CpuTemperature, Group::Status, EffectColors::None, false,
        "A gauge from the middle: wider and redder as the CPU heats up (35–95 °C)."),
    Effect::app("gputemp", "GPU temperature", Drawn::GpuTemperature, Group::Status, EffectColors::None, false,
        "A gauge from the middle: wider and redder as the NVIDIA GPU heats up (35–90 °C). Only a dim middle LED while the GPU is switched off."),
];

/// The keyboard's fixed slots for its Rainbow, and the light bar's seven-color Rainbow palette.
pub const KEYBOARD_RAINBOW: [Rgb; 4] = [Rgb::new(255, 0, 0), Rgb::new(0, 255, 0), Rgb::new(0, 0, 255), Rgb::new(255, 0, 255)];
pub const LIGHT_BAR_RAINBOW: [Rgb; 7] = [
    Rgb::new(255, 0, 0),
    Rgb::new(255, 165, 0),
    Rgb::new(255, 255, 0),
    Rgb::new(0, 255, 0),
    Rgb::new(0, 255, 255),
    Rgb::new(0, 0, 255),
    Rgb::new(139, 0, 255),
];
