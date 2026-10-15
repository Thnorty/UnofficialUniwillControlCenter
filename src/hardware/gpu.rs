//! The NVIDIA GPU's power state, from what Windows knows about the device, which doesn't wake it. With Optimus the
//! GPU is switched off (D3) while nothing uses it, and the integrated graphics drive the screen. Any call into the
//! NVIDIA driver (NVML) switches it on, at about 25 W on this laptop, so readings from it are only taken while it's on.

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_GETIDLIST_FILTER_CLASS, CM_GETIDLIST_FILTER_PRESENT, CM_Get_DevNode_PropertyW, CM_Get_Device_ID_ListW, CM_Get_Device_ID_List_SizeW,
    CM_LOCATE_DEVNODE_NORMAL, CM_Locate_DevNodeW, CR_SUCCESS,
};
use windows::Win32::Devices::Properties::{DEVPKEY_Device_PowerData, DEVPROPTYPE};
use windows::core::{PCWSTR, w};

/// The display adapters' device class.
const DISPLAY_CLASS: PCWSTR = w!("{4d36e968-e325-11ce-bfc1-08002be10318}");

/// Whether the NVIDIA GPU is switched on now; None if there's no NVIDIA GPU (or it's disabled).
pub fn nvidia_on() -> Option<bool> {
    let id = nvidia_device()?;
    let id: Vec<u16> = id.encode_utf16().chain([0]).collect();
    unsafe {
        let mut node = 0;
        if CM_Locate_DevNodeW(&mut node, PCWSTR(id.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) != CR_SUCCESS {
            return None;
        }
        // CM_POWER_DATA: its size, then the most recent device power state (1 = D0, on).
        let mut data = [0u8; 64];
        let mut size = data.len() as u32;
        let mut kind = DEVPROPTYPE::default();
        if CM_Get_DevNode_PropertyW(node, &DEVPKEY_Device_PowerData, &mut kind, Some(data.as_mut_ptr()), &mut size, 0) != CR_SUCCESS || size < 8 {
            return None;
        }
        Some(u32::from_le_bytes([data[4], data[5], data[6], data[7]]) == 1)
    }
}

/// The NVIDIA display adapter's device instance ID.
fn nvidia_device() -> Option<String> {
    let flags = CM_GETIDLIST_FILTER_CLASS | CM_GETIDLIST_FILTER_PRESENT;
    unsafe {
        let mut length = 0;
        if CM_Get_Device_ID_List_SizeW(&mut length, DISPLAY_CLASS, flags) != CR_SUCCESS || length == 0 {
            return None;
        }
        let mut buffer = vec![0u16; length as usize];
        if CM_Get_Device_ID_ListW(DISPLAY_CLASS, &mut buffer, flags) != CR_SUCCESS {
            return None;
        }
        // NUL-separated, ending in two NULs.
        buffer
            .split(|&c| c == 0)
            .map(String::from_utf16_lossy)
            .find(|id| id.to_ascii_uppercase().starts_with("PCI\\VEN_10DE&"))
    }
}

#[cfg(test)]
mod tests {
    // Reads this laptop's GPU; prints rather than asserts, since the state changes.
    #[test]
    #[ignore]
    fn reads_the_power_state() {
        let started = std::time::Instant::now();
        let on = super::nvidia_on();
        println!("NVIDIA GPU on: {on:?} ({:?})", started.elapsed());
        assert!(on.is_some());
    }
}
