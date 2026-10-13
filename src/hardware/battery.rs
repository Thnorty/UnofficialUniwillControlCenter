//! Battery care, as Control Center's BatteryProtection2 sets it: two bits in the EC choose how full the battery is
//! kept. The EC forgets them on resume, so they're set again then.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::ec::Registers;

const CHARGE_BYTE: u16 = 0x7A6; // bit 4: Balanced, bit 5: Stationary
const CHARGE_MASK: u8 = 0x30;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ChargeMode {
    /// Charges fully (PERFORMANCEDMODE).
    HighCapacity = 0,
    /// BALANCEDMODE
    Balanced = 1,
    /// Keeps the charge around 40-60% (HEALTHYMODE), for a laptop that's always plugged in.
    Stationary = 2,
}

impl ChargeMode {
    pub fn from_index(index: usize) -> ChargeMode {
        match index {
            1 => ChargeMode::Balanced,
            2 => ChargeMode::Stationary,
            _ => ChargeMode::HighCapacity,
        }
    }

    fn bits(self) -> u8 {
        match self {
            ChargeMode::HighCapacity => 0,
            ChargeMode::Balanced => 0x10,
            ChargeMode::Stationary => 0x20,
        }
    }
}

pub fn current(ec: &Registers) -> Result<ChargeMode> {
    Ok(match ec.read(CHARGE_BYTE)? & CHARGE_MASK {
        0x10 => ChargeMode::Balanced,
        0x20 => ChargeMode::Stationary,
        _ => ChargeMode::HighCapacity,
    })
}

pub fn set(ec: &Registers, mode: ChargeMode) -> Result<()> {
    ec.update(CHARGE_BYTE, CHARGE_MASK, mode.bits())
}

/// The mode Control Center last set, if it was installed (HKLM\SOFTWARE\OEM\GamingCenter2\BatteryProtection2).
pub fn control_center_choice() -> Option<ChargeMode> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\OEM\\GamingCenter2\\BatteryProtection2"),
            w!("HealthProtectionStatus"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as _),
            Some(&mut size),
        )
    };
    result.is_ok().then(|| ChargeMode::from_index(data as usize))
}
