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

/// The EC window is one 4 KB page.
const WINDOW: u16 = 0x1000;

pub fn writable(address: u16) -> bool {
    WRITABLE.iter().any(|&(first, last)| (first..=last).contains(&address))
}

pub fn readable(address: u16) -> bool {
    address < WINDOW
}

/// What to undo when a client goes away: per register, the bits to set and their values.
#[derive(Default)]
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
    fn cleared_release_is_not_applied() {
        let mut releases = Releases::default();
        releases.set(0x741, 0x01, 0x00);
        releases.set(0x7C6, 0x04, 0x00);
        releases.clear(0x7C6);
        assert_eq!(releases.take(), vec![(0x741, 0x01, 0x00)]);
    }
}
