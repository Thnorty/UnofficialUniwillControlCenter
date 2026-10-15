//! The app's end of the socket `uucc` talks to on Linux. Not opened yet.

use anyhow::{Result, bail};

use super::protocol::{Envelope, Reply};

/// Opens the socket `name` and answers each request with `handle`.
pub fn start(_name: &str, _handle: impl Fn(Envelope) -> Reply + Send + 'static) -> Result<()> {
    bail!("Not available on Linux yet")
}
