//! Battery, CPU load and temperatures for the status effects, read about once a second on the effects thread. Each
//! reading is None when this machine can't provide it right now.

use std::collections::HashMap;

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::Threading::GetSystemTimes;
use windows::core::{s, w};
use wmi::{Variant, WMIConnection};

use crate::lighting::engine::Inputs;

#[derive(Default)]
pub struct SystemStatus {
    /// Battery charge, 0-100.
    pub battery_percent: Option<u8>,
    pub charging: bool,
    pub on_ac: bool,
    /// CPU use since the previous reading, 0-1.
    pub cpu_load: Option<f32>,
    /// The hottest ACPI thermal zone in °C. Control Center reads the same Windows counter ("Thermal Zone Information").
    pub cpu_temperature: Option<f32>,
    /// The NVIDIA GPU's temperature in °C; None while the GPU is switched off or absent.
    pub gpu_temperature: Option<f32>,
    /// The NVIDIA GPU is switched off to save power, so it isn't asked for its temperature: that would switch it on.
    pub gpu_off: bool,

    last_times: Option<(u64, u64, u64)>,
    wmi: Option<WMIConnection>,
    nvml: Option<Nvml>,
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
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        if unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.is_err() {
            return;
        }
        let value = |t: FILETIME| (t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64;
        let now = (value(idle), value(kernel), value(user));
        if let Some((last_idle, last_kernel, last_user)) = self.last_times {
            // Kernel time includes idle time.
            let total = now.1.wrapping_sub(last_kernel) + now.2.wrapping_sub(last_user);
            if total > 0 {
                let idle_delta = now.0.wrapping_sub(last_idle);
                self.cpu_load = Some((1.0 - idle_delta as f32 / total as f32).clamp(0.0, 1.0));
            }
        }
        self.last_times = Some(now);
    }

    fn read_cpu_temperature(&mut self) {
        self.cpu_temperature = None;
        if self.wmi.is_none() {
            self.wmi = WMIConnection::new().ok();
        }
        let Some(wmi) = &self.wmi else { return };
        let rows: Result<Vec<HashMap<String, Variant>>, _> =
            wmi.raw_query("SELECT HighPrecisionTemperature FROM Win32_PerfFormattedData_Counters_ThermalZoneInformation");
        let Ok(rows) = rows else {
            self.wmi = None; // try a new connection next time
            return;
        };
        for row in rows {
            let tenths_kelvin = match row.get("HighPrecisionTemperature") {
                Some(Variant::UI4(v)) => *v as f32,
                Some(Variant::I4(v)) => *v as f32,
                Some(Variant::UI8(v)) => *v as f32,
                Some(Variant::String(s)) => s.parse().unwrap_or(0.0),
                _ => continue,
            };
            let celsius = tenths_kelvin / 10.0 - 273.15;
            if celsius > 0.0 && celsius < 150.0 && self.cpu_temperature.is_none_or(|hottest| celsius > hottest) {
                self.cpu_temperature = Some(celsius);
            }
        }
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
            match Nvml::load() {
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

/// The battery's charge (None without one), whether it's charging, and whether the power adapter is in.
pub fn power_now() -> (Option<u8>, bool, bool) {
    let mut power = SYSTEM_POWER_STATUS::default();
    let ok = unsafe { GetSystemPowerStatus(&mut power) }.is_ok();
    // BatteryFlag 128: no system battery; 255: unknown. BatteryLifePercent 255: unknown.
    let has_battery = ok && power.BatteryFlag != 128 && power.BatteryFlag != 255 && power.BatteryLifePercent != 255;
    let on_ac = ok && power.ACLineStatus == 1;
    (has_battery.then(|| power.BatteryLifePercent.min(100)), has_battery && on_ac && power.BatteryLifePercent < 100, on_ac)
}

/// Whether the laptop runs on its power adapter.
pub fn on_ac() -> bool {
    let mut power = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut power) }.is_ok() && power.ACLineStatus == 1
}

/// Physical memory in use and in total, in bytes.
pub fn memory() -> Option<(u64, u64)> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some((status.ullTotalPhys - status.ullAvailPhys, status.ullTotalPhys))
}

