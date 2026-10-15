//! The HID transport to the lighting controllers on Windows: finding a controller by its IDs and usage, and sending
//! feature and output reports to it.

use anyhow::Result;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList,
    SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HIDD_ATTRIBUTES, HIDP_CAPS, HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetFeature, HidD_GetHidGuid, HidD_GetPreparsedData,
    HidD_SetFeature, HidP_GetCaps, PHIDP_PREPARSED_DATA,
};
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, WriteFile};
use windows::core::PCWSTR;

struct OwnedHandle(HANDLE);

// The handle is only used under the device's lock.
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// An open HID device.
pub struct HidDevice {
    handle: OwnedHandle,
    feature_length: usize,
    output_length: usize,
}

impl HidDevice {
    /// The first present device with this vendor, one of these products, and this top-level usage; None if there's
    /// none.
    pub fn find(vendor: u16, products: &[u16], usage_page: u16, usage: u16) -> Result<Option<HidDevice>> {
        for path in hid_device_paths() {
            let Some(handle) = open_path(&path) else { continue };
            let mut attributes = HIDD_ATTRIBUTES { Size: size_of::<HIDD_ATTRIBUTES>() as u32, ..Default::default() };
            if !unsafe { HidD_GetAttributes(handle.0, &mut attributes) } || attributes.VendorID != vendor || !products.contains(&attributes.ProductID) {
                continue;
            }
            let Some(caps) = caps(&handle) else { continue };
            if caps.UsagePage != usage_page || caps.Usage != usage {
                continue;
            }
            return Ok(Some(HidDevice {
                handle,
                feature_length: caps.FeatureReportByteLength as usize,
                output_length: caps.OutputReportByteLength as usize,
            }));
        }
        Ok(None)
    }

    /// A feature report's length, with its report ID byte.
    pub fn feature_length(&self) -> usize {
        self.feature_length
    }

    /// An output report's length, with its report ID byte.
    pub fn output_length(&self) -> usize {
        self.output_length
    }

    pub fn set_feature(&self, report: &[u8]) -> Result<()> {
        if !unsafe { HidD_SetFeature(self.handle.0, report.as_ptr() as _, report.len() as u32) } {
            return Err(windows::core::Error::from_thread().into());
        }
        Ok(())
    }

    pub fn get_feature(&self, report: &mut [u8]) -> Result<()> {
        if !unsafe { HidD_GetFeature(self.handle.0, report.as_mut_ptr() as _, report.len() as u32) } {
            return Err(windows::core::Error::from_thread().into());
        }
        Ok(())
    }

    pub fn write_output(&self, report: &[u8]) -> Result<()> {
        unsafe { WriteFile(self.handle.0, Some(report), None, None) }?;
        Ok(())
    }
}

fn hid_device_paths() -> Vec<Vec<u16>> {
    let mut paths = Vec::new();
    unsafe {
        let guid = HidD_GetHidGuid();
        let Ok(set) = SetupDiGetClassDevsW(Some(&guid), PCWSTR::null(), None, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) else {
            return paths;
        };
        let mut data = SP_DEVICE_INTERFACE_DATA { cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default() };
        let mut index = 0;
        while SetupDiEnumDeviceInterfaces(set, None, &guid, index, &mut data).is_ok() {
            index += 1;
            let mut size = 0u32;
            let _ = SetupDiGetDeviceInterfaceDetailW(set, &data, None, 0, Some(&mut size), None);
            if size == 0 {
                continue;
            }
            // u32 storage keeps the detail structure aligned.
            let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
            let detail = buffer.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(set, &data, Some(detail), size, None, None).is_ok() {
                let start = std::ptr::addr_of!((*detail).DevicePath) as *const u16;
                let mut length = 0;
                while *start.add(length) != 0 {
                    length += 1;
                }
                let mut path = std::slice::from_raw_parts(start, length).to_vec();
                path.push(0);
                paths.push(path);
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
    }
    paths
}

fn open_path(path: &[u16]) -> Option<OwnedHandle> {
    unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .ok()
        .map(OwnedHandle)
    }
}

fn caps(handle: &OwnedHandle) -> Option<HIDP_CAPS> {
    unsafe {
        let mut data = PHIDP_PREPARSED_DATA::default();
        if !HidD_GetPreparsedData(handle.0, &mut data) {
            return None;
        }
        let mut caps = HIDP_CAPS::default();
        let ok = HidP_GetCaps(data, &mut caps).is_ok();
        let _ = HidD_FreePreparsedData(data);
        ok.then_some(caps)
    }
}
