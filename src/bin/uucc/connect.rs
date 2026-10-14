//! Reaching the running app over its pipe, and starting it when asked to.

use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::Win32::System::Pipes::WaitNamedPipeW;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, w};

use crate::protocol::{Envelope, Reply, pipe_name, read_message, write_message};

/// Longer than the app's own 15 s wait for its window's thread.
const READ_TIMEOUT: Duration = Duration::from_secs(20);
/// How long `--start` waits for the app's pipe.
const START_TIMEOUT: Duration = Duration::from_secs(15);
const TASK: &str = "UnofficialUniwillControlCenter";
const APP_EXE: &str = "UnofficialUniwillControlCenter.exe";

#[derive(Debug)]
pub enum Failure {
    NotRunning,
    DidntStart,
    /// The app couldn't be started at all: why.
    CantStart(String),
    NoAnswer,
    Io(String),
}

impl Failure {
    pub fn code(&self) -> i32 {
        match self {
            Failure::NotRunning | Failure::DidntStart | Failure::CantStart(_) => 2,
            Failure::NoAnswer | Failure::Io(_) => 3,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Failure::NotRunning => "Unofficial Uniwill Control Center isn't running. Start it, or add --start.".into(),
            Failure::DidntStart => "Unofficial Uniwill Control Center didn't start within 15 s. If it's new on this PC, it may be waiting for its first-start question: open it once from Start.".into(),
            Failure::CantStart(why) => format!("Couldn't start Unofficial Uniwill Control Center: {why}"),
            Failure::NoAnswer => "The app didn't answer.".into(),
            Failure::Io(e) => format!("Couldn't talk to the app: {e}"),
        }
    }
}

/// Sends `envelope` to the app (the demo's when `demo`) and returns its reply. With `start`, starts the app first if
/// it isn't running.
pub fn send(envelope: &Envelope, demo: bool, start: bool) -> Result<Reply, Failure> {
    send_with_timeout(pipe_name(demo), envelope, start && !demo, READ_TIMEOUT)
}

/// `read_timeout` also limits the wait while other clients are answered.
pub fn send_with_timeout(name: &str, envelope: &Envelope, start: bool, read_timeout: Duration) -> Result<Reply, Failure> {
    let mut pipe = open(name, start, read_timeout)?;
    let envelope = envelope.clone();
    let (tx, rx) = mpsc::channel();
    // The read blocks; a thread lets it give up. A thread still waiting ends with the process.
    std::thread::spawn(move || {
        let result = write_message(&mut pipe, &envelope).and_then(|_| read_message::<Reply>(&mut pipe));
        let _ = tx.send(result);
    });
    match rx.recv_timeout(read_timeout) {
        Ok(Ok(reply)) => Ok(reply),
        Ok(Err(e)) => Err(Failure::Io(e.to_string())),
        Err(_) => Err(Failure::NoAnswer),
    }
}

/// Connects, waiting up to `busy_limit` while other clients are being answered.
fn open(name: &str, start: bool, busy_limit: Duration) -> Result<std::fs::File, Failure> {
    let mut started: Option<Instant> = None;
    let mut busy_since: Option<Instant> = None;
    loop {
        match OpenOptions::new().read(true).write(true).open(name) {
            Ok(pipe) => return Ok(pipe),
            // ERROR_PIPE_BUSY: another uucc is being answered.
            Err(e) if e.raw_os_error() == Some(231) => {
                let since = *busy_since.get_or_insert_with(Instant::now);
                let left = busy_limit.saturating_sub(since.elapsed());
                if left.is_zero() {
                    return Err(Failure::NoAnswer);
                }
                unsafe {
                    let _ = WaitNamedPipeW(&HSTRING::from(name), left.as_millis().clamp(1, 5000) as u32);
                }
            }
            // ERROR_FILE_NOT_FOUND: the app isn't running (yet).
            Err(e) if e.raw_os_error() == Some(2) => match started {
                None if start => {
                    start_app()?;
                    started = Some(Instant::now());
                }
                None => return Err(Failure::NotRunning),
                Some(at) if at.elapsed() > START_TIMEOUT => return Err(Failure::DidntStart),
                Some(_) => std::thread::sleep(Duration::from_millis(250)),
            },
            Err(e) => return Err(Failure::Io(e.to_string())),
        }
    }
}

