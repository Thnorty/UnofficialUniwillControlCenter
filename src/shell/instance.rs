//! One instance at a time: two would send interleaved commands to the same controllers. A second launch brings the
//! running window forward, unless that instance is exiting: it still holds the lock for a few seconds while it hands
//! the Fn keys and fans back, so the new one waits for it and then starts.

use std::time::Duration;

use windows::Win32::Foundation::{HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{CreateEventW, CreateMutexW, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject};
use windows::core::w;

use super::tray;

const LOCK: windows::core::PCWSTR = w!("Local\\TulparControl");
const EXITING: windows::core::PCWSTR = w!("Local\\TulparControl.Exiting");
const EXIT_WAIT: Duration = Duration::from_secs(15);

/// Takes the single-instance lock, or shows the running instance and returns false.
pub fn acquire() -> bool {
    let Ok(lock) = (unsafe { CreateMutexW(None, false, LOCK) }) else { return true };
    // Held until the process ends.
    if owned(lock, Duration::ZERO) {
        return true;
    }
    let exiting = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, EXITING) }.is_ok();
    if exiting {
        return owned(lock, EXIT_WAIT);
    }
    tray::signal_running_instance();
    false
}

fn owned(lock: HANDLE, wait: Duration) -> bool {
    let result = unsafe { WaitForSingleObject(lock, wait.as_millis() as u32) };
    result == WAIT_OBJECT_0 || result == WAIT_ABANDONED // abandoned: the previous instance ended without releasing it
}

/// Tells later launches that this instance is on its way out. The event lives as long as this process.
pub fn note_exiting() {
    // The handle isn't closed, so the event stays until the process ends.
    let _ = unsafe { CreateEventW(None, true, true, EXITING) };
}
