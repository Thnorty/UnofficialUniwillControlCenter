//! Battery, CPU load and temperatures for the status effects, read about once a second on the effects thread. Each
//! reading is None when this machine can't provide it right now.

#[cfg(target_os = "linux")]
#[path = "status_linux.rs"]
mod platform;
#[cfg(windows)]
#[path = "status_windows.rs"]
mod platform;

pub use platform::{BatteryReader, memory, on_ac, power_now};

use crate::lighting::engine::Inputs;

#[derive(Default)]
pub struct SystemStatus {
    /// Battery charge, 0-100.
    pub battery_percent: Option<u8>,
    pub charging: bool,
    pub on_ac: bool,
    /// CPU use since the previous reading, 0-1.
    pub cpu_load: Option<f32>,
    /// The hottest ACPI thermal zone in °C, the reading Control Center uses.
    pub cpu_temperature: Option<f32>,
    /// The NVIDIA GPU's temperature in °C; None while the GPU is switched off or absent.
    pub gpu_temperature: Option<f32>,
    /// The NVIDIA GPU is switched off to save power, so it isn't asked for its temperature: that would switch it on.
    pub gpu_off: bool,

    last_times: Option<(u64, u64)>,
    platform: platform::State,
    nvml: Option<platform::Nvml>,
    nvml_missing: bool,
}

impl SystemStatus {
    pub fn update(&mut self, needed: Inputs) {
        if needed.contains(Inputs::BATTERY) {
            self.read_battery();
        }
        if needed.contains(Inputs::CPU_LOAD) {
            self.read_cpu_load();
        }
        if needed.contains(Inputs::CPU_TEMPERATURE) {
            self.read_cpu_temperature();
        }
        if needed.contains(Inputs::GPU_TEMPERATURE) {
            self.read_gpu_temperature();
        }
    }

    fn read_battery(&mut self) {
        (self.battery_percent, self.charging, self.on_ac) = power_now();
    }

    fn read_cpu_load(&mut self) {
        let Some(now) = platform::cpu_times() else { return };
        if let Some(last) = self.last_times {
            self.cpu_load = cpu_load(last, now).or(self.cpu_load);
        }
        self.last_times = Some(now);
    }

    fn read_cpu_temperature(&mut self) {
        self.cpu_temperature = platform::cpu_temperature(&mut self.platform);
    }

    fn read_gpu_temperature(&mut self) {
        self.gpu_temperature = None;
        self.gpu_off = false;
        if self.nvml_missing {
            return;
        }
        // While it's off, NVML would switch it on, and until then report the last temperature it saw.
        match super::gpu::nvidia_on() {
            Some(true) => {}
            Some(false) => {
                self.gpu_off = true;
                return;
            }
            None => return,
        }
        if self.nvml.is_none() {
            match platform::Nvml::load() {
                Some(nvml) => self.nvml = Some(nvml),
                None => {
                    self.nvml_missing = true; // no NVIDIA driver
                    return;
                }
            }
        }
        if let Some(nvml) = &mut self.nvml {
            self.gpu_temperature = nvml.temperature();
        }
    }
}

/// The CPU's use between two readings of (idle, total) time; None when no time passed.
fn cpu_load((last_idle, last_total): (u64, u64), (idle, total): (u64, u64)) -> Option<f32> {
    let total = total.wrapping_sub(last_total);
    (total > 0).then(|| (1.0 - idle.wrapping_sub(last_idle) as f32 / total as f32).clamp(0.0, 1.0))
}

/// What the battery's driver reports: capacities in mWh, the charge (+) or discharge (-) rate in mW.
#[derive(Clone, Copy, Debug, Default)]
pub struct BatteryDetails {
    pub designed: Option<u32>,
    pub full: Option<u32>,
    pub remaining: Option<u32>,
    pub rate: Option<i32>,
    pub voltage: Option<u32>,
    pub cycles: Option<u32>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn cpu_load_is_the_busy_share() {
        assert_eq!(super::cpu_load((100, 1000), (150, 1200)), Some(0.75));
        assert_eq!(super::cpu_load((100, 1000), (100, 1000)), None);
    }
}
