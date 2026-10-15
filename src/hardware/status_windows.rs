//! The Windows side of the readings: the ACPI thermal zones and the battery through WMI, the CPU's times, the power
//! status, memory, and NVML from the NVIDIA driver.

use std::collections::HashMap;

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::Threading::GetSystemTimes;
use windows::core::{s, w};
use wmi::{Variant, WMIConnection};

use super::BatteryDetails;

/// What the readers keep between readings.
#[derive(Default)]
pub struct State {
    wmi: Option<WMIConnection>,
}

/// The CPU's idle time and its total time so far, in any unit.
pub fn cpu_times() -> Option<(u64, u64)> {
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    if unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.is_err() {
        return None;
    }
    let value = |t: FILETIME| (t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64;
    // Kernel time includes idle time.
    Some((value(idle), value(kernel).wrapping_add(value(user))))
}

/// The hottest ACPI thermal zone in °C. Control Center reads the same Windows counter ("Thermal Zone Information").
pub fn cpu_temperature(state: &mut State) -> Option<f32> {
    if state.wmi.is_none() {
        state.wmi = WMIConnection::new().ok();
    }
    let wmi = state.wmi.as_ref()?;
    let mut hottest: Option<f32> = None;
        let rows: Result<Vec<HashMap<String, Variant>>, _> =
            wmi.raw_query("SELECT HighPrecisionTemperature FROM Win32_PerfFormattedData_Counters_ThermalZoneInformation");
        let Ok(rows) = rows else {
            state.wmi = None; // try a new connection next time
            return None;
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
            if celsius > 0.0 && celsius < 150.0 && hottest.is_none_or(|h| celsius > h) {
                hottest = Some(celsius);
            }
        }
    hottest
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

/// Reads the battery's details, keeping its WMI connection between readings.
#[derive(Default)]
pub struct BatteryReader {
    wmi: Option<WMIConnection>,
}

impl BatteryReader {
    pub fn read(&mut self) -> BatteryDetails {
        if self.wmi.is_none() {
            self.wmi = WMIConnection::with_namespace_path("ROOT\\WMI").ok();
        }
        self.wmi.as_ref().map(battery_details).unwrap_or_default()
    }
}

fn battery_details(wmi: &WMIConnection) -> BatteryDetails {
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

/// The few NVML functions used, loaded from the NVIDIA driver at run time.
pub struct Nvml {
    init: unsafe extern "C" fn() -> i32,
    shutdown: unsafe extern "C" fn() -> i32,
    handle_by_index: unsafe extern "C" fn(u32, *mut *mut std::ffi::c_void) -> i32,
    temperature: unsafe extern "C" fn(*mut std::ffi::c_void, i32, *mut u32) -> i32,
    started: bool,
}

impl Nvml {
    pub fn load() -> Option<Nvml> {
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

    pub fn temperature(&mut self) -> Option<f32> {
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

impl Drop for Nvml {
    fn drop(&mut self) {
        if self.started {
            unsafe { (self.shutdown)() };
        }
    }
}
