//! The UEFI variable Control Center keeps its BIOS settings in (`UniWillVariable`, Control Center's `NVRAM_STRUCT`,
//! written by its UEFI_Firmware.dll): the lighting the keyboard and light bar show while the laptop starts, and the
//! CPU core voltage offset the BIOS applies at boot. It needs administrator rights.
//!
//! Control Center rewrites all 180 bytes from a copy it read earlier; this app reads the variable, changes only the
//! bytes it means to, writes it back with the same attributes and reads it again to check.

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
use windows::Win32::Security::{AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::System::WindowsProgramming::{GetFirmwareEnvironmentVariableExW, SetFirmwareEnvironmentVariableExW};
use windows::core::w;

const NAME: windows::core::PCWSTR = w!("UniWillVariable");
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

    /// Set by the BIOS when it started without the tuning after the laptop failed to start with it.
    pub fn recovered(&self) -> bool {
        self.bytes[OVERCLOCK_RECOVERY] == 1
    }
}

/// Reads the variable.
pub fn read() -> Result<Variable> {
    enable_privilege()?;
    let mut bytes = vec![0u8; 1024];
    let mut attributes = 0u32;
    let size = unsafe { GetFirmwareEnvironmentVariableExW(NAME, GUID, Some(bytes.as_mut_ptr() as _), bytes.len() as u32, Some(&mut attributes)) };
    if size == 0 {
        return Err(windows::core::Error::from_thread()).context("Couldn't read the BIOS settings variable");
    }
    bytes.truncate(size as usize);
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
    unsafe { SetFirmwareEnvironmentVariableExW(NAME, GUID, Some(variable.bytes.as_ptr() as _), variable.bytes.len() as u32, variable.attributes) }
        .context("Couldn't write the BIOS settings variable")?;
    let written = read()?;
    if written.bytes != variable.bytes {
        bail!("The BIOS settings variable didn't keep the change");
    }
    Ok(written)
}

/// Reading and writing firmware variables needs SeSystemEnvironmentPrivilege enabled in the process token.
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
    }
}
