//! The on-screen pop-up for the Fn keys and toggles, drawn like Windows 11's volume and brightness flyout: a small
//! rounded card at the bottom of the screen with an icon, a bar and the value, in the system theme and accent color.
//! It slides up and fades in, follows changes smoothly, then fades out. It never takes focus and clicks pass through.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory,
    ID2D1DCRenderTarget, ID2D1Factory, ID2D1RenderTarget,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDC, GetMonitorInfoW, HBITMAP, HDC, HGDIOBJ, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ReleaseDC, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetCursorPos, HWND_TOPMOST, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SetWindowPos, ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::color::Rgb;
use crate::hardware::system;

pub const KEYBOARD_GLYPH: char = '\u{E765}'; // KeyboardClassic
pub const TOUCHPAD_GLYPH: char = '\u{EFA5}'; // Touchpad
pub const LOCK_GLYPH: char = '\u{E72E}'; // Lock
pub const UNLOCK_GLYPH: char = '\u{E785}'; // Unlock
pub const LIGHT_GLYPH: char = '\u{EA80}'; // Lightbulb
pub const TIMER_GLYPH: char = '\u{E916}'; // Stopwatch
pub const OFFICE_GLYPH: char = '\u{E821}'; // Work
pub const GAMING_GLYPH: char = '\u{E7FC}'; // Game
pub const TURBO_GLYPH: char = '\u{E945}'; // LightningBolt
pub const FAN_GLYPH: char = '\u{E9CA}'; // Frigid

// Layout in logical pixels, as Windows 11 draws its flyout.
const CARD_HEIGHT: f32 = 48.0;
const LEVEL_WIDTH: f32 = 196.0;
const RADIUS: f32 = 8.0;
const SHADOW: f32 = 20.0; // room around the card for its shadow
const BOTTOM_GAP: f32 = 24.0; // above the taskbar

const SHOW_TIME: Duration = Duration::from_millis(250);
const HIDE_TIME: Duration = Duration::from_millis(170);
const STAY_TIME: Duration = Duration::from_millis(1900);
const BAR_TIME: f32 = 0.12; // seconds for the bar to follow a change

#[derive(Clone, PartialEq)]
struct Content {
    glyph: char,
    caption: String,
    /// Bar fill 0-1, or None for a text-only pop-up.
    level: Option<f32>,
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Hidden,
    Showing(Instant),
    Visible,
    Hiding(Instant),
}

struct Theme {
    dark: bool,
    accent: Rgb,
}

struct Osd {
    hwnd: HWND,
    d2d: ID2D1Factory,
    dwrite: IDWriteFactory,
    icon_font: PCWSTR,
    content: Option<Content>,
    phase: Phase,
    hide_at: Instant,
    shown_level: f32,
    /// Where the card sits when fully shown (screen pixels of the window's top-left) and its scale.
    anchor: (i32, i32),
    scale: f32,
    size: (i32, i32),
    theme: Theme,
    timer: slint::Timer,
    last_tick: Instant,
}

/// The pop-up window. Cheap to clone; everything runs on the UI thread.
#[derive(Clone)]
pub struct OsdWindow(Rc<RefCell<Option<Osd>>>);


impl OsdWindow {
    pub fn new() -> OsdWindow {
        OsdWindow(Rc::new(RefCell::new(Osd::create().ok())))
    }

    /// The keyboard brightness as a fraction of the maximum, 0-1.
    pub fn show_brightness(&self, fraction: f32) {
        let percent = (fraction * 100.0).round() as i32;
        self.show(Content { glyph: KEYBOARD_GLYPH, caption: percent.to_string(), level: Some(fraction.clamp(0.0, 1.0)) });
    }

    pub fn show_light_bar(&self, on: bool) {
        self.show_text(LIGHT_GLYPH, if on { "Light bar on" } else { "Light bar off" });
    }

    pub fn show_touchpad(&self, enabled: bool) {
        self.show_text(TOUCHPAD_GLYPH, if enabled { "Touchpad on" } else { "Touchpad off" });
    }

    pub fn show_win_key(&self, locked: bool) {
        if locked {
            self.show_text(LOCK_GLYPH, "Windows key locked")
        } else {
            self.show_text(UNLOCK_GLYPH, "Windows key unlocked")
        }
    }

    pub fn show_text(&self, glyph: char, caption: &str) {
        self.show(Content { glyph, caption: caption.to_string(), level: None });
    }

