//! The HID transport to the lighting controllers on Linux. Not reached yet: no controller is found.

use anyhow::{Result, bail};

/// An open HID device.
pub struct HidDevice {
    _private: (),
}

impl HidDevice {
    /// The first present device with this vendor, one of these products, and this top-level usage; None if there's
    /// none.
    pub fn find(_vendor: u16, _products: &[u16], _usage_page: u16, _usage: u16) -> Result<Option<HidDevice>> {
        Ok(None)
    }

    /// A feature report's length, with its report ID byte.
    pub fn feature_length(&self) -> usize {
        9
    }

    /// An output report's length, with its report ID byte.
    pub fn output_length(&self) -> usize {
        65
    }

    pub fn set_feature(&self, _report: &[u8]) -> Result<()> {
        bail!("Not available on Linux yet")
    }

    pub fn get_feature(&self, _report: &mut [u8]) -> Result<()> {
        bail!("Not available on Linux yet")
    }

    pub fn write_output(&self, _report: &[u8]) -> Result<()> {
        bail!("Not available on Linux yet")
    }
}
