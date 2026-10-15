//! One instance at a time on Linux. Not enforced yet: every launch runs.

use std::time::Duration;

/// Takes the single-instance lock, or shows the running instance and returns false.
pub fn acquire() -> bool {
    true
}

/// The app runs as the user on Linux; the helper does what needs root.
pub fn is_elevated() -> bool {
    true
}

/// Never needed on Linux.
pub fn open_elevated(_arguments: &[String]) {}

/// Asks a running instance to quit and waits up to `wait` for it; false if none was running.
pub fn close_running(_wait: Duration) -> bool {
    false
}

/// Marks this instance as exiting.
pub fn note_exiting() {}
