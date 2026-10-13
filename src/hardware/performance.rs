//! Performance modes and Fan Boost, as Control Center's MyFanManager_RamFan1p5 sets them: the EC's fan mode byte,
//! and the CPU power limits the EC holds as each mode's defaults. CPU voltage offsets and GPU overclocking, which
//! Control Center can also change, are left alone.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::ec::Registers;

const FAN_MODE: u16 = 0x751; // bit 7 Office, bit 4 Turbo, neither Gaming; bit 6 Fan Boost
const FAN_BOOST_BIT: u8 = 0x40;
const PL1: u16 = 0x783;
const PL2: u16 = 0x784;
const PL4: u16 = 0x785;
const TCC_OFFSET: u16 = 0x786; // ADDR_TimAP_TccOffset_Setting: 0 = the firmware's
const FAN_SWITCH_SPEED: u16 = 0x787; // ADDR_TimAP_FanSwitchSpeedT100mSec: 0 = the firmware's
const DEFAULTS_GAMING: u16 = 0x730; // PL1, PL2, PL4, GPU D-state
const DEFAULTS_OFFICE: u16 = 0x734;
const DEFAULTS_TURBO: u16 = 0x7A7;
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

/// CPU power limits in watts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerLimits {
    pub pl1: u8,
    pub pl2: u8,
    pub pl4: u8,
}

/// The limits the EC keeps for `mode` (GetGamingPLDefaultValue and the like).
pub fn default_limits(ec: &Registers, mode: Mode) -> Result<PowerLimits> {
    let base = mode.defaults_address();
    Ok(PowerLimits { pl1: ec.read(base)?, pl2: ec.read(base + 1)?, pl4: ec.read(base + 2)? })
}

/// Switches to `mode` the way Control Center's SetUserProfile does with an untouched profile: the fan mode byte, the
/// mode's default power limits on AC (0 on battery, where the EC picks its own), and the firmware's own TCC offset
/// and fan switching speed. Returns the limits sent.
pub fn apply(ec: &Registers, mode: Mode, fan_boost: bool, on_ac: bool) -> Result<PowerLimits> {
    set_fan_mode(ec, mode, fan_boost)?;
    let limits = if on_ac { default_limits(ec, mode)? } else { PowerLimits::default() };
    // PL1 can't be above PL2 (CheckInvalidParameter).
    ec.write(PL1, limits.pl1)?;
    ec.write(PL2, limits.pl2.max(limits.pl1))?;
    ec.write(PL4, limits.pl4)?;
    ec.write(TCC_OFFSET, 0)?;
    ec.write(FAN_SWITCH_SPEED, 0)?;
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
