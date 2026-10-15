//! Reaching the running app over its socket on Linux, and starting it when asked to. Not connected yet.

use crate::protocol::{Envelope, Reply};

#[derive(Debug)]
#[allow(dead_code)] // until the socket is used
pub enum Failure {
    NotRunning,
    DidntStart,
    /// The app couldn't be started at all: why.
    CantStart(String),
    NoAnswer,
    /// The app runs, but its socket isn't there.
    NoPipe,
    Io(String),
}

impl Failure {
    pub fn code(&self) -> i32 {
        match self {
            Failure::NotRunning | Failure::DidntStart | Failure::CantStart(_) => 2,
            Failure::NoAnswer | Failure::NoPipe | Failure::Io(_) => 3,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Failure::NotRunning => "Unofficial Uniwill Control Center isn't running. Start it, or add --start.".into(),
            Failure::DidntStart => "Unofficial Uniwill Control Center didn't start within 15 s. If it's new on this PC, it may be waiting for its first-start question: open it once from your app menu.".into(),
            Failure::CantStart(why) => format!("Couldn't start Unofficial Uniwill Control Center: {why}"),
            Failure::NoAnswer => "The app didn't answer.".into(),
            Failure::NoPipe => "Unofficial Uniwill Control Center is running, but uucc can't reach it. It may still be starting \
                (or waiting for its first-start question); otherwise it couldn't open its connection for uucc: see \
                ~/.config/UnofficialUniwillControlCenter/uucc.log, and restart the app."
                .into(),
            Failure::Io(e) => format!("Couldn't talk to the app: {e}"),
        }
    }
}

/// Sends `envelope` to the app (the demo's when `demo`) and returns its reply. With `start`, starts the app first if
/// it isn't running.
pub fn send(_envelope: &Envelope, _demo: bool, _start: bool) -> Result<Reply, Failure> {
    Err(Failure::NotRunning)
}
