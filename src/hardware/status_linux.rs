//! The Linux side of the readings. Not read yet: every reading is None.

use super::BatteryDetails;

/// What the readers keep between readings.
#[derive(Default)]
pub struct State;

/// The CPU's idle time and its total time so far, in any unit.
pub fn cpu_times() -> Option<(u64, u64)> {
    None
}

/// The hottest ACPI thermal zone in °C.
pub fn cpu_temperature(_state: &mut State) -> Option<f32> {
    None
}

/// The battery's charge (None without one), whether it's charging, and whether the power adapter is in.
pub fn power_now() -> (Option<u8>, bool, bool) {
    (None, false, false)
}

/// Whether the laptop runs on its power adapter.
pub fn on_ac() -> bool {
    false
}

/// Physical memory in use and in total, in bytes.
pub fn memory() -> Option<(u64, u64)> {
    None
}

/// Reads the battery's details.
#[derive(Default)]
pub struct BatteryReader;

impl BatteryReader {
    pub fn read(&mut self) -> BatteryDetails {
        BatteryDetails::default()
    }
}

/// The NVIDIA driver's NVML.
pub struct Nvml;

impl Nvml {
    pub fn load() -> Option<Nvml> {
        None
    }

    pub fn temperature(&mut self) -> Option<f32> {
        None
    }
}
