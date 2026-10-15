//! The embedded controller's RAM on Linux. Not reached yet: connecting fails.

use std::time::Duration;

use anyhow::{Result, bail};

/// A handle to the EC; clones share it.
#[derive(Clone)]
pub struct Ec {
    _private: (),
}

impl Ec {
    /// Connects to the EC.
    pub fn connect() -> Result<Ec> {
        bail!("Not available on Linux yet")
    }

    /// Runs `job` with the EC's registers and waits at most `timeout` for its result.
    pub fn run_for<R: Send + 'static>(&self, _timeout: Duration, job: impl FnOnce(&Registers) -> Result<R> + Send + 'static) -> Result<R> {
        job(&Registers { _private: () })
    }

    /// Runs `job` with the EC's registers without waiting.
    pub fn spawn(&self, job: impl FnOnce(&Registers) + Send + 'static) {
        job(&Registers { _private: () });
    }
}

/// The EC's RAM.
pub struct Registers {
    _private: (),
}

impl Registers {
    pub fn read(&self, _address: u16) -> Result<u8> {
        bail!("Not available on Linux yet")
    }

    pub fn write(&self, _address: u16, _value: u8) -> Result<()> {
        bail!("Not available on Linux yet")
    }

    /// Sets the bits in `mask` to those of `value`, leaving the others.
    pub fn update(&self, address: u16, mask: u8, value: u8) -> Result<()> {
        let current = self.read(address)?;
        let next = (current & !mask) | (value & mask);
        if next != current { self.write(address, next) } else { Ok(()) }
    }

    pub fn bit(&self, address: u16, bit: u8) -> Result<bool> {
        Ok(self.read(address)? & (1 << bit) != 0)
    }
}
