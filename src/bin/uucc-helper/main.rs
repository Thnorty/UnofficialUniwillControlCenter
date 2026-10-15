//! `uucc-helper`: the small root service the Linux app reaches the embedded controller and the BIOS settings variable
//! through. It does nothing else, and only for users signed in at the laptop.

#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../../hardware/helper_protocol.rs"]
mod protocol;
#[cfg(target_os = "linux")]
mod rules;

fn main() {}
