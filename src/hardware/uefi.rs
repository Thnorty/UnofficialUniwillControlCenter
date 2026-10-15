//! The UEFI variable Control Center keeps its BIOS settings in (`UniWillVariable`, Control Center's `NVRAM_STRUCT`,
//! written by its UEFI_Firmware.dll): the lighting the keyboard and light bar show while the laptop starts, the CPU
//! core voltage offset the BIOS applies at boot, and the GPU mode (the MUX switch). It needs administrator rights.
//!
//! Control Center rewrites all 180 bytes from a copy it read earlier; this app reads the variable, changes only the
//! bytes it means to, writes it back with the same attributes and reads it again to check.

#[cfg(windows)]
use anyhow::Context;
use anyhow::{Result, bail};
#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
#[cfg(windows)]
use windows::Win32::Security::{AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY};
#[cfg(windows)]
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
#[cfg(windows)]
use windows::Win32::System::WindowsProgramming::{GetFirmwareEnvironmentVariableExW, SetFirmwareEnvironmentVariableExW};
#[cfg(windows)]
use windows::core::w;

#[cfg(windows)]
const NAME: windows::core::PCWSTR = w!("UniWillVariable");
#[cfg(windows)]
const GUID: windows::core::PCWSTR = w!("{9f33f85c-13ca-4fd1-9c4a-96217722c593}");
/// NVRAM_STRUCT's size with C# default (natural) alignment, as this laptop's variable is.
const SIZE: usize = 180;

// Byte offsets in NVRAM_STRUCT.
const KEYBOARD_TIMEOUT: usize = 15; // RGBKeyboard1A: the 1Ah command the BIOS sends at boot
const KEYBOARD_EFFECT: usize = 23; // RGBKeyboard08: the 08h command the BIOS sends at boot
const LIGHT_BAR_TIMEOUT: usize = 31; // SmartLightbar1A
const LIGHT_BAR_EFFECT: usize = 39; // SmartLightbar08
const VOLTAGE_OFFSET: usize = 58; // ICpuCoreVoltageOffsetValue, mV up
const VOLTAGE_OFFSET_MAXIMUM: usize = 60; // ICpuCoreVoltageOffsetMaximum
const OVERCLOCK_RECOVERY: usize = 92; // OverClockRecoveryFlag: the BIOS reset the tuning after a failed start
const DISPLAY_MODE: usize = 98; // OemDisplayMode: the GPU mode, as the BIOS setup's own option; 255 without a MUX
const VOLTAGE_OFFSET_NEGATIVE: usize = 100; // ICpuCoreVoltageOffsetNegativeValue, mV down
const VOLTAGE_OFFSET_RANGE: usize = 102; // ICpuCoreVoltageOffsetRangeType: 0 up only, 1 down only, 2 both

/// Which device's startup lighting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Startup {
    Keyboard,
    LightBar,
}

impl Startup {
    fn offsets(self) -> (usize, usize) {
        match self {
            Startup::Keyboard => (KEYBOARD_TIMEOUT, KEYBOARD_EFFECT),
            Startup::LightBar => (LIGHT_BAR_TIMEOUT, LIGHT_BAR_EFFECT),
        }
    }
}

/// Which GPU drives the screen from the next start: the BIOS setup's MSHybrid, iGPU and dGPU (OemDisplayMode on Intel
/// laptops; Control Center switches between Hybrid and Discrete only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuMode {
    /// The integrated graphics drive the screen and the NVIDIA GPU works for the apps that need it (Optimus).
    Hybrid,
    /// The integrated graphics (Intel or AMD) alone; the NVIDIA GPU is switched off.
    Integrated,
    /// The NVIDIA GPU drives the screen.
    Discrete,
}

impl GpuMode {
    pub const ALL: [GpuMode; 3] = [GpuMode::Hybrid, GpuMode::Integrated, GpuMode::Discrete];

    fn byte(self) -> u8 {
        match self {
            GpuMode::Hybrid => 4,
            GpuMode::Integrated => 1,
            GpuMode::Discrete => 2,
        }
    }

    fn from_byte(byte: u8) -> Option<GpuMode> {
        GpuMode::ALL.into_iter().find(|m| m.byte() == byte)
    }
}

/// The variable as read.
#[derive(Clone, Debug)]
pub struct Variable {
    bytes: Vec<u8>,
    attributes: u32,
}

/// The CPU core voltage offset the BIOS applies at boot, in mV, and the range it allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoltageOffset {
    pub value: i32,
    pub minimum: i32,
    pub maximum: i32,
}

/// Larger offsets than this aren't written whatever the BIOS reports.
const VOLTAGE_LIMIT: i32 = 200;

impl Variable {
    fn u16(&self, offset: usize) -> u16 {
        u16::from_le_bytes([self.bytes[offset], self.bytes[offset + 1]])
    }

    /// The two 8-byte commands the BIOS sends to a lighting controller at boot: the effect (08h) and its timeout (1Ah).
    pub fn startup(&self, device: Startup) -> ([u8; 8], [u8; 8]) {
        let (timeout, effect) = device.offsets();
        (self.bytes[effect..effect + 8].try_into().unwrap(), self.bytes[timeout..timeout + 8].try_into().unwrap())
    }

