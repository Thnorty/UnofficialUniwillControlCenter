//! The tray icon, the pop-ups, the single-instance lock, and opening files and folders.

#[cfg(windows)]
mod dialogs;
#[cfg(target_os = "linux")]
#[path = "dialogs_linux.rs"]
mod dialogs;
pub mod display;
#[cfg(windows)]
pub mod instance;
#[cfg(target_os = "linux")]
#[path = "instance_linux.rs"]
pub mod instance;
#[cfg(windows)]
pub mod osd;
#[cfg(target_os = "linux")]
#[path = "osd_linux.rs"]
pub mod osd;
#[cfg(windows)]
pub mod tray;
mod tray_common;
#[cfg(target_os = "linux")]
#[path = "tray_linux.rs"]
pub mod tray;

pub use dialogs::{Owner, confirm, error_box, open_path, pick_file, pick_save_file, warning_ok_cancel};
