//! Performance modes and Fan Boost, as Control Center's MyFanManager_RamFan1p5 sets them: the EC's fan mode byte,
//! and the CPU power limits and TCC offset, each mode's defaults or the user's own within the range the BIOS allows.
//! GPU overclocking, which Control Center can also change, is left alone.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE};
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows::Win32::System::IO::DeviceIoControl;
use windows::core::w;

use super::ec::Registers;

const FAN_MODE: u16 = 0x751; // bit 7 Office, bit 4 Turbo, neither Gaming; bit 6 Fan Boost
const FAN_BOOST_BIT: u8 = 0x40;
const PL1: u16 = 0x783;
const PL2: u16 = 0x784;
const PL4: u16 = 0x785;
const TCC_OFFSET: u16 = 0x786; // ADDR_TimAP_TccOffset_Setting: 0 = the firmware's, else 0x80 | °C
const FAN_SWITCH_SPEED: u16 = 0x787; // ADDR_TimAP_FanSwitchSpeedT100mSec: 0 = the firmware's
const DEFAULTS_GAMING: u16 = 0x730; // PL1, PL2, PL4, GPU D-state
const DEFAULTS_OFFICE: u16 = 0x734;
const DEFAULTS_TURBO: u16 = 0x7A7;
const DEFAULT_TCC: u16 = 0x7D8; // Gaming, Office, Turbo (GetTCCDefaultValue)
const TURBO_SUPPORT: u16 = 0x49F; // bit 1 (MyEcCtrl.GetTurboModeSupport)
const MODE_OPTIONS: u16 = 0x782; // bit 4: the default mode is Gaming, not Office

/// Control Center's operating modes, numbered as it numbers them.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Mode {
    Office = 0,
    Gaming = 1,
    Turbo = 2,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Office, Mode::Gaming, Mode::Turbo];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Office => "Office",
            Mode::Gaming => "Gaming",
            Mode::Turbo => "Turbo",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(index: usize) -> Mode {
        Mode::ALL.get(index).copied().unwrap_or(Mode::Gaming)
    }

    /// MyFanManager_RamFan1p5.SetFanMode
    fn fan_byte(self) -> u8 {
        match self {
            Mode::Office => 0xA0,
            Mode::Gaming => 0x00,
            Mode::Turbo => 0x10,
        }
    }

    fn defaults_address(self) -> u16 {
        match self {
            Mode::Office => DEFAULTS_OFFICE,
            Mode::Gaming => DEFAULTS_GAMING,
            Mode::Turbo => DEFAULTS_TURBO,
        }
    }

    fn default_tcc_address(self) -> u16 {
        match self {
            Mode::Gaming => DEFAULT_TCC,
            Mode::Office => DEFAULT_TCC + 1,
            Mode::Turbo => DEFAULT_TCC + 2,
        }
    }
}

/// What this laptop's EC offers.
#[derive(Clone, Copy, Debug)]
pub struct Support {
    pub turbo: bool,
    pub default_mode: Mode,
}

pub fn support(ec: &Registers) -> Result<Support> {
    Ok(Support {
        turbo: ec.bit(TURBO_SUPPORT, 1)?,
        default_mode: if ec.bit(MODE_OPTIONS, 4)? { Mode::Gaming } else { Mode::Office },
    })
}

/// The mode and Fan Boost the EC is in now (MyFanManager_RamFan1p5.GetFanMode).
pub fn current(ec: &Registers) -> Result<(Mode, bool)> {
    let value = ec.read(FAN_MODE)?;
    let mode = match (value & 0x80 != 0, value & 0x10 != 0) {
        (true, _) => Mode::Office,
        (false, true) => Mode::Turbo,
        _ => Mode::Gaming,
    };
    Ok((mode, value & FAN_BOOST_BIT != 0))
}

/// CPU power limits in watts: PL1 sustained, PL2 boost, PL4 peak.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct PowerLimits {
    pub pl1: u8,
    pub pl2: u8,
    pub pl4: u8,
}

impl PowerLimits {
    /// Within the range and in order, PL1 ≤ PL2 ≤ PL4, raising the later ones as ReCheckCpuPLMinimumValueI does.
    pub fn checked(self, range: &Ranges) -> PowerLimits {
        let pl1 = self.pl1.clamp(MIN_LIMIT, range.pl1.max(MIN_LIMIT));
        let pl2 = self.pl2.clamp(pl1, range.pl2.max(pl1));
        let pl4 = self.pl4.clamp(pl2, range.pl4.max(pl2));
        PowerLimits { pl1, pl2, pl4 }
    }
}

/// A mode's defaults, as the EC keeps them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Defaults {
    pub limits: PowerLimits,
    /// The TCC offset Control Center's profiles start from, °C.
    pub tcc: u8,
}