/// Starts the app in the tray: through its startup task (no UAC prompt), or else with a UAC prompt.
fn start_app() -> Result<(), Failure> {
    let task = |args: &[&str]| Command::new("schtasks.exe").args(args).output().is_ok_and(|o| o.status.success());
    if task(&["/Query", "/TN", TASK]) && task(&["/Run", "/TN", TASK]) {
        return Ok(());
    }
    let exe: PathBuf = std::env::current_exe()
        .ok()
        .and_then(|me| me.parent().map(|dir| dir.join(APP_EXE)))
        .filter(|exe| exe.exists())
        .ok_or_else(|| Failure::CantStart(format!("can't find {APP_EXE} next to uucc")))?;
    let result = unsafe { ShellExecuteW(None, w!("runas"), &HSTRING::from(exe.as_os_str()), w!("--tray"), None, SW_SHOWNORMAL) };
    if result.0 as usize <= 32 {
        return Err(Failure::CantStart("it needs administrator rights, and they weren't given".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Envelope, Request, VERSION};
    use std::time::Duration;
    use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT};
    use windows::core::HSTRING;

    fn envelope() -> Envelope {
        Envelope { version: VERSION, request: Request::Status }
    }

    #[test]
    fn no_answer_times_out() {
        let name = format!(r"\\.\pipe\uucc-client-test-{}", std::process::id());
        let pipe = unsafe {
            CreateNamedPipeW(&HSTRING::from(name.as_str()), PIPE_ACCESS_DUPLEX, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT, 1, 4096, 4096, 0, None)
        };
        let pipe = pipe.0 as usize;
        // Accepts the client and never answers.
        std::thread::spawn(move || unsafe {
            let _ = ConnectNamedPipe(windows::Win32::Foundation::HANDLE(pipe as _), None);
            std::thread::sleep(Duration::from_secs(30));
        });
        std::thread::sleep(Duration::from_millis(100));
        let started = std::time::Instant::now();
        let result = send_with_timeout(&name, &envelope(), false, Duration::from_secs(1));
        assert!(matches!(result, Err(Failure::NoAnswer)), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_pipe_that_stays_busy_gives_up() {
        let name = format!(r"\\.\pipe\uucc-client-busy-{}", std::process::id());
        let pipe = unsafe {
            CreateNamedPipeW(&HSTRING::from(name.as_str()), PIPE_ACCESS_DUPLEX, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT, 1, 4096, 4096, 0, None)
        };
        let pipe = pipe.0 as usize;
        std::thread::spawn(move || unsafe {
            let _ = ConnectNamedPipe(windows::Win32::Foundation::HANDLE(pipe as _), None);
        });
        std::thread::sleep(Duration::from_millis(100));
        // The only instance is taken, and never freed.
        let _first = std::fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let name2 = name.clone();
        std::thread::spawn(move || {
            let _ = tx.send(send_with_timeout(&name2, &envelope(), false, Duration::from_secs(1)));
        });
        let result = rx.recv_timeout(Duration::from_secs(15)).expect("uucc kept waiting for a busy pipe");
        assert!(matches!(result, Err(Failure::NoAnswer)), "{result:?}");
    }

    #[test]
    fn missing_pipe_is_not_running() {
        let result = send_with_timeout(r"\\.\pipe\uucc-nobody-here", &envelope(), false, Duration::from_secs(1));
        assert!(matches!(result, Err(Failure::NotRunning)), "{result:?}");
        assert_eq!(Failure::NotRunning.code(), 2);
    }

    #[test]
    fn didnt_start_mentions_the_first_start_question() {
        assert!(Failure::DidntStart.message().contains("waiting for its first-start question"));
        assert_eq!(Failure::DidntStart.code(), 2);
        assert_eq!(Failure::NoAnswer.code(), 3);
    }
}
