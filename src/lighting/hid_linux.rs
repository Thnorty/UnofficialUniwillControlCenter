//! The HID transport to the lighting controllers on Linux: hidraw. Devices are found through sysfs, by their USB ids
//! and their report descriptor's top-level usage; feature reports go through hidraw's ioctls, output reports through
//! `write`. The package's udev rule gives the person signed in access to the ITE controllers' hidraw nodes.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::Path;

use anyhow::{Result, anyhow};

/// What the app needs from a report descriptor: the top-level usage, and the report lengths with the ID byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DescriptorInfo {
    pub usage_page: u16,
    pub usage: u16,
    pub feature_length: usize,
    pub output_length: usize,
}

/// Reads a HID report descriptor's short items: the first Usage Page and Usage, and the bits in the Feature and
/// Output main items (Report Size × Report Count each), plus one byte for the report ID.
pub fn descriptor_info(descriptor: &[u8]) -> DescriptorInfo {
    let (mut usage_page, mut usage) = (None, None);
    let (mut size, mut count) = (0usize, 0usize);
    let (mut feature_bits, mut output_bits) = (0usize, 0usize);
    let mut at = 0;
    while at < descriptor.len() {
        let prefix = descriptor[at];
        if prefix == 0xFE {
            // A long item: its data size is the next byte, then a tag byte.
            at += 3 + descriptor.get(at + 1).copied().unwrap_or(0) as usize;
            continue;
        }
        let length = match prefix & 0x03 {
            3 => 4,
            n => n as usize,
        };
        let data = descriptor.get(at + 1..at + 1 + length).unwrap_or(&[]);
        let value = data.iter().rev().fold(0u32, |v, &b| (v << 8) | b as u32);
        match prefix & 0xFC {
            0x04 => {
                usage_page.get_or_insert(value as u16);
            }
            0x08 => {
                usage.get_or_insert(value as u16);
            }
            0x74 => size = value as usize,
            0x94 => count = value as usize,
            0x90 => output_bits += size * count,
            0xB0 => feature_bits += size * count,
            _ => {}
        }
        at += 1 + length;
    }
    DescriptorInfo {
        usage_page: usage_page.unwrap_or(0),
        usage: usage.unwrap_or(0),
        feature_length: 1 + feature_bits / 8,
        output_length: 1 + output_bits / 8,
    }
}

/// The error for a device node that can't be opened, with the fix when it's a permission problem.
fn open_error(path: &Path, error: io::Error) -> anyhow::Error {
    if error.kind() == io::ErrorKind::PermissionDenied {
        anyhow!(
            "No permission to open {}. Install the package (its udev rule gives you access to the lighting controllers), then \
             sign out and back in.",
            path.display()
        )
    } else {
        anyhow!("Couldn't open {}: {error}", path.display())
    }
}

/// The vendor and product in a hidraw device's uevent (`HID_ID=0003:0000048D:0000CE00`).
fn ids(uevent: &str) -> Option<(u16, u16)> {
    let id = uevent.lines().find_map(|line| line.strip_prefix("HID_ID="))?;
    let mut parts = id.split(':').skip(1);
    let vendor = u32::from_str_radix(parts.next()?, 16).ok()?;
    let product = u32::from_str_radix(parts.next()?, 16).ok()?;
    Some((vendor as u16, product as u16))
}

/// An open hidraw device.
pub struct HidDevice {
    file: File,
    info: DescriptorInfo,
}

/// hidraw's `HIDIOCSFEATURE(len)` (nr 0x06) and `HIDIOCGFEATURE(len)` (nr 0x07): `_IOC(_IOC_READ|_IOC_WRITE, 'H', nr,
/// len)`.
fn feature_ioctl(nr: libc::c_ulong, length: usize) -> libc::c_ulong {
    (3 << 30) | ((length as libc::c_ulong & 0x3FFF) << 16) | ((b'H' as libc::c_ulong) << 8) | nr
}

impl HidDevice {
    /// The first present device with this vendor, one of these products, and this top-level usage; None if there's
    /// none.
    pub fn find(vendor: u16, products: &[u16], usage_page: u16, usage: u16) -> Result<Option<HidDevice>> {
        let Ok(entries) = std::fs::read_dir("/sys/class/hidraw") else { return Ok(None) };
        let mut names: Vec<_> = entries.flatten().map(|e| e.file_name()).collect();
        names.sort();
        for name in names {
            let sys = Path::new("/sys/class/hidraw").join(&name).join("device");
            let Some((v, p)) = std::fs::read_to_string(sys.join("uevent")).ok().as_deref().and_then(ids) else { continue };
            if v != vendor || !products.contains(&p) {
                continue;
            }
            let Ok(descriptor) = std::fs::read(sys.join("report_descriptor")) else { continue };
            let info = descriptor_info(&descriptor);
            if info.usage_page != usage_page || info.usage != usage {
                continue;
            }
            let path = Path::new("/dev").join(&name);
            let file = OpenOptions::new().read(true).write(true).open(&path).map_err(|e| open_error(&path, e))?;
            return Ok(Some(HidDevice { file, info }));
        }
        Ok(None)
    }

    /// A feature report's length, with its report ID byte.
    pub fn feature_length(&self) -> usize {
        self.info.feature_length
    }

    /// An output report's length, with its report ID byte.
    pub fn output_length(&self) -> usize {
        self.info.output_length
    }

    pub fn set_feature(&self, report: &[u8]) -> Result<()> {
        let result = unsafe { libc::ioctl(self.file.as_raw_fd(), feature_ioctl(0x06, report.len()), report.as_ptr()) };
        if result < 0 {
            return Err(anyhow!("The lighting controller didn't take the report: {}", io::Error::last_os_error()));
        }
        Ok(())
    }

    /// Reads a feature report; `report[0]` is its ID.
    pub fn get_feature(&self, report: &mut [u8]) -> Result<()> {
        let result = unsafe { libc::ioctl(self.file.as_raw_fd(), feature_ioctl(0x07, report.len()), report.as_mut_ptr()) };
        if result < 0 {
            return Err(anyhow!("The lighting controller didn't answer: {}", io::Error::last_os_error()));
        }
        Ok(())
    }

    pub fn write_output(&self, report: &[u8]) -> Result<()> {
        (&self.file).write_all(report).map_err(|e| anyhow!("The lighting controller didn't take the report: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len() / 2).map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap()).collect()
    }

    const KEYBOARD: &str = "0612ff0901a101150026ff0075089540092081020921910209229508b102c0";

    #[test]
    fn keyboard_descriptor() {
        let info = descriptor_info(&hex(KEYBOARD));
        assert_eq!(info, DescriptorInfo { usage_page: 0xFF12, usage: 1, feature_length: 9, output_length: 65 });
    }

    #[test]
    fn light_bar_descriptor() {
        let info = descriptor_info(&hex(&KEYBOARD.replacen("0612ff", "0603ff", 1)));
        assert_eq!(info, DescriptorInfo { usage_page: 0xFF03, usage: 1, feature_length: 9, output_length: 65 });
    }

    #[test]
    fn permission_error_names_the_fix() {
        let error = open_error(Path::new("/dev/hidraw3"), std::io::Error::from_raw_os_error(libc::EACCES)).to_string();
        assert!(error.contains("/dev/hidraw3") && error.contains("udev rule"), "{error}");
    }
}
