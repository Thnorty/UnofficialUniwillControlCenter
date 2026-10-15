//! `uucc-helper`: the small root service the Linux app reaches the embedded controller and the BIOS settings variable
//! through. It does nothing else, and only for users signed in at the laptop.

#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../../hardware/helper_protocol.rs"]
mod protocol;
#[cfg(target_os = "linux")]
mod ec;
#[cfg(target_os = "linux")]
mod efivar;
#[cfg(target_os = "linux")]
mod polkit;
#[cfg(target_os = "linux")]
mod rules;
#[cfg(target_os = "linux")]
mod server;

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
fn main() {
    linux::run()
}

#[cfg(target_os = "linux")]
mod linux {
    use std::io::{self, BufReader};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use crate::ec::{self, EcAccess, EcWindow};
    use crate::protocol::{self, Envelope, Reply, Unavailable};
    use crate::server::{self, Session};
    use crate::{efivar, polkit};

    /// What every client shares: the EC (one request at a time) and how many clients are connected since when.
    struct Shared {
        ec: Mutex<Option<Box<dyn EcAccess>>>,
        status: Result<u64, Unavailable>,
        clients: Mutex<(usize, Instant)>,
    }

    pub fn run() {
        let mut status = ec::check(&ec::Paths::system());
        let window = match status {
            Ok(base) => match EcWindow::map(base) {
                Ok(window) => Some(Box::new(window) as Box<dyn EcAccess>),
                Err(error) => {
                    status = Err(Unavailable::MapFailed { error: error.to_string() });
                    None
                }
            },
            Err(_) => None,
        };
        if let Err(reason) = &status {
            eprintln!("The embedded controller can't be used: {}", reason.message());
        }
        let listener = match listener() {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("Couldn't open the helper's socket: {error}");
                std::process::exit(1);
            }
        };
        let shared = Arc::new(Shared { ec: Mutex::new(window), status, clients: Mutex::new((0, Instant::now())) });

        let watcher = shared.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(5));
            let clients = watcher.clients.lock().unwrap();
            if server::idle_should_exit(clients.0, clients.1.elapsed()) {
                std::process::exit(0);
            }
        });

        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            {
                let mut clients = shared.clients.lock().unwrap();
                clients.0 += 1;
            }
            let shared = shared.clone();
            std::thread::spawn(move || {
                if let Err(error) = serve(&stream, &shared) {
                    eprintln!("A client's connection ended: {error}");
                }
                let mut clients = shared.clients.lock().unwrap();
                *clients = (clients.0 - 1, Instant::now());
            });
        }
    }

    /// The socket systemd passes in (fd 3, when LISTEN_PID is this process), or, for development, one bound at
    /// `UUCC_HELPER_SOCKET` (by default the usual path).
    fn listener() -> io::Result<UnixListener> {
        let ours = std::env::var("LISTEN_PID").ok().and_then(|p| p.parse::<u32>().ok()) == Some(std::process::id());
        if ours && std::env::var("LISTEN_FDS").as_deref() == Ok("1") {
            return Ok(unsafe { UnixListener::from_raw_fd(3) });
        }
        let path = std::env::var("UUCC_HELPER_SOCKET").unwrap_or_else(|_| protocol::SOCKET.into());
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666))?;
        Ok(listener)
    }

    /// The connected process's pid and uid.
    fn peer(stream: &UnixStream) -> io::Result<(u32, u32)> {
        let mut credentials = libc::ucred { pid: 0, uid: 0, gid: 0 };
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut credentials as *mut libc::ucred as *mut libc::c_void,
                &mut length,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((credentials.pid as u32, credentials.uid))
    }

    /// Answers one client until it goes away, then undoes what it asked to have undone.
    fn serve(stream: &UnixStream, shared: &Shared) -> io::Result<()> {
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut writer = stream.try_clone()?;
        let mut session = Session::default();
        let mut checked = false;
        let result = loop {
            let envelope: Envelope = match protocol::read_line(&mut reader) {
                Ok(Some(envelope)) => envelope,
                Ok(None) => break Ok(()),
                Err(error) => {
                    let _ = protocol::write_line(&mut writer, &Reply::Error { message: error.to_string() });
                    break Err(error);
                }
            };
            if envelope.version != protocol::VERSION {
                protocol::write_line(&mut writer, &Reply::WrongVersion { helper: protocol::VERSION })?;
                break Ok(());
            }
            if !checked {
                let allowed = peer(stream).is_ok_and(|(pid, uid)| polkit::authorized(pid, uid));
                if !allowed {
                    let message = "The helper only answers users signed in at this laptop.".into();
                    protocol::write_line(&mut writer, &Reply::Refused { message })?;
                    break Ok(());
                }
                checked = true;
            }
            let reply = {
                let mut ec = shared.ec.lock().unwrap();
                let ec = ec.as_mut().map(|ec| -> &mut dyn EcAccess { ec.as_mut() });
                server::handle(envelope.request, &mut session, ec, &shared.status, Path::new(efivar::VARIABLE))
            };
            if let Err(error) = protocol::write_line(&mut writer, &reply) {
                break Err(error);
            }
        };
        if let Some(ec) = shared.ec.lock().unwrap().as_deref_mut() {
            server::release(&mut session, ec);
        }
        result
    }
}