    /// The voltage offset, or None if the BIOS doesn't offer one (MyFanManager_RamFan1p5.LoadProfileAll).
    pub fn voltage_offset(&self) -> Option<VoltageOffset> {
        let maximum = (self.u16(VOLTAGE_OFFSET_MAXIMUM) as i32).min(VOLTAGE_LIMIT);
        let (minimum, maximum) = match self.bytes[VOLTAGE_OFFSET_RANGE] {
            0 => (0, maximum),
            1 => (-maximum, 0),
            2 => (-maximum, maximum),
            _ => return None,
        };
        if minimum == maximum {
            return None;
        }
        let value = self.u16(VOLTAGE_OFFSET) as i32 - self.u16(VOLTAGE_OFFSET_NEGATIVE) as i32;
        Some(VoltageOffset { value, minimum, maximum })
    }

    /// The GPU mode, or None if the laptop has no MUX switch (or the BIOS keeps a value this app doesn't know).
    pub fn gpu_mode(&self) -> Option<GpuMode> {
        GpuMode::from_byte(self.bytes[DISPLAY_MODE])
    }

    /// Set by the BIOS when it started without the tuning after the laptop failed to start with it.
    pub fn recovered(&self) -> bool {
        self.bytes[OVERCLOCK_RECOVERY] == 1
    }
}

/// Reads the variable.
pub fn read() -> Result<Variable> {
    let (attributes, bytes) = read_raw()?;
    if bytes.len() != SIZE {
        bail!("The BIOS settings variable is {} bytes, not the {SIZE} this app knows", bytes.len());
    }
    Ok(Variable { bytes, attributes })
}

/// Writes the startup lighting commands for a device.
pub fn set_startup(device: Startup, effect: [u8; 8], timeout: [u8; 8]) -> Result<Variable> {
    let (timeout_at, effect_at) = device.offsets();
    update(|v| {
        // Only over what's expected there: a command, or an empty slot.
        let (old_effect, old_timeout) = v.startup(device);
        if ![0x00, 0x08, 0xFF].contains(&old_effect[0]) || ![0x00, 0x1A, 0xFF].contains(&old_timeout[0]) {
            bail!("The BIOS settings variable doesn't look as expected, so it wasn't changed");
        }
        v.bytes[effect_at..effect_at + 8].copy_from_slice(&effect);
        v.bytes[timeout_at..timeout_at + 8].copy_from_slice(&timeout);
        Ok(())
    })
}

/// Sets the voltage offset for the next start, as SetCpuCoreVoltageOffset: up in one field, down in the other.
pub fn set_voltage_offset(mv: i32) -> Result<Variable> {
    update(|v| {
        let Some(range) = v.voltage_offset() else { bail!("This BIOS doesn't offer a voltage offset") };
        if !(range.minimum..=range.maximum).contains(&mv) {
            bail!("{mv} mV is outside what the BIOS allows ({} to {} mV)", range.minimum, range.maximum);
        }
        v.bytes[VOLTAGE_OFFSET..VOLTAGE_OFFSET + 2].copy_from_slice(&(mv.max(0) as u16).to_le_bytes());
        v.bytes[VOLTAGE_OFFSET_NEGATIVE..VOLTAGE_OFFSET_NEGATIVE + 2].copy_from_slice(&((-mv).max(0) as u16).to_le_bytes());
        Ok(())
    })
}

/// Sets the GPU mode for the next start (UserSetDGgpuDirectConnectionSwitch), only over a mode this app knows.
pub fn set_gpu_mode(mode: GpuMode) -> Result<Variable> {
    update(|v| {
        if v.gpu_mode().is_none() {
            bail!("This laptop's BIOS doesn't have a GPU mode this app knows, so it wasn't changed");
        }
        v.bytes[DISPLAY_MODE] = mode.byte();
        Ok(())
    })
}

/// Clears the recovery flag once the tuning has been reset (DetectRecoveryFlagFromBiosVariable).
pub fn clear_recovery() -> Result<Variable> {
    update(|v| {
        v.bytes[OVERCLOCK_RECOVERY] = 0;
        Ok(())
    })
}

fn update(change: impl FnOnce(&mut Variable) -> Result<()>) -> Result<Variable> {
    let mut variable = read()?;
    let before = variable.bytes.clone();
    change(&mut variable)?;
    if variable.bytes == before {
        return Ok(variable);
    }
    write_raw(variable.attributes, &variable.bytes)?;
    let written = read()?;
    if written.bytes != variable.bytes {
        bail!("The BIOS settings variable didn't keep the change");
    }
    Ok(written)
}

/// The variable's attributes and bytes, through the app's helper (efivarfs is root's).
#[cfg(target_os = "linux")]
fn read_raw() -> Result<(u32, Vec<u8>)> {
    use super::helper_protocol::{Reply, Request};
    match super::ec::request(Request::ReadVariable)? {
        Reply::Variable { attributes, bytes } => Ok((attributes, bytes)),
        Reply::Error { message } | Reply::Refused { message } => bail!(message),
        other => bail!("The app's helper answered {other:?}"),
    }
}

