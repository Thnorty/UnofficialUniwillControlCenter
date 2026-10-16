//! Which EC registers clients may read and write, and what to undo when a client goes away.

use std::collections::BTreeMap;

/// The registers the app writes: the app-present bit, the GPU's power, the mode and Fan Boost, the trigger byte
/// (Windows key lock, USB charging while off), the power limits, TCC offset and fan switch speed, battery care, the
/// fan table control bytes, and the fan tables with their request bytes.
const WRITABLE: &[(u16, u16)] = &[
    (0x741, 0x741),
    (0x743, 0x746),
    (0x751, 0x751),
    (0x767, 0x767),
    (0x783, 0x787),
    (0x7A6, 0x7A6),
    (0x7C5, 0x7C6),
    (0xF00, 0xF5F),
];

/// The BIOS settings variable's bytes the app changes, as ranges: the boot lighting of the keyboard and the light bar
/// (15-46), the voltage offset up (58-59), the overclock recovery flag (92), the GPU mode (98) and the voltage offset
/// down (100-101).
const VARIABLE_WRITABLE: &[std::ops::Range<usize>] = &[15..47, 58..60, 92..93, 98..99, 100..102];

/// Whether `bytes` with `attributes` may replace the variable that has `current_attributes` and `current`: the same
/// attributes (0 would delete it, APPEND_WRITE grow it) and size, and changes only in the app's fields.
pub fn variable_write_allowed(current_attributes: u32, current: &[u8], attributes: u32, bytes: &[u8]) -> Result<(), String> {
    if attributes != current_attributes {
        return Err(format!("the attributes must stay {current_attributes:#x}, not {attributes:#x}"));
    }
    if bytes.len() != current.len() {
        return Err(format!("the variable must stay {} bytes, not {}", current.len(), bytes.len()));
    }
    match (0..bytes.len()).find(|&i| bytes[i] != current[i] && !VARIABLE_WRITABLE.iter().any(|r| r.contains(&i))) {
        Some(i) => Err(format!("byte {i} isn't one the app changes")),
        None => Ok(()),
    }
}

/// The EC window is one 4 KB page.
const WINDOW: u16 = 0x1000;

pub fn writable(address: u16) -> bool {
    WRITABLE.iter().any(|&(first, last)| (first..=last).contains(&address))
}

pub fn readable(address: u16) -> bool {
    address < WINDOW
}

/// What to undo when a client goes away: per register, the bits to set and their values.
#[derive(Clone, Default)]
pub struct Releases(BTreeMap<u16, (u8, u8)>);

impl Releases {
    pub fn set(&mut self, address: u16, mask: u8, value: u8) {
        self.0.insert(address, (mask, value));
    }

    pub fn clear(&mut self, address: u16) {
        self.0.remove(&address);
    }

    /// Everything registered, by address, leaving nothing.
    pub fn take(&mut self) -> Vec<(u16, u8, u8)> {
        std::mem::take(&mut self.0).into_iter().map(|(address, (mask, value))| (address, mask, value)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_register_the_app_writes_is_writable() {
        let written = [
            0x741, 0x743, 0x744, 0x745, 0x746, 0x751, 0x767, 0x783, 0x784, 0x785, 0x786, 0x787, 0x7A6, 0x7C5, 0x7C6, 0xF00, 0xF2F, 0xF5F,
        ];
        for address in written {
            assert!(writable(address), "0x{address:03X}");
        }
    }

    #[test]
    fn neighbors_are_not_writable() {
        for address in [0x740, 0x742, 0x747, 0x750, 0x752, 0x766, 0x768, 0x782, 0x788, 0x7A5, 0x7A7, 0x7C4, 0x7C7, 0xEFF, 0xF60] {
            assert!(!writable(address), "0x{address:03X}");
        }
    }

    #[test]
    fn reads_cover_the_window() {
        assert!(readable(0x000));
        assert!(readable(0xFFF));
        assert!(!readable(0x1000));
    }

    #[test]
    fn releases_keep_the_latest_per_address() {
        let mut releases = Releases::default();
        releases.set(0x7C6, 0x04, 0x00);
        releases.set(0x7C6, 0x04, 0x04);
        assert_eq!(releases.take(), vec![(0x7C6, 0x04, 0x04)]);
        assert!(releases.take().is_empty());
    }

    #[test]
    fn variable_writes_change_only_the_apps_fields() {
        let before = vec![0u8; 180];
        let changed = |at: &[usize]| {
            let mut bytes = before.clone();
            for &i in at {
                bytes[i] = 0x5A;
            }
            bytes
        };
        // Boot lighting (15-46), the voltage offset (58-59, 100-101), the recovery flag (92) and the GPU mode (98).
        for fields in [&[15, 30, 31, 46][..], &[58, 59, 100, 101], &[92], &[98]] {
            assert_eq!(variable_write_allowed(7, &before, 7, &changed(fields)), Ok(()), "{fields:?}");
        }
        // The rest is the BIOS's, such as the voltage offset's maximum (60) and the bytes next to the app's.
        for i in [0, 14, 47, 57, 60, 91, 93, 97, 99, 102, 179] {
            assert!(variable_write_allowed(7, &before, 7, &changed(&[i])).is_err(), "byte {i}");
        }
        assert!(variable_write_allowed(7, &before, 7, &before[..179]).is_err());
    }

    #[test]
    fn variable_writes_keep_the_attributes() {
        let before = vec![0u8; 180];
        let mut gpu = before.clone();
        gpu[98] = 1;
        // Attributes 0 would delete the variable; APPEND_WRITE (0x40) would grow it until the NVRAM is full.
        assert!(variable_write_allowed(7, &before, 0, &gpu).is_err());
        assert!(variable_write_allowed(7, &before, 7 | 0x40, &gpu).is_err());
        assert!(variable_write_allowed(7, &before, 3, &gpu).is_err());
    }

    #[test]
    fn cleared_release_is_not_applied() {
        let mut releases = Releases::default();
        releases.set(0x741, 0x01, 0x00);
        releases.set(0x7C6, 0x04, 0x00);
        releases.clear(0x7C6);
        assert_eq!(releases.take(), vec![(0x741, 0x01, 0x00)]);
    }
}
