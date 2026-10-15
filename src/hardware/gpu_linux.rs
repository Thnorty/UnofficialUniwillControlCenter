//! The NVIDIA GPU's power state on Linux, from sysfs: reading it doesn't wake the GPU, as asking the driver would.

use std::path::Path;

/// Whether the NVIDIA GPU is switched on now; None if there's no NVIDIA GPU (or it's disabled).
pub fn nvidia_on() -> Option<bool> {
    nvidia_state(Path::new("/sys"))
}

/// The NVIDIA display controller's PCI power state under `root` (/sys): D0 is on, D3hot/D3cold off. Kernels without
/// `power_state` give the runtime PM status instead. None without such a device.
pub fn nvidia_state(root: &Path) -> Option<bool> {
    let read = |path: &Path| std::fs::read_to_string(path).ok().map(|s| s.trim().to_string());
    let mut devices: Vec<_> = std::fs::read_dir(root.join("bus/pci/devices")).ok()?.flatten().map(|e| e.path()).collect();
    devices.sort();
    let gpu = devices.into_iter().find(|device| {
        read(&device.join("vendor")).as_deref() == Some("0x10de") && read(&device.join("class")).is_some_and(|c| c.starts_with("0x03"))
    })?;
    if let Some(state) = read(&gpu.join("power_state")) {
        return Some(state == "D0");
    }
    match read(&gpu.join("power/runtime_status"))?.as_str() {
        "active" => Some(true),
        "suspended" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tree(name: &str, devices: &[(&str, &str, &str, Option<&str>)]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("uucc-gpu-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (address, vendor, class, power) in devices {
            let device = root.join("bus/pci/devices").join(address);
            fs::create_dir_all(&device).unwrap();
            fs::write(device.join("vendor"), format!("{vendor}\n")).unwrap();
            fs::write(device.join("class"), format!("{class}\n")).unwrap();
            if let Some(power) = power {
                fs::write(device.join("power_state"), format!("{power}\n")).unwrap();
            }
        }
        root
    }

    const INTEL_GPU: (&str, &str, &str, Option<&str>) = ("0000:00:02.0", "0x8086", "0x030000", Some("D0"));
    const NVIDIA_AUDIO: (&str, &str, &str, Option<&str>) = ("0000:01:00.1", "0x10de", "0x040300", Some("D0"));

    #[test]
    fn no_nvidia_device_reads_as_absent() {
        // Review Focus 5: an Intel-only laptop, or iGPU mode. None, as on Windows: SystemStatus then neither shows the
        // GPU as off nor loads NVML.
        let root = tree("none", &[INTEL_GPU, NVIDIA_AUDIO]);
        assert_eq!(nvidia_state(&root), None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn nvidia_d3cold_is_off() {
        let root = tree("d3", &[INTEL_GPU, ("0000:01:00.0", "0x10de", "0x030000", Some("D3cold"))]);
        assert_eq!(nvidia_state(&root), Some(false));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn nvidia_d0_is_on() {
        let root = tree("d0", &[INTEL_GPU, NVIDIA_AUDIO, ("0000:01:00.0", "0x10de", "0x030200", Some("D0"))]);
        assert_eq!(nvidia_state(&root), Some(true));
        let _ = fs::remove_dir_all(root);
    }
}