    fn show(&self, content: Content) {
        let mut guard = self.0.borrow_mut();
        let Some(osd) = guard.as_mut() else { return };
        let now = Instant::now();
        let restart = matches!(osd.phase, Phase::Hidden | Phase::Hiding(_))
            || osd.content.as_ref().is_none_or(|c| c.level.is_some() != content.level.is_some());
        if restart {
            // A new pop-up starts at its value; an open one moves its bar there (see frame).
            osd.shown_level = content.level.unwrap_or(0.0);
        }
        osd.content = Some(content);
        osd.hide_at = now + STAY_TIME;
        if restart {
            osd.theme = Theme { dark: system::system_dark_mode(), accent: accent_fill(system::system_dark_mode()) };
            osd.place();
            osd.phase = Phase::Showing(now);
            unsafe {
                let _ = ShowWindow(osd.hwnd, SW_SHOWNOACTIVATE);
                let _ = SetWindowPos(osd.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE);
            }
        }
        osd.last_tick = now;
        osd.frame(now);
        let weak = Rc::downgrade(&self.0);
        osd.timer.start(slint::TimerMode::Repeated, Duration::from_millis(10), move || {
            if let Some(osd) = weak.upgrade()
                && let Some(osd) = osd.borrow_mut().as_mut()
            {
                osd.frame(Instant::now());
            }
        });
    }
}

