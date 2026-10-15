//! One instance at a time on Linux: two would send interleaved commands to the same controllers. The running
//! instance holds a lock file in the runtime folder and listens on a socket beside it; a second launch asks it to
//! show its window, unless it's on its way out (it holds the lock for a few seconds while it hands the Fn keys and
//! fans back), in which case the new one waits for it and then starts.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const NAME: &str = "unofficial-uniwill-control-center";
const EXIT_WAIT: Duration = Duration::from_secs(15);

/// What a later launch asks the running instance to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Show,
    Exit,
}

static EXITING: AtomicBool = AtomicBool::new(false);
/// The lock, held until the process ends.
static LOCK: OnceLock<File> = OnceLock::new();
static CHANNEL: OnceLock<(Sender<Control>, Mutex<Option<Receiver<Control>>>)> = OnceLock::new();

fn channel() -> &'static (Sender<Control>, Mutex<Option<Receiver<Control>>>) {
    CHANNEL.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        (sender, Mutex::new(Some(receiver)))
    })
}

/// What later launches asked for; the tray takes it once.
pub fn control_messages() -> Receiver<Control> {
    channel().1.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_else(|| mpsc::channel().1)
}

/// `$XDG_RUNTIME_DIR`, which is per user and cleared at sign-out; the temporary folder without one.
fn runtime_folder() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join(format!("{NAME}-{}", unsafe { libc::getuid() })))
}

fn paths(folder: &Path) -> (PathBuf, PathBuf) {
    (folder.join(format!("{NAME}.lock")), folder.join(format!("{NAME}.sock")))
}

/// Takes the lock without waiting; None if another process has it.
fn try_lock(path: &Path) -> Option<File> {
    let file = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(path).ok()?;
    (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(file)
}

/// Takes the lock within `wait`.
fn lock_within(path: &Path, wait: Duration) -> Option<File> {
    let started = Instant::now();
    loop {
        if let Some(file) = try_lock(path) {
            return Some(file);
        }
        if started.elapsed() >= wait {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Sends a word to the running instance and returns its one-line answer.
fn tell(socket: &Path, word: &str) -> Option<String> {
    // Right after it took the lock, the running instance may not be listening yet.
    for _ in 0..20 {
        if let Ok(mut stream) = UnixStream::connect(socket) {
            stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
            writeln!(stream, "{word}").ok()?;
            let mut answer = String::new();
            BufReader::new(stream).read_line(&mut answer).ok()?;
            return Some(answer.trim().to_string());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// Answers later launches on the socket beside the lock.
fn listen(socket: &Path, messages: Sender<Control>) {
    let _ = std::fs::remove_file(socket);
    let Ok(listener) = UnixListener::bind(socket) else { return };
    std::thread::Builder::new()
        .name("Instance".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_err() {
                    continue;
                }
                let control = match line.trim() {
                    "show" => Control::Show,
                    "exit" => Control::Exit,
                    _ => continue,
                };
                let answer = if EXITING.load(Ordering::Relaxed) { "exiting" } else { "ok" };
                let _ = writeln!(&stream, "{answer}");
                if !(control == Control::Show && answer == "exiting") {
                    let _ = messages.send(control);
                }
            }
        })
        .ok();
}

/// The lock in `folder`, with the socket listening; or None after the running instance was asked to show itself.
fn acquire_in(folder: &Path, messages: Sender<Control>) -> Option<File> {
    let _ = std::fs::create_dir_all(folder);
    let (lock_path, socket) = paths(folder);
    let lock = match try_lock(&lock_path) {
        Some(lock) => lock,
        None => match tell(&socket, "show").as_deref() {
            // On its way out: wait for it, then start.
            Some("exiting") => lock_within(&lock_path, EXIT_WAIT)?,
            _ => return None,
        },
    };
    listen(&socket, messages);
    Some(lock)
}

/// Takes the single-instance lock, or shows the running instance and returns false.
pub fn acquire() -> bool {
    match acquire_in(&runtime_folder(), channel().0.clone()) {
        Some(lock) => {
            let _ = LOCK.set(lock);
            true
        }
        None => false,
    }
}

/// The app runs as the user on Linux; the helper does what needs root.
pub fn is_elevated() -> bool {
    true
}

/// Never needed on Linux.
pub fn open_elevated(_arguments: &[String]) {}

fn close_running_in(folder: &Path, wait: Duration) -> bool {
    let (lock_path, socket) = paths(folder);
    if try_lock(&lock_path).is_some() {
        return true;
    }
    tell(&socket, "exit");
    lock_within(&lock_path, wait).is_some()
}

/// Asks the running instance, if any, to quit, and waits until it has handed everything back and ended. False if it
/// was still running after `wait`.
pub fn close_running(wait: Duration) -> bool {
    close_running_in(&runtime_folder(), wait)
}

/// Tells later launches that this instance is on its way out.
pub fn note_exiting() {
    EXITING.store(true, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("uucc-instance-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn second_acquire_signals_show() {
        let dir = folder("show");
        let (first, messages) = mpsc::channel();
        let lock = acquire_in(&dir, first);
        assert!(lock.is_some());
        let (second, _) = mpsc::channel();
        assert!(acquire_in(&dir, second).is_none());
        assert_eq!(messages.recv_timeout(Duration::from_secs(2)), Ok(Control::Show));
        drop(lock);
    }

    #[test]
    fn close_running_sends_exit() {
        let dir = folder("exit");
        assert!(close_running_in(&dir, Duration::from_secs(1)), "nothing running counts as closed");
        let (sender, messages) = mpsc::channel();
        let lock = acquire_in(&dir, sender).unwrap();
        // The running instance: quits (lets go of the lock) when told to.
        std::thread::spawn(move || {
            if messages.recv() == Ok(Control::Exit) {
                drop(lock);
            }
        });
        assert!(close_running_in(&dir, Duration::from_secs(5)));
    }
}
