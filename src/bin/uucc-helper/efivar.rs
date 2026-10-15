//! `UniWillVariable` in efivarfs: 4 bytes of attributes, then the 180 bytes of Control Center's NVRAM_STRUCT. Linux
//! marks every variable file immutable, so a write lifts that first and puts it back after.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::Path;

pub const VARIABLE: &str = "/sys/firmware/efi/efivars/UniWillVariable-9f33f85c-13ca-4fd1-9c4a-96217722c593";
/// NVRAM_STRUCT's size.
pub const SIZE: usize = 180;

/// A file's immutable flag; the real one through ioctls, a fake in tests.
pub trait Immutable {
    fn get(&self, file: &File) -> io::Result<bool>;
    fn set(&self, file: &File, on: bool) -> io::Result<()>;
}

/// FS_IOC_GETFLAGS / FS_IOC_SETFLAGS, with FS_IMMUTABLE_FL.
pub struct Ioctl;

const FS_IOC_GETFLAGS: libc::c_ulong = 0x8008_6601;
const FS_IOC_SETFLAGS: libc::c_ulong = 0x4008_6602;
const FS_IMMUTABLE_FL: libc::c_int = 0x10;

impl Ioctl {
    fn flags(file: &File) -> io::Result<libc::c_int> {
        let mut flags: libc::c_int = 0;
        if unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_GETFLAGS, &mut flags) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(flags)
    }
}

impl Immutable for Ioctl {
    fn get(&self, file: &File) -> io::Result<bool> {
        Ok(Ioctl::flags(file)? & FS_IMMUTABLE_FL != 0)
    }

    fn set(&self, file: &File, on: bool) -> io::Result<()> {
        let flags = Ioctl::flags(file)?;
        let flags = if on { flags | FS_IMMUTABLE_FL } else { flags & !FS_IMMUTABLE_FL };
        if unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_SETFLAGS, &flags) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// The variable's attributes and its data.
pub fn read(path: &Path) -> io::Result<(u32, Vec<u8>)> {
    let contents = std::fs::read(path)?;
    if contents.len() != 4 + SIZE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("the variable is {} bytes, not {SIZE}", contents.len().saturating_sub(4))));
    }
    Ok((u32::from_le_bytes(contents[..4].try_into().unwrap()), contents[4..].to_vec()))
}

/// Writes the variable in one write, as efivarfs needs: its attributes, then its data. The immutable flag is lifted
/// for it and put back as it was, also when the write fails.
pub fn write(path: &Path, attributes: u32, bytes: &[u8], flags: &impl Immutable) -> io::Result<()> {
    if bytes.len() != SIZE {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("the variable must be {SIZE} bytes, not {}", bytes.len())));
    }
    // An immutable file can't be opened for writing, so the flag is changed through a read-only handle.
    let handle = File::open(path)?;
    let was_immutable = flags.get(&handle)?;
    if was_immutable {
        flags.set(&handle, false)?;
    }
    let mut contents = attributes.to_le_bytes().to_vec();
    contents.extend_from_slice(bytes);
    let written = OpenOptions::new().write(true).open(path).and_then(|mut file| {
        let n = file.write(&contents)?;
        if n != contents.len() {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "the variable was only partly written"));
        }
        Ok(())
    });
    let restored = if was_immutable { flags.set(&handle, true) } else { Ok(()) };
    written.and(restored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    struct FakeFlag {
        on: Cell<bool>,
        sets: Cell<u32>,
    }

    impl Immutable for FakeFlag {
        fn get(&self, _file: &File) -> io::Result<bool> {
            Ok(self.on.get())
        }

        fn set(&self, _file: &File, on: bool) -> io::Result<()> {
            self.on.set(on);
            self.sets.set(self.sets.get() + 1);
            Ok(())
        }
    }

    fn temp(name: &str, contents: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("uucc-efivar-test-{}-{name}", std::process::id()));
        let _ = fs::remove_file(&path);
        fs::write(&path, contents).unwrap();
        path
    }

    fn variable(attributes: u32, fill: u8) -> Vec<u8> {
        let mut bytes = attributes.to_le_bytes().to_vec();
        bytes.extend([fill; SIZE]);
        bytes
    }

    #[test]
    fn reads_attributes_and_data() {
        let path = temp("read", &variable(7, 0xAB));
        let (attributes, bytes) = read(&path).unwrap();
        assert_eq!(attributes, 7);
        assert_eq!(bytes, vec![0xAB; SIZE]);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn wrong_size_is_rejected() {
        let path = temp("size", &variable(7, 0));
        let flag = FakeFlag { on: Cell::new(true), sets: Cell::new(0) };
        assert!(write(&path, 7, &[1u8; SIZE - 1], &flag).is_err());
        assert_eq!(fs::read(&path).unwrap(), variable(7, 0));
        assert_eq!(flag.sets.get(), 0);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn write_clears_and_restores_the_flag() {
        let path = temp("write", &variable(7, 0));
        let flag = FakeFlag { on: Cell::new(true), sets: Cell::new(0) };
        write(&path, 7, &[5u8; SIZE], &flag).unwrap();
        assert_eq!(fs::read(&path).unwrap(), variable(7, 5));
        assert!(flag.on.get());
        assert_eq!(flag.sets.get(), 2);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn flag_restored_when_write_fails() {
        let path = temp("fails", &variable(7, 0));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        let flag = FakeFlag { on: Cell::new(true), sets: Cell::new(0) };
        assert!(write(&path, 7, &[5u8; SIZE], &flag).is_err());
        assert!(flag.on.get());
        assert_eq!(fs::read(&path).unwrap(), variable(7, 0));
        fs::remove_file(path).unwrap();
    }
}
