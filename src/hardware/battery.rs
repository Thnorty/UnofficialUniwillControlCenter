//! Battery care, as Control Center's BatteryProtection2 sets it: two bits in the EC choose how full the battery is
//! kept. The EC forgets them on resume, so they're set again then. Also USB charging while the laptop is off, as its
//! MySettingManager sets it.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::ec::Registers;

const CHARGE_BYTE: u16 = 0x7A6; // bit 4: Balanced, bit 5: Stationary
const CHARGE_MASK: u8 = 0x30;
const SUPPORT_BYTE2: u16 = 0x766; // bit 1: USB charging while off is supported (SupportByteTwoFlag.USBChargerMode)
const USB_SUPPORT_BIT: u8 = 1;
const TRIGGER_BYTE: u16 = 0x767; // bit 4: USB charging while off (USB_Charger_ON / USB_Charger_OFF)
const USB_POWER_BIT: u8 = 4;

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

/// Whether USB ports keep charging devices while the laptop is off, or None if the laptop can't. Control Center
/// offers the switch where the EC reports it; its own setting counts too, and so does `known` (this app saw the
/// switch before), which still holds once Control Center and its setting are gone.
pub fn usb_power(ec: &Registers, known: bool) -> Result<Option<bool>> {
    if !known && !ec.bit(SUPPORT_BYTE2, USB_SUPPORT_BIT)? && control_center_usb_power().is_none() {
        return Ok(None);
    }
    Ok(Some(ec.bit(TRIGGER_BYTE, USB_POWER_BIT)?))
}

/// Changes USB charging while off; the byte holds other switches, so only when it differs.
pub fn set_usb_power(ec: &Registers, on: bool) -> Result<()> {
    if ec.bit(TRIGGER_BYTE, USB_POWER_BIT)? != on {
        let mask = 1 << USB_POWER_BIT;
        ec.update(TRIGGER_BYTE, mask, if on { mask } else { 0 })?;
    }
    Ok(())
}

/// The mode Control Center last set, if it was installed (HKLM\SOFTWARE\OEM\GamingCenter2\BatteryProtection2).
pub fn control_center_choice() -> Option<ChargeMode> {
    control_center_value(windows::core::w!("SOFTWARE\\OEM\\GamingCenter2\\BatteryProtection2"), windows::core::w!("HealthProtectionStatus"))
        .map(|value| ChargeMode::from_index(value as usize))
}

/// USB charging while off as Control Center last set it (HKLM\SOFTWARE\OEM\GamingCenter2\MySetting).
pub fn control_center_usb_power() -> Option<bool> {
    control_center_value(windows::core::w!("SOFTWARE\\OEM\\GamingCenter2\\MySetting"), windows::core::w!("USBCharger")).map(|value| value == 1)
}

fn control_center_value(key: windows::core::PCWSTR, name: windows::core::PCWSTR) -> Option<u32> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    let result = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key, name, RRF_RT_REG_DWORD, None, Some(&mut data as *mut u32 as _), Some(&mut size)) };
    result.is_ok().then_some(data)
}
