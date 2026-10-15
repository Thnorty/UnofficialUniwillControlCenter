//! The NVIDIA GPU's temperature from the driver's NVML (libnvidia-ml.so.1), loaded when first needed; without the
//! proprietary driver there's no NVML and no reading.

use std::ffi::c_void;

use libloading::Library;

pub struct Nvml {
    init: unsafe extern "C" fn() -> i32,
    shutdown: unsafe extern "C" fn() -> i32,
    handle_by_index: unsafe extern "C" fn(u32, *mut *mut c_void) -> i32,
    temperature: unsafe extern "C" fn(*mut c_void, i32, *mut u32) -> i32,
    started: bool,
    // Last, so the functions above are never used after it's unloaded.
    _library: Library,
}

impl Nvml {
    pub fn load() -> Option<Nvml> {
        unsafe {
            let library = Library::new("libnvidia-ml.so.1").ok()?;
            let init = *library.get::<unsafe extern "C" fn() -> i32>(b"nvmlInit_v2\0").ok()?;
            let shutdown = *library.get::<unsafe extern "C" fn() -> i32>(b"nvmlShutdown\0").ok()?;
            let handle_by_index = *library.get::<unsafe extern "C" fn(u32, *mut *mut c_void) -> i32>(b"nvmlDeviceGetHandleByIndex_v2\0").ok()?;
            let temperature = *library.get::<unsafe extern "C" fn(*mut c_void, i32, *mut u32) -> i32>(b"nvmlDeviceGetTemperature\0").ok()?;
            Some(Nvml { init, shutdown, handle_by_index, temperature, started: false, _library: library })
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
