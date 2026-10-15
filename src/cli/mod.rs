//! `uucc`, the terminal command: the request and reply format it shares with the app, and the app's pipe server.

pub mod protocol;
#[cfg(windows)]
pub mod server;
#[cfg(target_os = "linux")]
#[path = "server_linux.rs"]
pub mod server;
