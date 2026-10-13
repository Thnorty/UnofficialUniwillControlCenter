//! Drives an ITE lighting controller directly over HID feature reports, with the same commands Control Center's
//! GCUService sends (recovered from its decompiled LightingModel.LM_ITE_RGB): the four-zone keyboard (ITE 8291) or
//! the light bar on the front edge (LM_ITE_RGB type MEZone_Lighbar). Every command is a 9-byte feature report:
//! report ID 0, then 8 bytes.

use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList,
    SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HIDD_ATTRIBUTES, HIDP_CAPS, HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetFeature, HidD_GetHidGuid, HidD_GetPreparsedData,
    HidD_SetFeature, HidP_GetCaps, PHIDP_PREPARSED_DATA,
};
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, WriteFile};
use windows::core::PCWSTR;

use crate::color::Rgb;
use crate::lighting::effects::{self, Effect, EffectColors};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Kind {
    Keyboard,
    LightBar,
}

const ITE_VENDOR_ID: u16 = 0x048D;
// ITE_SPEC.PIDList and LBPIDList.
const KEYBOARD_PRODUCT_IDS: &[u16] = &[0xCE00, 0x6000, 0x6001, 0x6002, 0x6003, 0x6004, 0x6006, 0x6007, 0x600A];
const LIGHT_BAR_PRODUCT_IDS: &[u16] = &[0x6005, 0x6008, 0x6010];
const FOUR_ZONE_USAGE_PAGE: u16 = 0xFF12; // ITE_SPEC.USAGE_PAGE_4Zone
const LIGHT_BAR_USAGE_PAGE: u16 = 0xFF03; // ITE_SPEC.USAGE_PAGE_Ligbar
const CONTROL_USAGE: u16 = 0x0001;

const CMD_SET_EFFECT: u8 = 0x08;
const CMD_SET_BRIGHTNESS: u8 = 0x09;
const CMD_SET_COLOR: u8 = 0x14;
const CMD_GET_FIRMWARE_VERSION: u8 = 0x80;
const CMD_GET_EFFECT: u8 = 0x88;
const CMD_SET_ROW: u8 = 0x12; // HID_Set_RowIndex_12H: the next output report fills this row of user-mode LEDs

const EFFECT_USER_MODE: u8 = 51; // ITE_SPEC.EFFECT_USERMODE: each LED set by software
const LEDS_PER_ROW: usize = 21; // Set_ITE_Effect_Type_UserMode_Lightbar12h: 21 LEDs (B, G, R) per output report

/// LEDs of the light bar in user mode (Set_Lighting_Effect sends the first 23).
pub const LIGHT_BAR_LEDS: usize = 23;

const CONTROL_OFF: u8 = 1;
const CONTROL_LIGHTING: u8 = 2;
const CONTROL_RESET: u8 = 5;
const COLOR_INDEX_SLOT1: u8 = 1; // effect uses the single color in slot 1
const COLOR_INDEX_PALETTE: u8 = 8; // effect reads the circular palette loaded into slots 1..n
const PALETTE_SLOTS: usize = 7;

/// Highest firmware brightness. Control Center's four levels are 8, 22, 36 and 50.
pub const MAX_LIGHT: u8 = 50;

// LM_ITE_RGB.Translate_ITE_SpeedValue (slider 0-4 -> firmware value).
const SPEED_VALUES: [u8; 5] = [10, 7, 5, 3, 1];

struct OwnedHandle(HANDLE);

// The handle is only used under the device's lock.
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct State {
    last_known_light: i32,
    is_off: bool,       // switched off with control 1, which 09h can't undo
    current_light: u8,  // the brightness last sent, including fades and Music
}

pub struct Device {
    pub kind: Kind,
    handle: OwnedHandle,
    feature_length: usize,
    output_length: usize,
    pub firmware: (u8, u8),
    // Commands come from the UI thread, the brightness watcher and the effects thread; a query must not interleave
    // with a command sequence.
    state: Mutex<State>,
}

