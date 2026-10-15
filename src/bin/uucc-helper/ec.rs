//! The EC's RAM, memory-mapped: the firmware's own `ECRR`/`ECRW` methods (on the `INOU0000` device) read and write
//! the byte at a fixed base plus the register's address. The helper takes that base from the DSDT, maps the page
//! from `/dev/mem`, and reads and writes single bytes as the firmware's `MMRW` does.

use std::fs::OpenOptions;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::protocol::Unavailable;

/// The window's size: registers 0x000-0xFFF.
const WINDOW: usize = 0x1000;

/// The EC window's base from the DSDT's `ECRR` method, whose body starts `Add (base, Arg0, Local0)`:
/// `14 <PkgLength> "ECRR" <flags> 72 0C <base, u32 LE> 68`. None if there's no such method.
pub fn ecrr_base(dsdt: &[u8]) -> Option<u64> {
    let mut from = 0;
    while let Some(found) = find(&dsdt[from..], b"ECRR") {
        let name = from + found;
        from = name + 1;
        // A Method op, then a PkgLength of 1-4 bytes whose first byte's top two bits count the bytes after it.
        let method = (0..4).any(|extra| {
            name.checked_sub(2 + extra).is_some_and(|op| dsdt[op] == 0x14 && (dsdt[op + 1] >> 6) as usize == extra)
        });
        let body = name + 5; // past the name and the method's flags
        if method && dsdt.get(body..body + 7).is_some_and(|b| b[0] == 0x72 && b[1] == 0x0C && b[6] == 0x68) {
            return Some(u32::from_le_bytes(dsdt[body + 2..body + 6].try_into().ok()?) as u64);
        }
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Where the helper looks; tests use a temporary tree.
pub struct Paths {
    pub lockdown: PathBuf,
    pub inou: PathBuf,
    pub dsdt: PathBuf,
}

impl Paths {
    pub fn system() -> Paths {
        Paths {
            lockdown: "/sys/kernel/security/lockdown".into(),
            inou: "/sys/bus/acpi/devices/INOU0000:00".into(),
            dsdt: "/sys/firmware/acpi/tables/DSDT".into(),
        }
    }
}

/// The EC window's base, or why the EC can't be used: Secure Boot's lockdown (which blocks /dev/mem), no EC device,
/// a kernel driver already in charge of it, or a DSDT without the method this app knows.
pub fn check(paths: &Paths) -> Result<u64, Unavailable> {
    // "[none] integrity confidentiality"; without the file (no lockdown LSM), there's no lockdown.
    let lockdown = std::fs::read_to_string(&paths.lockdown).unwrap_or_default();
    if lockdown.contains("[integrity]") || lockdown.contains("[confidentiality]") {
        return Err(Unavailable::SecureBoot);
    }
    if !paths.inou.exists() {
        return Err(Unavailable::NoEcDevice);
    }
    if let Ok(driver) = std::fs::read_link(paths.inou.join("physical_node").join("driver")) {
        let driver = driver.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        return Err(Unavailable::DriverInCharge { driver });
    }
    let dsdt = std::fs::read(&paths.dsdt).map_err(|_| Unavailable::NoEcDevice)?;
    ecrr_base(&dsdt).ok_or(Unavailable::NoEcDevice)
}

/// Reading and writing EC registers; the real window or, in tests, a fake.
pub trait EcAccess: Send {
    fn read(&mut self, address: u16) -> u8;
    fn write(&mut self, address: u16, value: u8);
}

/// The EC's RAM mapped from /dev/mem.
pub struct EcWindow {
    memory: *mut u8,
}

// The mapping is only used under the helper's EC lock.
unsafe impl Send for EcWindow {}

impl EcWindow {
    pub fn map(base: u64) -> io::Result<EcWindow> {
        let mem = OpenOptions::new().read(true).write(true).custom_flags(libc::O_SYNC).open(Path::new("/dev/mem"))?;
        let memory = unsafe {
            libc::mmap(std::ptr::null_mut(), WINDOW, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, mem.as_raw_fd(), base as libc::off_t)
        };
        if memory == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        // The mapping outlives the file.
        Ok(EcWindow { memory: memory as *mut u8 })
    }
}

impl EcAccess for EcWindow {
    fn read(&mut self, address: u16) -> u8 {
        assert!((address as usize) < WINDOW);
        unsafe { self.memory.add(address as usize).read_volatile() }
    }

    fn write(&mut self, address: u16, value: u8) {
        assert!((address as usize) < WINDOW);
        unsafe { self.memory.add(address as usize).write_volatile(value) }
    }
}

impl Drop for EcWindow {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.memory as *mut libc::c_void, WINDOW);
        }
    }
}