/// The defaults the EC keeps for `mode` (GetGamingPLDefaultValue, GetTCCDefaultValue and the like).
pub fn defaults(ec: &Registers, mode: Mode) -> Result<Defaults> {
    let base = mode.defaults_address();
    Ok(Defaults {
        limits: PowerLimits { pl1: ec.read(base)?, pl2: ec.read(base + 1)?, pl4: ec.read(base + 2)? },
        tcc: ec.read(mode.default_tcc_address())?,
    })
}

/// The user's CPU tuning for one mode: Control Center's CPU profile settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Tuning {
    /// Use `limits` on the charger rather than the mode's defaults.
    pub custom_limits: bool,
    pub limits: PowerLimits,
    pub tcc_enabled: bool,
    /// °C below the CPU's maximum temperature at which it starts to throttle.
    pub tcc_offset: u8,
}

/// Lowest power limit Control Center allows (RefreshCpuPLMinimumValue).
pub const MIN_LIMIT: u8 = 10;

/// The highest values the BIOS allows (its Smart APC table, SMAPCTABLE_STRUCT).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ranges {
    pub pl1: u8,
    pub pl2: u8,
    pub pl4: u8,
    pub tcc: u8,
}

/// Reads the Smart APC table through the ACPI driver Control Center installs (UWACPIDriver, `\\.\ACPIDriver`), as
/// SmartApcTableCtrl.GetTableByDriver does: one byte per key.
pub fn ranges() -> Result<Ranges> {
    const IOCTL_READ_TABLE: u32 = 0x9C40_A500;
    const READ_TABLE: u8 = 0xBB;
    let handle = unsafe {
        CreateFileW(w!("\\\\.\\ACPIDriver"), (GENERIC_READ | GENERIC_WRITE).0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None)
    }
    .context("Control Center's ACPI driver isn't there")?;
    let mut table = [0u8; 4];
    let mut result = Ok(());
    for (key, value) in table.iter_mut().enumerate() {
        let input = [READ_TABLE, key as u8];
        let mut output = [0xFFu8; 4];
        let sent = unsafe { DeviceIoControl(handle, IOCTL_READ_TABLE, Some(input.as_ptr() as _), 2, Some(output.as_mut_ptr() as _), 4, None, None) };
        if let Err(e) = sent {
            result = Err(e);
            break;
        }
        *value = output[0];
    }
    unsafe {
        let _ = CloseHandle(handle);
    }
    result.context("Couldn't read the BIOS's power limit table")?;
    // 0xFF: not set (CheckInvalidSmartApcTable).
    let [pl1, pl2, pl4, tcc] = table.map(|v| if v == 0xFF { 0 } else { v });
    if pl1 < MIN_LIMIT || pl2 < MIN_LIMIT || pl4 < MIN_LIMIT {
        bail!("The BIOS's power limit table is empty");
    }
    Ok(Ranges { pl1, pl2, pl4, tcc })
}

/// Switches to `mode` the way Control Center's SetUserProfile does: the fan mode byte, the power limits and the TCC
/// offset, and the firmware's own fan switching speed. Returns the limits sent.
pub fn apply(ec: &Registers, mode: Mode, fan_boost: bool, on_ac: bool, tuning: Option<&Tuning>) -> Result<PowerLimits> {
    set_fan_mode(ec, mode, fan_boost)?;
    let limits = set_tuning(ec, mode, on_ac, tuning)?;
    ec.write(FAN_SWITCH_SPEED, 0)?;
    Ok(limits)
}

/// The power limits on AC, the mode's defaults unless `tuning` has custom ones (0 on battery, where the EC picks its
/// own), and the TCC offset. `tuning` has been checked against the ranges. Returns the limits sent.
pub fn set_tuning(ec: &Registers, mode: Mode, on_ac: bool, tuning: Option<&Tuning>) -> Result<PowerLimits> {
    let limits = match tuning {
        _ if !on_ac => PowerLimits::default(),
        Some(t) if t.custom_limits => t.limits,
        _ => defaults(ec, mode)?.limits,
    };
    // PL1 can't be above PL2 (CheckInvalidParameter).
    ec.write(PL1, limits.pl1)?;
    ec.write(PL2, limits.pl2.max(limits.pl1))?;
    ec.write(PL4, limits.pl4)?;
    ec.write(TCC_OFFSET, match tuning {
        Some(t) if t.tcc_enabled => 0x80 | t.tcc_offset.min(0x7F),
        _ => 0,
    })?;
    Ok(limits)
}

/// Sets the fan mode byte alone (MyFanManager_RamFan1p5.SetFanMode): the mode's fans, and Fan Boost, which runs them
/// at full speed.
pub fn set_fan_mode(ec: &Registers, mode: Mode, fan_boost: bool) -> Result<()> {
    ec.write(FAN_MODE, mode.fan_byte() | if fan_boost { FAN_BOOST_BIT } else { 0 })
}

/// The limits in force now.
pub fn limits(ec: &Registers) -> Result<PowerLimits> {
    Ok(PowerLimits { pl1: ec.read(PL1)?, pl2: ec.read(PL2)?, pl4: ec.read(PL4)? })
}