impl Device {
    /// Finds and opens the controller, or returns None if this laptop doesn't have one.
    pub fn open(kind: Kind) -> Result<Option<Device>> {
        let (product_ids, usage_page) = match kind {
            Kind::Keyboard => (KEYBOARD_PRODUCT_IDS, FOUR_ZONE_USAGE_PAGE),
            Kind::LightBar => (LIGHT_BAR_PRODUCT_IDS, LIGHT_BAR_USAGE_PAGE),
        };
        for path in hid_device_paths() {
            let Some(handle) = open_path(&path) else { continue };
            let mut attributes = HIDD_ATTRIBUTES { Size: size_of::<HIDD_ATTRIBUTES>() as u32, ..Default::default() };
            if !unsafe { HidD_GetAttributes(handle.0, &mut attributes) }
                || attributes.VendorID != ITE_VENDOR_ID
                || !product_ids.contains(&attributes.ProductID)
            {
                continue;
            }
            let Some(caps) = caps(&handle) else { continue };
            if caps.UsagePage != usage_page || caps.Usage != CONTROL_USAGE {
                continue;
            }
            let mut device = Device {
                kind,
                handle,
                feature_length: caps.FeatureReportByteLength as usize,
                output_length: caps.OutputReportByteLength as usize,
                firmware: (0, 0),
                state: Mutex::new(State { last_known_light: -1, is_off: false, current_light: 0 }),
            };
            let reply = device.query(CMD_GET_FIRMWARE_VERSION)?;
            device.firmware = (reply[2], reply[3]);
            return Ok(Some(device));
        }
        Ok(None)
    }