/// The EC's RAM in memory, for tests.
#[cfg(test)]
pub struct FakeEc(pub [u8; WINDOW]);

#[cfg(test)]
impl EcAccess for FakeEc {
    fn read(&mut self, address: u16) -> u8 {
        self.0[address as usize]
    }

    fn write(&mut self, address: u16, value: u8) {
        self.0[address as usize] = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const ECRR: &[u8] = include_bytes!("testdata/ecrr.bin");

    #[test]
    fn finds_the_base_in_this_laptops_ecrr() {
        assert_eq!(ecrr_base(ECRR), Some(0xFE41_0000));
        // Anywhere in a larger table.
        let mut dsdt = vec![0x5Bu8; 1000];
        dsdt.extend_from_slice(ECRR);
        dsdt.extend_from_slice(&[0u8; 50]);
        assert_eq!(ecrr_base(&dsdt), Some(0xFE41_0000));
    }

    #[test]
    fn other_method_body_gives_none() {
        let mut changed = ECRR.to_vec();
        changed[7] = 0x70; // Store instead of Add
        assert_eq!(ecrr_base(&changed), None);
    }

    #[test]
    fn ecrr_without_method_op_gives_none() {
        let mut changed = ECRR.to_vec();
        changed[0] = 0x08; // a Name, not a Method
        assert_eq!(ecrr_base(&changed), None);
    }

    /// A fake /sys tree in a temporary folder.
    struct Tree(std::path::PathBuf);

    impl Tree {
        fn new(name: &str) -> Tree {
            let root = std::env::temp_dir().join(format!("uucc-helper-test-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Tree(root)
        }

        fn paths(&self) -> Paths {
            Paths { lockdown: self.0.join("lockdown"), inou: self.0.join("INOU0000:00"), dsdt: self.0.join("DSDT") }
        }

        /// A laptop like this one: no lockdown, the EC device without a driver, and the DSDT.
        fn ready(name: &str) -> Tree {
            let tree = Tree::new(name);
            fs::write(tree.0.join("lockdown"), "[none] integrity confidentiality\n").unwrap();
            fs::create_dir_all(tree.0.join("INOU0000:00")).unwrap();
            fs::write(tree.0.join("DSDT"), ECRR).unwrap();
            tree
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn ready_returns_the_base() {
        let tree = Tree::ready("ready");
        assert_eq!(check(&tree.paths()), Ok(0xFE41_0000));
    }

    #[test]
    fn lockdown_none_passes() {
        let tree = Tree::ready("none");
        assert!(check(&tree.paths()).is_ok());
    }

    #[test]
    fn lockdown_integrity_means_secure_boot() {
        let tree = Tree::ready("integrity");
        fs::write(tree.0.join("lockdown"), "none [integrity] confidentiality\n").unwrap();
        assert_eq!(check(&tree.paths()), Err(Unavailable::SecureBoot));
        fs::write(tree.0.join("lockdown"), "none integrity [confidentiality]\n").unwrap();
        assert_eq!(check(&tree.paths()), Err(Unavailable::SecureBoot));
    }

    #[test]
    fn missing_lockdown_file_means_no_lockdown() {
        let tree = Tree::ready("no-lockdown");
        fs::remove_file(tree.0.join("lockdown")).unwrap();
        assert_eq!(check(&tree.paths()), Ok(0xFE41_0000));
    }

    #[test]
    fn no_inou_device() {
        let tree = Tree::ready("no-inou");
        fs::remove_dir_all(tree.0.join("INOU0000:00")).unwrap();
        assert_eq!(check(&tree.paths()), Err(Unavailable::NoEcDevice));
    }

    #[test]
    fn unknown_dsdt_means_no_ec_device() {
        let tree = Tree::ready("no-ecrr");
        fs::write(tree.0.join("DSDT"), [0u8; 64]).unwrap();
        assert_eq!(check(&tree.paths()), Err(Unavailable::NoEcDevice));
    }

    #[test]
    fn bound_driver_is_named() {
        let tree = Tree::ready("driver");
        let driver = tree.0.join("drivers").join("uniwill");
        fs::create_dir_all(&driver).unwrap();
        let node = tree.0.join("INOU0000:00").join("physical_node");
        fs::create_dir_all(&node).unwrap();
        std::os::unix::fs::symlink(&driver, node.join("driver")).unwrap();
        assert_eq!(check(&tree.paths()), Err(Unavailable::DriverInCharge { driver: "uniwill".into() }));
    }
}