/// What the battery's driver reports (ROOT\WMI): capacities in mWh, the charge (+) or discharge (-) rate in mW.
#[derive(Clone, Copy, Debug, Default)]
pub struct BatteryDetails {
    pub designed: Option<u32>,
    pub full: Option<u32>,
    pub remaining: Option<u32>,
    pub rate: Option<i32>,
    pub voltage: Option<u32>,
    pub cycles: Option<u32>,
}

pub fn battery_details(wmi: &WMIConnection) -> BatteryDetails {
    let first = |query: &str, field: &str| -> Option<u32> {
        let rows: Vec<HashMap<String, Variant>> = wmi.raw_query(query).ok()?;
        rows.iter().find_map(|row| match row.get(field)? {
            Variant::UI4(v) => Some(*v),
            Variant::I4(v) => Some(*v as u32),
            Variant::UI2(v) => Some(*v as u32),
            Variant::String(s) => s.parse().ok(),
            _ => None,
        })
    };
    let charging = first("SELECT ChargeRate FROM BatteryStatus", "ChargeRate");
    let discharging = first("SELECT DischargeRate FROM BatteryStatus", "DischargeRate");
    let rate = match (charging, discharging) {
        (Some(c), _) if c > 0 => Some(c as i32),
        (_, Some(d)) if d > 0 => Some(-(d as i32)),
        (None, None) => None,
        _ => Some(0),
    };
    BatteryDetails {
        designed: first("SELECT DesignedCapacity FROM BatteryStaticData", "DesignedCapacity").filter(|&c| c > 0),
        full: first("SELECT FullChargedCapacity FROM BatteryFullChargedCapacity", "FullChargedCapacity").filter(|&c| c > 0),
        remaining: first("SELECT RemainingCapacity FROM BatteryStatus", "RemainingCapacity"),
        rate,
        voltage: first("SELECT Voltage FROM BatteryStatus", "Voltage").filter(|&v| v > 0),
        cycles: first("SELECT CycleCount FROM BatteryCycleCount", "CycleCount").filter(|&c| c > 0),
    }
}

impl Drop for SystemStatus {
    fn drop(&mut self) {
        if let Some(nvml) = &self.nvml
            && nvml.started
        {
            unsafe { (nvml.shutdown)() };
        }
    }
}

/// The few NVML functions used, loaded from the NVIDIA driver at run time.
struct Nvml {
    init: unsafe extern "C" fn() -> i32,
    shutdown: unsafe extern "C" fn() -> i32,
    handle_by_index: unsafe extern "C" fn(u32, *mut *mut std::ffi::c_void) -> i32,
    temperature: unsafe extern "C" fn(*mut std::ffi::c_void, i32, *mut u32) -> i32,
    started: bool,
}

impl Nvml {
    fn load() -> Option<Nvml> {
        unsafe {
            let module = LoadLibraryW(w!("nvml.dll"))
                .or_else(|_| LoadLibraryW(w!("C:\\Program Files\\NVIDIA Corporation\\NVSMI\\nvml.dll")))
                .ok()?;
            let init = GetProcAddress(module, s!("nvmlInit_v2"))?;
            let shutdown = GetProcAddress(module, s!("nvmlShutdown"))?;
            let handle_by_index = GetProcAddress(module, s!("nvmlDeviceGetHandleByIndex_v2"))?;
            let temperature = GetProcAddress(module, s!("nvmlDeviceGetTemperature"))?;
            Some(Nvml {
                init: std::mem::transmute::<_, unsafe extern "C" fn() -> i32>(init),
                shutdown: std::mem::transmute::<_, unsafe extern "C" fn() -> i32>(shutdown),
                handle_by_index: std::mem::transmute::<_, unsafe extern "C" fn(u32, *mut *mut std::ffi::c_void) -> i32>(handle_by_index),
                temperature: std::mem::transmute::<_, unsafe extern "C" fn(*mut std::ffi::c_void, i32, *mut u32) -> i32>(temperature),
                started: false,
            })
        }
    }

    fn temperature(&mut self) -> Option<f32> {
        unsafe {
            if !self.started {
                if (self.init)() != 0 {
                    return None; // no NVIDIA GPU in use right now; try again next time
                }
                self.started = true;
            }
            let mut device = std::ptr::null_mut();
            let mut celsius = 0u32;
            if (self.handle_by_index)(0, &mut device) == 0 && (self.temperature)(device, 0 /* NVML_TEMPERATURE_GPU */, &mut celsius) == 0 {
                Some(celsius as f32)
            } else {
                None
            }
        }
    }
}