/// The accent fill Windows 11 uses for sliders: a lighter shade of the accent in dark mode, a darker one in light mode
/// (from the accent palette Windows keeps in the registry).
pub fn accent_fill(dark: bool) -> Rgb {
    let mut palette = [0u8; 32];
    let mut size = palette.len() as u32;
    let ok = unsafe {
        windows::Win32::System::Registry::RegGetValueW(
            windows::Win32::System::Registry::HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent"),
            w!("AccentPalette"),
            windows::Win32::System::Registry::RRF_RT_REG_BINARY,
            None,
            Some(palette.as_mut_ptr() as _),
            Some(&mut size),
        )
    }
    .is_ok();
    if ok && size >= 32 {
        // Light 3, light 2, light 1, accent, dark 1, dark 2, dark 3, …, as RGBA.
        let i = if dark { 1 } else { 4 } * 4;
        return Rgb::new(palette[i], palette[i + 1], palette[i + 2]);
    }
    let accent = system::accent_color().unwrap_or(Rgb::new(0, 120, 212));
    if dark { accent.lerp(Rgb::WHITE, 0.45) } else { accent.lerp(Rgb::BLACK, 0.15) }
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn ease_in(t: f32) -> f32 {
    t * t
}

impl Osd {
    fn create() -> windows::core::Result<Osd> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("KeyboardRgbOsd");
            let wc = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: instance.into(), lpszClassName: class, ..Default::default() };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
                class,
                w!("Tulpar Control"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let icon_font = if font_exists(&dwrite, w!("Segoe Fluent Icons")) { w!("Segoe Fluent Icons") } else { w!("Segoe MDL2 Assets") };
            Ok(Osd {
                hwnd,
                d2d,
                dwrite,
                icon_font,
                content: None,
                phase: Phase::Hidden,
                hide_at: Instant::now(),
                shown_level: 0.0,
                anchor: (0, 0),
                scale: 1.0,
                size: (1, 1),
                theme: Theme { dark: true, accent: Rgb::new(76, 194, 255) },
                timer: slint::Timer::default(),
                last_tick: Instant::now(),
            })
        }
    }

    /// Picks the screen with the mouse pointer and the size for the content.
    fn place(&mut self) {
        unsafe {
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(monitor, &mut info);
            let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            self.scale = dpi_x as f32 / 96.0;
            let card_width = self.card_width();
            let s = self.scale;
            let width = ((card_width + SHADOW * 2.0) * s).ceil() as i32;
            let height = ((CARD_HEIGHT + SHADOW * 2.0) * s).ceil() as i32;
            let work = info.rcWork;
            let x = work.left + (work.right - work.left - width) / 2;
            let y = work.bottom - ((CARD_HEIGHT + BOTTOM_GAP + SHADOW) * s).round() as i32;
            self.anchor = (x, y);
            self.size = (width, height);
        }
    }

    fn card_width(&self) -> f32 {
        match &self.content {
            Some(Content { level: Some(_), .. }) | None => LEVEL_WIDTH,
            Some(content) => (48.0 + self.text_width(&content.caption) + 20.0).max(160.0),
        }
    }

    fn text_format(&self, size: f32, bold: bool, family: PCWSTR) -> windows::core::Result<IDWriteTextFormat> {
        unsafe {
            let format = self.dwrite.CreateTextFormat(
                family,
                None,
                if bold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL },
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("en-us"),
            )?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            Ok(format)
        }
    }

    fn text_width(&self, text: &str) -> f32 {
        let wide: Vec<u16> = text.encode_utf16().collect();
        (|| -> windows::core::Result<f32> {
            let format = self.text_format(14.0, false, w!("Segoe UI Variable Text"))?;
            let layout = unsafe { self.dwrite.CreateTextLayout(&wide, &format, 1000.0, 100.0)? };
            let mut metrics = DWRITE_TEXT_METRICS::default();
            unsafe { layout.GetMetrics(&mut metrics)? };
            Ok(metrics.widthIncludingTrailingWhitespace)
        })()
        .unwrap_or(text.chars().count() as f32 * 7.5)
    }

    /// Advances the animation and redraws.
    fn frame(&mut self, now: Instant) {
        let delta = (now - self.last_tick).as_secs_f32();
        self.last_tick = now;
        if let Some(target) = self.content.as_ref().and_then(|c| c.level) {
            // Ease towards the new value, like Windows' bar.
            let difference = target - self.shown_level;
            self.shown_level += difference * (delta / BAR_TIME).min(1.0);
            if (target - self.shown_level).abs() < 0.002 {
                self.shown_level = target;
            }
        }
        let (alpha, offset) = match self.phase {
            Phase::Hidden => {
                self.timer.stop();
                return;
            }
            Phase::Showing(start) => {
                let t = ((now - start).as_secs_f32() / SHOW_TIME.as_secs_f32()).min(1.0);
                if t >= 1.0 {
                    self.phase = Phase::Visible;
                }
                let eased = ease_out(t);
                ((eased * 1.6).min(1.0), (1.0 - eased) * 16.0)
            }
            Phase::Visible => {
                if now >= self.hide_at {
                    self.phase = Phase::Hiding(now);
                }
                (1.0, 0.0)
            }
            Phase::Hiding(start) => {
                let t = ((now - start).as_secs_f32() / HIDE_TIME.as_secs_f32()).min(1.0);
                if t >= 1.0 {
                    self.phase = Phase::Hidden;
                    self.timer.stop();
                    unsafe {
                        let _ = ShowWindow(self.hwnd, SW_HIDE);
                    }
                    return;
                }
                (1.0 - ease_in(t), ease_in(t) * 8.0)
            }
        };
        let _ = self.draw(alpha, offset);
    }

    fn draw(&self, alpha: f32, offset: f32) -> windows::core::Result<()> {
        let Some(content) = &self.content else { return Ok(()) };
        let (width, height) = self.size;
        unsafe {
            let screen = GetDC(None);
            let memory = CreateCompatibleDC(Some(screen));
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap: HBITMAP = CreateDIBSection(Some(memory), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let old = SelectObject(memory, HGDIOBJ(bitmap.0));
            let result = self.render(memory, width, height, content);
            if result.is_ok() {
                let position = POINT { x: self.anchor.0, y: self.anchor.1 + (offset * self.scale).round() as i32 };
                let size = SIZE { cx: width, cy: height };
                let source = POINT::default();
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: (alpha.clamp(0.0, 1.0) * 255.0) as u8,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };
                let _ = UpdateLayeredWindow(
                    self.hwnd,
                    Some(screen),
                    Some(&position),
                    Some(&size),
                    Some(memory),
                    Some(&source),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                );
            }
            SelectObject(memory, old);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(memory);
            ReleaseDC(None, screen);
            result
        }
    }

    fn render(&self, dc: HDC, width: i32, height: i32, content: &Content) -> windows::core::Result<()> {
        unsafe {
            let properties = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let target: ID2D1DCRenderTarget = self.d2d.CreateDCRenderTarget(&properties)?;
            target.BindDC(dc, &RECT { left: 0, top: 0, right: width, bottom: height })?;
            let rt: ID2D1RenderTarget = windows::core::Interface::cast(&target)?;
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            rt.BeginDraw();
            rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            self.paint(&rt, content);
            rt.EndDraw(None, None)
        }
    }

    fn paint(&self, rt: &ID2D1RenderTarget, content: &Content) {
        let s = self.scale;
        let dark = self.theme.dark;
        let card_width = self.card_width();
        let left = SHADOW * s;
        let top = SHADOW * s;
        let card = D2D_RECT_F { left, top, right: left + card_width * s, bottom: top + CARD_HEIGHT * s };
        let color = |r: u8, g: u8, b: u8, a: f32| D2D1_COLOR_F { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a };
        let rounded = |rect: D2D_RECT_F, radius: f32| D2D1_ROUNDED_RECT { rect, radiusX: radius, radiusY: radius };
        unsafe {
            let fill = |c: D2D1_COLOR_F, shape: &D2D1_ROUNDED_RECT| {
                if let Ok(brush) = rt.CreateSolidColorBrush(&c, None) {
                    rt.FillRoundedRectangle(shape, &brush);
                }
            };
            // A soft shadow: layered rounded rectangles, faint and wide to dark and close, offset downwards a little.
            let layers = 14;
            for i in 0..layers {
                let spread = (layers - i) as f32 * 1.3 * s;
                let shadow = D2D_RECT_F { left: card.left - spread, top: card.top - spread + 6.0 * s, right: card.right + spread, bottom: card.bottom + spread + 6.0 * s };
                fill(color(0, 0, 0, if dark { 0.035 } else { 0.018 }), &rounded(shadow, RADIUS * s + spread));
            }
            // The card: Windows 11's flyout acrylic, with a hairline border.
            let background = if dark { color(44, 44, 44, 0.97) } else { color(249, 249, 249, 0.97) };
            fill(background, &rounded(card, RADIUS * s));
            if let Ok(border) = rt.CreateSolidColorBrush(&if dark { color(255, 255, 255, 0.08) } else { color(0, 0, 0, 0.07) }, None) {
                let inset = 0.5 * s;
                let edge = D2D_RECT_F { left: card.left + inset, top: card.top + inset, right: card.right - inset, bottom: card.bottom - inset };
                rt.DrawRoundedRectangle(&rounded(edge, RADIUS * s - inset), &border, 1.0 * s, None);
            }

            let text_color = if dark { color(255, 255, 255, 1.0) } else { color(27, 27, 27, 1.0) };
            let Ok(text_brush) = rt.CreateSolidColorBrush(&text_color, None) else { return };
            // Icon.
            if let Ok(icon_format) = self.text_format(16.0 * s, false, self.icon_font) {
                let _ = icon_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
                let glyph: Vec<u16> = content.glyph.to_string().encode_utf16().collect();
                let icon_box = D2D_RECT_F { left: card.left + 14.0 * s, top: card.top, right: card.left + 38.0 * s, bottom: card.bottom };
                rt.DrawText(&glyph, &icon_format, &icon_box, &text_brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            }

            match content.level {
                Some(_) => {
                    // The bar, then the value at the right like Windows' volume number.
                    let bar_left = card.left + 50.0 * s;
                    let bar_right = card.right - 52.0 * s;
                    let middle = (card.top + card.bottom) / 2.0;
                    let track = D2D_RECT_F { left: bar_left, top: middle - 2.0 * s, right: bar_right, bottom: middle + 2.0 * s };
                    fill(if dark { color(255, 255, 255, 0.54) } else { color(0, 0, 0, 0.45) }, &rounded(track, 2.0 * s));
                    let level = self.shown_level.clamp(0.0, 1.0);
                    if level > 0.0 {
                        let filled = D2D_RECT_F { right: bar_left + ((bar_right - bar_left) * level).max(4.0 * s), ..track };
                        let accent = self.theme.accent;
                        fill(color(accent.r, accent.g, accent.b, 1.0), &rounded(filled, 2.0 * s));
                    }
                    if let Ok(number_format) = self.text_format(14.0 * s, false, w!("Segoe UI Variable Text")) {
                        let _ = number_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING);
                        let text: Vec<u16> = content.caption.encode_utf16().collect();
                        let number_box = D2D_RECT_F { left: bar_right + 6.0 * s, top: card.top, right: card.right - 16.0 * s, bottom: card.bottom };
                        rt.DrawText(&text, &number_format, &number_box, &text_brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                    }
                }
                None => {
                    if let Ok(caption_format) = self.text_format(14.0 * s, false, w!("Segoe UI Variable Text")) {
                        let _ = caption_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
                        let text: Vec<u16> = content.caption.encode_utf16().collect();
                        let caption_box = D2D_RECT_F { left: card.left + 48.0 * s, top: card.top, right: card.right - 12.0 * s, bottom: card.bottom };
                        rt.DrawText(&text, &caption_format, &caption_box, &text_brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                    }
                }
            }
        }
    }
}

fn font_exists(dwrite: &IDWriteFactory, family: PCWSTR) -> bool {
    unsafe {
        let mut collection = None;
        if dwrite.GetSystemFontCollection(&mut collection, false).is_err() {
            return false;
        }
        let Some(collection) = collection else { return false };
        let mut index = 0;
        let mut exists = windows::core::BOOL(0);
        collection.FindFamilyName(family, &mut index, &mut exists).is_ok() && exists.as_bool()
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
