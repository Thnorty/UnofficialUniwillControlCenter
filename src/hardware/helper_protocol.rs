//! What the Linux app and `uucc-helper` say to each other: one JSON line per request and per reply, over the helper's
//! socket.

use std::io::{self, BufRead, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Changes when a request or reply changes, so an app and a helper of different versions notice.
pub const VERSION: u32 = 2;
/// Where the helper listens; systemd opens it (`uucc-helper.socket`).
pub const SOCKET: &str = "/run/uucc-helper.sock";
/// The longest line either side reads.
pub const MAX_LINE: usize = 64 * 1024;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Envelope {
    pub version: u32,
    pub request: Request,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum Request {
    /// Whether the EC can be used, and if not, why.
    Status,
    Read { address: u16 },
    ReadMany { addresses: Vec<u16> },
    Write { address: u16, value: u8 },
    /// Sets the bits in `mask` to those of `value`, as one step no other client's change can interleave with.
    Update { address: u16, mask: u8, value: u8 },
    /// What the helper does if this client goes away without `ClearOnDisconnect`: crashed or killed.
    OnDisconnect { address: u16, mask: u8, value: u8 },
    ClearOnDisconnect { address: u16 },
    ReadVariable,
    WriteVariable { attributes: u32, bytes: Vec<u8> },
    /// The laptop's hotkeys: sent first on a connection of its own, which then carries only `Hotkey` replies.
    Hotkeys,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum Reply {
    Ready,
    Unavailable { reason: Unavailable },
    Byte { value: u8 },
    Bytes { values: Vec<u8> },
    Done,
    Variable { attributes: u32, bytes: Vec<u8> },
    /// A hotkey's code (Control Center's: 176 the performance key, 177/178 keyboard light down/up, ...).
    Hotkey { code: u8 },
    /// The request isn't allowed: a register the app doesn't write, or a client that isn't signed in at the laptop.
    Refused { message: String },
    Error { message: String },
    WrongVersion { helper: u32 },
}

/// Why the EC can't be used.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "reason")]
pub enum Unavailable {
    SecureBoot,
    NoEcDevice,
    DriverInCharge { driver: String },
    MapFailed { error: String },
}

impl Unavailable {
    pub fn message(&self) -> String {
        match self {
            Unavailable::SecureBoot => "Secure Boot is on, so Linux blocks direct access to the embedded controller. Modes, fans, power \
                limits and battery care need Secure Boot off for now: see the README's Linux section."
                .into(),
            Unavailable::NoEcDevice => {
                "This laptop's firmware has no embedded controller interface this app knows (INOU0000 with ECRR).".into()
            }
            Unavailable::DriverInCharge { driver } => {
                format!("Linux's {driver} driver controls the embedded controller, so this app leaves it alone.")
            }
            Unavailable::MapFailed { error } => format!("Couldn't reach the embedded controller: {error}"),
        }
    }
}

/// Writes `value` as one JSON line.
pub fn write_line(w: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()
}

/// Reads one JSON line; None at the end of the stream. A line longer than `MAX_LINE` is an error.
pub fn read_line<T: DeserializeOwned>(r: &mut impl BufRead) -> io::Result<Option<T>> {
    let mut line = Vec::new();
    r.take(MAX_LINE as u64 + 1).read_until(b'\n', &mut line)?;
    if line.len() > MAX_LINE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
    }
    if line.is_empty() {
        return Ok(None);
    }
    serde_json::from_slice(&line).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn request_round_trips_as_one_json_line() {
        let envelope = Envelope { version: VERSION, request: Request::Update { address: 0x7C6, mask: 4, value: 4 } };
        let mut out = Vec::new();
        write_line(&mut out, &envelope).unwrap();
        assert_eq!(out.iter().filter(|&&b| b == b'\n').count(), 1);
        assert_eq!(*out.last().unwrap(), b'\n');
        let mut reader = BufReader::new(Cursor::new(out));
        let back: Envelope = read_line(&mut reader).unwrap().unwrap();
        assert_eq!(back, envelope);
        assert!(read_line::<Envelope>(&mut reader).unwrap().is_none());
    }

    #[test]
    fn oversized_line_is_an_error() {
        let mut line = vec![b'a'; MAX_LINE + 10];
        line.push(b'\n');
        let mut reader = BufReader::new(Cursor::new(line));
        assert!(read_line::<Envelope>(&mut reader).is_err());
    }

    #[test]
    fn unavailable_reasons_say_why() {
        assert!(Unavailable::SecureBoot.message().contains("Secure Boot"));
        assert!(Unavailable::DriverInCharge { driver: "uniwill".into() }.message().contains("Linux's uniwill driver"));
        assert!(Unavailable::MapFailed { error: "denied".into() }.message().ends_with("denied"));
    }
}
