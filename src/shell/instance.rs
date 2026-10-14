//! One instance at a time: two would send interleaved commands to the same controllers. A second launch brings the
//! running window forward, unless that instance is exiting: it still holds the lock for a few seconds while it hands
//! the Fn keys and fans back, so the new one waits for it and then starts.
//!
//! The app needs administrator rights, but its manifest doesn't ask for them: a launch without them (the Start menu,
//! a shortcut) opens the running app without a UAC prompt, and otherwise starts it with them.

use std::os::windows::io::AsRawHandle;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, E_ACCESSDENIED, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Diagnostics::Debug::{
    EXCEPTION_EXECUTE_HANDLER, EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules, MiniDumpWriteDump, SetUnhandledExceptionFilter,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, OpenEventW, OpenProcessToken, ReleaseMutex,
    SYNCHRONIZATION_SYNCHRONIZE, TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

use crate::hardware::system::startup;

use super::tray;

// The names from when the app was called Tulpar Control, so this copy and one of then never run together.
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
    if exiting() {
        return owned(lock, EXIT_WAIT);
    }
    tray::signal_running_instance();
    false
}

/// Whether the running instance is on its way out. Without administrator rights the event can't be opened, which
/// still means it's there.
fn exiting() -> bool {
    match unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, EXITING) } {
        Ok(event) => {
            let _ = unsafe { CloseHandle(event) };
            true
        }
        Err(e) => e.code() == E_ACCESSDENIED,
    }
}

/// Whether this process has administrator rights.
pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = 0;
        let read = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        );
        let _ = CloseHandle(token);
        read.is_ok() && elevation.TokenIsElevated != 0
    }
}

/// For a launch without administrator rights: shows the running app's window. When it isn't running, starts it with
/// administrator rights: through the startup task if that starts this exe (no prompt, as when signing in), otherwise
/// with a UAC prompt.
pub fn open_elevated(arguments: &[String]) {
    if !exiting() && tray::show_running() {
        return;
    }
    if !exiting() && startup::starts_this_exe() && startup::run_now() {
        // The task starts it in the tray; its window is shown once it's there.
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(250));
            if tray::show_running() {
                return;
            }
        }
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let file = HSTRING::from(exe.as_os_str());
    let parameters = HSTRING::from(arguments.iter().map(|a| format!("\"{}\"", a.replace('"', "\\\""))).collect::<Vec<_>>().join(" "));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // Declined at the prompt: nothing to do.
    let _ = unsafe { ShellExecuteExW(&mut info) };
}

fn owned(lock: HANDLE, wait: Duration) -> bool {
    let result = unsafe { WaitForSingleObject(lock, wait.as_millis() as u32) };
    result == WAIT_OBJECT_0 || result == WAIT_ABANDONED // abandoned: the previous instance ended without releasing it
}

/// Asks the running instance, if any, to quit, and waits until it has handed everything back and ended. False if it
/// was still running after `wait`.
pub fn close_running(wait: Duration) -> bool {
    let Ok(lock) = (unsafe { CreateMutexW(None, false, LOCK) }) else { return true };
    let closed = owned(lock, Duration::ZERO) || {
        tray::signal_exit();
        owned(lock, wait)
    };
    unsafe {
        let _ = ReleaseMutex(lock);
        let _ = CloseHandle(lock);
    }
    closed
}

/// Tells later launches that this instance is on its way out. The event lives as long as this process.
pub fn note_exiting() {
    // The handle isn't closed, so the event stays until the process ends.
    let _ = unsafe { CreateEventW(None, true, true, EXITING) };
    unsafe {
        SetUnhandledExceptionFilter(Some(crashed_while_exiting));
    }
}

/// The file a crash while exiting leaves, in the app's folder.
const EXIT_CRASH_DUMP: &str = "exit-crash.dmp";

/// A crash while exiting (seen rarely, in a system DLL, after hours of running) ends the process at once, with a small
/// dump to find it by. Otherwise Windows Error Reporting holds the process for about half a minute, and so the
/// installer, which waits for it to end.
unsafe extern "system" fn crashed_while_exiting(info: *const EXCEPTION_POINTERS) -> i32 {
    unsafe {
        let path = std::env::var_os("APPDATA").map(|a| std::path::PathBuf::from(a).join(crate::settings::FOLDER).join(EXIT_CRASH_DUMP));
        if let Some(file) = path.and_then(|p| std::fs::File::create(p).ok()) {
            let exception = MINIDUMP_EXCEPTION_INFORMATION { ThreadId: GetCurrentThreadId(), ExceptionPointers: info as *mut _, ClientPointers: false.into() };
            let _ = MiniDumpWriteDump(
                GetCurrentProcess(),
                GetCurrentProcessId(),
                HANDLE(file.as_raw_handle()),
                MiniDumpWithThreadInfo | MiniDumpWithUnloadedModules,
                Some(&exception),
                None,
                None,
            );
        }
        let code = info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()).map_or(1, |r| r.ExceptionCode.0 as u32);
        let _ = TerminateProcess(GetCurrentProcess(), code);
    }
    EXCEPTION_EXECUTE_HANDLER
}