    // FourZoneKeyboard.SetBrightnessStrategy: keyboard firmware >= 0x11.03 takes brightness through command 09h.
    // The light bar's driver (HIDKeyboard.SetBrightnessStrategy) always uses it.
    fn has_brightness_command(&self) -> bool {
        self.kind == Kind::LightBar || (self.firmware.0 >= 0x11 && self.firmware.1 >= 0x03)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Shows an effect. `light` is the firmware brightness, 0-MAX_LIGHT (0 keeps the effect running but dark, as
    /// Control Center's brightness 0 does); speed is 0-4 like Control Center's slider.
    pub fn set_effect(&self, effect: &Effect, colors: &[Rgb], direction: u8, light: u8, speed: u8, save: bool) -> Result<()> {
        let mut state = self.lock();
        let light = light.min(MAX_LIGHT);
        state.is_off = false;
        self.send(&[CMD_SET_EFFECT, CONTROL_RESET])?;
        let mut colors = colors;
        if effect.is_rainbow() {
            match self.kind {
                Kind::LightBar => colors = &effects::LIGHT_BAR_RAINBOW,
                Kind::Keyboard => {
                    // Set_Lighting_Effect loads these fixed slots for the four-zone Rainbow, then starts it with no palette.
                    for (i, color) in effects::KEYBOARD_RAINBOW.iter().enumerate() {
                        self.set_color(i as u8 + 1, *color)?;
                    }
                    colors = &[];
                }
            }
        }
        let color_index = if colors.is_empty() {
            0
        } else if colors.len() == 1 && self.kind == Kind::Keyboard {
            // Set_ITE_Effect_Type_FwMode with a non-circular color: one color in slot 1.
            self.set_color(1, colors[0])?;
            COLOR_INDEX_SLOT1
        } else {
            // A circular palette. Effects that cycle colors read all the slots, so a shorter palette is repeated
            // (the light bar's driver always sends circular palettes, even for one color).
            let slots = if effect.colors == EffectColors::Palette { PALETTE_SLOTS.max(colors.len()) } else { colors.len() };
            for i in 0..slots {
                self.set_color(i as u8 + 1, colors[i % colors.len()])?;
            }
            COLOR_INDEX_PALETTE
        };
        // Effects drawn by this app run on top of the static effect (Music varies the brightness, the others the
        // zone colors through set_zone_colors).
        let effect_id = if effect.drawn_by_app() { effects::STATIC_ID } else { effect.id };
        state.current_light = light;
        self.send(&[
            CMD_SET_EFFECT,
            CONTROL_LIGHTING,
            effect_id,
            SPEED_VALUES[speed.min(4) as usize],
            light,
            color_index,
            if effect.has_direction { direction } else { 0 },
            save as u8,
        ])?;
        if self.has_brightness_command() {
            self.send(&[CMD_SET_BRIGHTNESS, CONTROL_LIGHTING, light])?;
        }
        state.last_known_light = light as i32;
        Ok(())
    }

    /// Switches the lights off (ILM_RGBKB_SetPower Off). Any later effect switches them back on.
    pub fn switch_off(&self) -> Result<()> {
        let mut state = self.lock();
        self.send(&[CMD_SET_EFFECT, CONTROL_RESET])?;
        self.send(&[CMD_SET_EFFECT, CONTROL_OFF])?;
        state.is_off = true;
        state.last_known_light = 0;
        state.current_light = 0;
        Ok(())
    }

    /// Switches the light bar to user mode, where software sets each LED with set_leds, all dark to start with
    /// (Set_ITE_Effect_Type_UserMode_Lightbar12h with bRefresh).
    pub fn enter_user_mode(&self, light: u8) -> Result<()> {
        let mut state = self.lock();
        state.is_off = false;
        let light = light.min(MAX_LIGHT);
        self.send(&[CMD_SET_EFFECT, CONTROL_RESET])?;
        self.send(&[CMD_SET_EFFECT, CONTROL_LIGHTING, EFFECT_USER_MODE, 0, light, 0, 0, 0])?;
        if self.has_brightness_command() {
            self.send(&[CMD_SET_BRIGHTNESS, CONTROL_LIGHTING, light])?;
        }
        self.write_leds(&[Rgb::BLACK; LIGHT_BAR_LEDS])?;
        state.last_known_light = light as i32;
        state.current_light = light;
        Ok(())
    }

    /// Changes the keyboard's four zone colors while the static effect runs, for effects drawn by this app. This is
    /// how Control Center changes colors over time too (ChangeSingleTheme resends the static effect).
    pub fn set_zone_colors(&self, zones: &[Rgb]) -> Result<()> {
        let state = self.lock();
        if state.is_off {
            return Ok(());
        }
        for (i, color) in zones.iter().enumerate() {
            self.set_color(i as u8 + 1, *color)?;
        }
        let light = state.current_light;
        self.send(&[CMD_SET_EFFECT, CONTROL_LIGHTING, effects::STATIC_ID, SPEED_VALUES[2], light, COLOR_INDEX_PALETTE, 0, 0])?;
        if self.has_brightness_command() {
            self.send(&[CMD_SET_BRIGHTNESS, CONTROL_LIGHTING, light])?;
        }
        Ok(())
    }

    /// Sets every LED of the light bar while it's in user mode, from one end to the other.
    pub fn set_leds(&self, leds: &[Rgb]) -> Result<()> {
        let _state = self.lock();
        self.write_leds(leds)
    }

    fn write_leds(&self, leds: &[Rgb]) -> Result<()> {
        for (row, chunk) in leds.chunks(LEDS_PER_ROW).enumerate() {
            let mut report = vec![0u8; self.output_length.max(2 + LEDS_PER_ROW * 3)];
            for (i, color) in chunk.iter().enumerate() {
                let (r, g, b) = light_bar_white_balance(*color);
                report[2 + i * 3] = b;
                report[3 + i * 3] = g;
                report[4 + i * 3] = r;
            }
            self.send(&[CMD_SET_ROW, 0, row as u8])?;
            unsafe { WriteFile(self.handle.0, Some(&report), None, None) }.context("The light bar did not accept the LED colors")?;
        }
        Ok(())
    }

    /// The controller's state: whether it's switched on, and its effect ID.
    pub fn read_state(&self) -> Result<(bool, u8)> {
        let _state = self.lock();
        let reply = self.query(CMD_GET_EFFECT)?;
        Ok((reply[2] != CONTROL_OFF, reply[3]))
    }

    /// Changes only the brightness (command 09h, as GCUService does for Fn keys). Returns false when that isn't
    /// possible because the firmware lacks 09h or the lights were switched off, so the caller must resend the effect.
    pub fn try_set_light(&self, light: u8) -> Result<bool> {
        let mut state = self.lock();
        if !self.has_brightness_command() || state.is_off {
            return Ok(false);
        }
        let light = light.min(MAX_LIGHT);
        self.send(&[CMD_SET_BRIGHTNESS, CONTROL_LIGHTING, light])?;
        state.last_known_light = light as i32;
        state.current_light = light;
        Ok(true)
    }

    /// Sets the firmware brightness without changing the value this app tracks, for software-driven changes such
    /// as Music and fades. Does nothing while the lights are switched off.
    pub fn set_raw_light(&self, light: u8) -> Result<()> {
        let mut state = self.lock();
        if !self.has_brightness_command() || state.is_off {
            return Ok(());
        }
        state.current_light = light.min(MAX_LIGHT);
        self.send(&[CMD_SET_BRIGHTNESS, CONTROL_LIGHTING, state.current_light])
    }

    /// The firmware brightness (0-MAX_LIGHT) the controller reports; 0 if it's switched off.
    pub fn read_light(&self) -> Result<u8> {
        let mut state = self.lock();
        self.read_light_locked(&mut state)
    }

    fn read_light_locked(&self, state: &mut State) -> Result<u8> {
        let reply = self.query(CMD_GET_EFFECT)?; // control, effect, speed, light, color index, direction
        state.is_off = reply[2] == CONTROL_OFF;
        let light = if state.is_off { 0 } else { reply[5].min(MAX_LIGHT) };
        state.last_known_light = light as i32;
        Ok(light)
    }

    /// Returns the new firmware brightness if something other than this app changed it (the Fn keys are handled by
    /// the laptop's firmware when this app doesn't own them), otherwise None.
    pub fn poll_external_light_change(&self) -> Result<Option<u8>> {
        let mut state = self.lock();
        let previous = state.last_known_light;
        let light = self.read_light_locked(&mut state)?;
        Ok((previous >= 0 && light as i32 != previous).then_some(light))
    }

    fn set_color(&self, slot: u8, color: Rgb) -> Result<()> {
        let (r, g, b) = match self.kind {
            Kind::Keyboard => keyboard_white_balance(color),
            Kind::LightBar => light_bar_white_balance(color),
        };
        self.send(&[CMD_SET_COLOR, 0, slot, r, g, b])
    }

    fn send(&self, command: &[u8]) -> Result<()> {
        let mut report = vec![0u8; self.feature_length.max(9)];
        report[1..1 + command.len()].copy_from_slice(command); // report[0] is the report ID (0)
        if !unsafe { HidD_SetFeature(self.handle.0, report.as_ptr() as _, report.len() as u32) } {
            bail!("The controller did not accept the command: {}", windows::core::Error::from_thread());
        }
        std::thread::sleep(Duration::from_millis(1)); // the service waits 1 ms between reports too
        Ok(())
    }

    fn query(&self, command: u8) -> Result<Vec<u8>> {
        self.send(&[command])?;
        let mut reply = vec![0u8; self.feature_length.max(9)];
        if !unsafe { HidD_GetFeature(self.handle.0, reply.as_mut_ptr() as _, reply.len() as u32) } {
            bail!("The controller did not answer: {}", windows::core::Error::from_thread());
        }
        Ok(reply)
    }
}

/// WKDColor.cheatRGB_4Zone: the LEDs' green and blue are much stronger than red, so the service scales them down.
/// This is the branch for every project except IDs 6 and 7 (this laptop reports 15).
fn keyboard_white_balance(c: Rgb) -> (u8, u8, u8) {
    match (c.r, c.g, c.b) {
        (255, 100, 0) => (255, 40, 0),
        (255, 165, 0) => (255, 70, 0),
        (r, g, b) => (r, (100 * g as u32 / 255) as u8, (200 * b as u32 / 255) as u8),
    }
}

/// WKDColor.cheatRGB_HIDLightbar: the light bar's own color correction.
fn light_bar_white_balance(c: Rgb) -> (u8, u8, u8) {
    match (c.r, c.g, c.b) {
        (255, 255, 255) => (220, 120, 110),
        (255, 165, 0) => (220, 30, 0),
        (255, 255, 0) => (220, 120, 0),
        (0, 255, 255) => (0, 60, 110),
        (139, 0, 255) => (220, 0, 110),
        (r, g, b) => ((220 * r as u32 / 255) as u8, (120 * g as u32 / 255) as u8, (110 * b as u32 / 255) as u8),
    }
}

fn hid_device_paths() -> Vec<Vec<u16>> {
    let mut paths = Vec::new();
    unsafe {
        let guid = HidD_GetHidGuid();
        let Ok(set) = SetupDiGetClassDevsW(Some(&guid), PCWSTR::null(), None, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) else {
            return paths;
        };
        let mut data = SP_DEVICE_INTERFACE_DATA { cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default() };
        let mut index = 0;
        while SetupDiEnumDeviceInterfaces(set, None, &guid, index, &mut data).is_ok() {
            index += 1;
            let mut size = 0u32;
            let _ = SetupDiGetDeviceInterfaceDetailW(set, &data, None, 0, Some(&mut size), None);
            if size == 0 {
                continue;
            }
            // u32 storage keeps the detail structure aligned.
            let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
            let detail = buffer.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(set, &data, Some(detail), size, None, None).is_ok() {
                let start = std::ptr::addr_of!((*detail).DevicePath) as *const u16;
                let mut length = 0;
                while *start.add(length) != 0 {
                    length += 1;
                }
                let mut path = std::slice::from_raw_parts(start, length).to_vec();
                path.push(0);
                paths.push(path);
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
    }
    paths
}

fn open_path(path: &[u16]) -> Option<OwnedHandle> {
    unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .ok()
        .map(OwnedHandle)
    }
}

fn caps(handle: &OwnedHandle) -> Option<HIDP_CAPS> {
    unsafe {
        let mut data = PHIDP_PREPARSED_DATA::default();
        if !HidD_GetPreparsedData(handle.0, &mut data) {
            return None;
        }
        let mut caps = HIDP_CAPS::default();
        let ok = HidP_GetCaps(data, &mut caps).is_ok();
        let _ = HidD_FreePreparsedData(data);
        ok.then_some(caps)
    }
}
