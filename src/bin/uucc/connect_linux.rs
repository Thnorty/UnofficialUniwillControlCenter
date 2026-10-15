//! Reaching the running app over its socket on Linux, and starting it when asked to.

use std::fs::File;
use std::io::ErrorKind;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::protocol::{APP_NAME, Envelope, Reply, read_message, runtime_folder, socket_path, write_message};

/// How long the app may take to answer; some commands wait for the hardware.
const READ_TIMEOUT: Duration = Duration::from_secs(20);
/// How long `--start` waits for the app's socket.
const START_TIMEOUT: Duration = Duration::from_secs(15);
/// The app's program, as the packages install it; a build from source names it after the crate.
const APP_PROGRAMS: [&str; 2] = ["unofficial-uniwill-control-center", "UnofficialUniwillControlCenter"];

#[derive(Debug)]
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

/// The lock the running app holds (the app's `shell::instance` takes it).
fn app_lock_path() -> PathBuf {
    runtime_folder().join(format!("{APP_NAME}.lock"))
}

/// Sends `envelope` to the app (the demo's when `demo`) and returns its reply. With `start`, starts the app first if
/// it isn't running.
pub fn send(envelope: &Envelope, demo: bool, start: bool) -> Result<Reply, Failure> {
    let lock = app_lock_path();
    send_with_timeout(&socket_path(demo), (!demo).then_some(lock.as_path()), envelope, start && !demo, READ_TIMEOUT)
}

/// `lock`: the app's single-instance lock, which tells a running app without its socket from one that isn't running.
pub fn send_with_timeout(socket: &Path, lock: Option<&Path>, envelope: &Envelope, start: bool, read_timeout: Duration) -> Result<Reply, Failure> {
    let mut stream = open(socket, lock, start)?;
    stream.set_read_timeout(Some(read_timeout)).map_err(|e| Failure::Io(e.to_string()))?;
    write_message(&mut stream, envelope).and_then(|()| read_message::<Reply>(&mut stream)).map_err(|e| match e.kind() {
        ErrorKind::WouldBlock | ErrorKind::TimedOut => Failure::NoAnswer,
        _ => Failure::Io(e.to_string()),
    })
}

/// Connects; with `start`, starts the app (or waits for a running one's socket) first.
fn open(socket: &Path, lock: Option<&Path>, start: bool) -> Result<UnixStream, Failure> {
    // When this started the app, or found it running without its socket and waits for it (with --start).
    let mut started: Option<Instant> = None;
    let mut started_here = false;
    loop {
        match UnixStream::connect(socket) {
            Ok(stream) => return Ok(stream),
            // No socket, or one nobody answers on: the app isn't running (yet).
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => match started {
                None if lock.is_some_and(app_running) => {
                    if !start {
                        return Err(Failure::NoPipe);
                    }
                    started = Some(Instant::now());
                }
                None if start => {
                    start_app()?;
                    started = Some(Instant::now());
                    started_here = true;
                }
                None => return Err(Failure::NotRunning),
                Some(at) if at.elapsed() > START_TIMEOUT => return Err(if started_here { Failure::DidntStart } else { Failure::NoPipe }),
                Some(_) => std::thread::sleep(Duration::from_millis(250)),
            },
            Err(e) => return Err(Failure::Io(e.to_string())),
        }
    }
}

/// Whether the app holds its single-instance lock: a shared lock on the file can't be had then.
fn app_running(lock: &Path) -> bool {
    let Ok(file) = File::open(lock) else { return false };
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) != 0 }
}

/// Starts the app in the tray: the program next to uucc, or else the one on PATH. It gets its own process group, so
/// closing the terminal doesn't end it.
fn start_app() -> Result<(), Failure> {
    let beside: Option<PathBuf> = std::env::current_exe()
        .ok()
        .and_then(|me| me.parent().map(Path::to_path_buf))
        .and_then(|dir| APP_PROGRAMS.iter().map(|name| dir.join(name)).find(|exe| exe.exists()));
    let program = beside.unwrap_or_else(|| PathBuf::from(APP_PROGRAMS[0]));
    Command::new(&program)
        .arg("--tray")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(drop)
        .map_err(|e| Failure::CantStart(format!("{}: {e}", program.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Request;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::Duration;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("uucc-socket-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn envelope() -> Envelope {
        Envelope { version: crate::protocol::VERSION, request: Request::Status }
    }

    fn echo(envelope: Envelope) -> Reply {
        Reply::Ok(serde_json::json!({ "asked": format!("{:?}", envelope.request) }))
    }

    #[test]
    fn round_trip_over_the_socket() {
        let socket = folder("round-trip").join("uucc.sock");
        crate::server::start(&socket, echo).unwrap();
        let reply = send_with_timeout(&socket, None, &envelope(), false, Duration::from_secs(2)).unwrap();
        assert_eq!(reply, Reply::Ok(serde_json::json!({ "asked": "Status" })));
        // One client at a time, but each gets its turn.
        assert!(send_with_timeout(&socket, None, &envelope(), false, Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn socket_is_private() {
        let socket = folder("private").join("uucc.sock");
        crate::server::start(&socket, echo).unwrap();
        assert_eq!(std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn not_running_without_lock() {
        let dir = folder("not-running");
        let (socket, lock) = (dir.join("uucc.sock"), dir.join("app.lock"));
        let result = send_with_timeout(&socket, Some(&lock), &envelope(), false, Duration::from_secs(1));
        assert!(matches!(result, Err(Failure::NotRunning)), "{result:?}");
        // A lock file nobody holds is no app either.
        std::fs::write(&lock, "").unwrap();
        let result = send_with_timeout(&socket, Some(&lock), &envelope(), false, Duration::from_secs(1));
        assert!(matches!(result, Err(Failure::NotRunning)), "{result:?}");
        // Held, but no socket: the app runs without its connection for uucc.
        let held = std::fs::File::open(&lock).unwrap();
        assert_eq!(unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
        let result = send_with_timeout(&socket, Some(&lock), &envelope(), false, Duration::from_secs(1));
        assert!(matches!(result, Err(Failure::NoPipe)), "{result:?}");
        assert!(Failure::NoPipe.message().contains("~/.config/UnofficialUniwillControlCenter/uucc.log"));
        assert!(Failure::DidntStart.message().contains("from your app menu"));
    }

    #[test]
    fn stale_socket_is_replaced() {
        let socket = folder("stale").join("uucc.sock");
        drop(std::os::unix::net::UnixListener::bind(&socket).unwrap()); // left behind by an app that crashed
        assert!(socket.exists());
        crate::server::start(&socket, echo).unwrap();
        assert!(send_with_timeout(&socket, None, &envelope(), false, Duration::from_secs(2)).is_ok());
        // A socket someone answers on isn't taken over.
        assert!(crate::server::start(&socket, echo).is_err());
    }
}