#[cfg(target_os = "linux")]
fn write_raw(attributes: u32, bytes: &[u8]) -> Result<()> {
    use super::helper_protocol::{Reply, Request};
    match super::ec::request(Request::WriteVariable { attributes, bytes: bytes.to_vec() })? {
        Reply::Done => Ok(()),
        Reply::Error { message } | Reply::Refused { message } => bail!(message),
        other => bail!("The app's helper answered {other:?}"),
    }
}

/// The variable's attributes and bytes.
#[cfg(windows)]
fn read_raw() -> Result<(u32, Vec<u8>)> {
    enable_privilege()?;
    let mut bytes = vec![0u8; 1024];
    let mut attributes = 0u32;
    let size = unsafe { GetFirmwareEnvironmentVariableExW(NAME, GUID, Some(bytes.as_mut_ptr() as _), bytes.len() as u32, Some(&mut attributes)) };
    if size == 0 {
        return Err(windows::core::Error::from_thread()).context("Couldn't read the BIOS settings variable");
    }
    bytes.truncate(size as usize);
    Ok((attributes, bytes))
}

#[cfg(windows)]
fn write_raw(attributes: u32, bytes: &[u8]) -> Result<()> {
    unsafe { SetFirmwareEnvironmentVariableExW(NAME, GUID, Some(bytes.as_ptr() as _), bytes.len() as u32, attributes) }
        .context("Couldn't write the BIOS settings variable")
}

/// Reading and writing firmware variables needs SeSystemEnvironmentPrivilege enabled in the process token.
#[cfg(windows)]
fn enable_privilege() -> Result<()> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token).context("Couldn't open the process token")?;
        let mut luid = LUID::default();
        let result = LookupPrivilegeValueW(None, w!("SeSystemEnvironmentPrivilege"), &mut luid).and_then(|()| {
            let privileges = TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }] };
            AdjustTokenPrivileges(token, false, Some(&privileges), 0, None, None)
        });
        let _ = CloseHandle(token);
        result.context("Couldn't enable access to firmware variables (the app needs administrator rights)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // This laptop's variable as Control Center left it.
    const READ: &str = concat!(
        "051D16110000000000FE00000000001A0001040000000008021305320800011A00010400000000080221052400000100",
        "000000000000D00700000000640000000000000000000000000000000000000000000000000000000000000000010000",
        "000101000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
        "000000000000000000000000000000000000000000000000000000000000000000000000",
    );

    fn sample() -> Variable {
        let bytes: Vec<u8> = (0..READ.len() / 2).map(|i| u8::from_str_radix(&READ[i * 2..i * 2 + 2], 16).unwrap()).collect();
        assert_eq!(bytes.len(), SIZE);
        Variable { bytes, attributes: 7 }
    }

    #[test]
    fn fields() {
        let v = sample();
        assert_eq!(v.startup(Startup::Keyboard), ([0x08, 0x02, 0x13, 0x05, 0x32, 0x08, 0x00, 0x01], [0x1A, 0, 1, 4, 0, 0, 0, 0]));
        assert_eq!(v.startup(Startup::LightBar).0, [0x08, 0x02, 0x21, 0x05, 0x24, 0x00, 0x00, 0x01]);
        assert_eq!(v.voltage_offset(), Some(VoltageOffset { value: 0, minimum: 0, maximum: 100 }));
        assert!(!v.recovered());
        // Saved while the BIOS setup had iGPU; it now has MSHybrid (4).
        assert_eq!(v.gpu_mode(), Some(GpuMode::Integrated));
        let mut hybrid = v.clone();
        hybrid.bytes[DISPLAY_MODE] = 4;
        assert_eq!(hybrid.gpu_mode(), Some(GpuMode::Hybrid));
        hybrid.bytes[DISPLAY_MODE] = 0xFF;
        assert_eq!(hybrid.gpu_mode(), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn read_goes_through_the_helper() {
        let _serial = crate::hardware::fake_helper::serial();
        let original = sample();
        let bytes = original.bytes.clone();
        let _helper = crate::hardware::fake_helper::FakeHelper::start("uefi-read", |s| s.variable = Some((7, bytes)));
        let read = read().unwrap();
        assert_eq!((read.attributes, read.bytes), (7, original.bytes));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn set_gpu_mode_writes_only_its_field() {
        let _serial = crate::hardware::fake_helper::serial();
        let before = sample().bytes;
        let bytes = before.clone();
        let helper = crate::hardware::fake_helper::FakeHelper::start("uefi-gpu", |s| s.variable = Some((7, bytes)));
        let written = set_gpu_mode(GpuMode::Hybrid).unwrap();
        assert_eq!(written.gpu_mode(), Some(GpuMode::Hybrid));
        let (attributes, after) = helper.state.lock().unwrap().variable.clone().unwrap();
        assert_eq!(attributes, 7);
        let changed: Vec<usize> = (0..SIZE).filter(|&i| before[i] != after[i]).collect();
        assert_eq!(changed, [DISPLAY_MODE]);
        assert_eq!(read().unwrap().gpu_mode(), Some(GpuMode::Hybrid));
    }
}
