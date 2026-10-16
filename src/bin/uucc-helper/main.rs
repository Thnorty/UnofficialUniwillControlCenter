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
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};
    use std::time::{Duration, Instant};

    use crate::ec::{self, EcAccess, EcWindow};
    use crate::protocol::{self, Envelope, Reply, Unavailable};
    use crate::rules::Releases;
    use crate::server::{self, Session};
    use crate::{efivar, polkit};

    /// Connections waiting for polkit's answer at once; more are closed at once, so nobody can tie the helper up.
    const MAX_PENDING: usize = 16;
    /// How long a refused client has to send its request before it's told no.
    const REFUSE_WAIT: Duration = Duration::from_secs(2);

    /// What every client shares: the EC (one request at a time), how many authorized clients are connected since
    /// when, how many connections wait for authorization, and what each connected client wants undone (for when the
    /// helper itself is stopped). `sessions` is only locked while holding `ec`.
    struct Shared {
        ec: Mutex<Option<Box<dyn EcAccess>>>,
        status: Result<u64, Unavailable>,
        clients: Mutex<(usize, Instant)>,
        pending: Mutex<usize>,
        sessions: Mutex<BTreeMap<u64, Releases>>,
        next_session: AtomicU64,
    }

    impl Shared {
        fn new(ec: Option<Box<dyn EcAccess>>, status: Result<u64, Unavailable>) -> Shared {
            Shared {
                ec: Mutex::new(ec),
                status,
                clients: Mutex::new((0, Instant::now())),
                pending: Mutex::new(0),
                sessions: Mutex::new(BTreeMap::new()),
                next_session: AtomicU64::new(0),
            }
        }
    }

    /// A connection waiting for authorization, counted against `MAX_PENDING` until dropped.
    struct Pending(Arc<Shared>);

    impl Pending {
        fn admit(shared: &Arc<Shared>) -> Option<Pending> {
            let mut pending = shared.pending.lock().unwrap_or_else(|e| e.into_inner());
            if *pending >= MAX_PENDING {
                return None;
            }
            *pending += 1;
            Some(Pending(shared.clone()))
        }
    }

    impl Drop for Pending {
        fn drop(&mut self) {
            *self.0.pending.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        }
    }

    /// An authorized client, counted for the idle exit until dropped (also when its thread panics).
    struct Counted(Arc<Shared>);

    impl Counted {
        fn new(shared: &Arc<Shared>) -> Counted {
            shared.clients.lock().unwrap_or_else(|e| e.into_inner()).0 += 1;
            Counted(shared.clone())
        }
    }

    impl Drop for Counted {
        fn drop(&mut self) {
            let mut clients = self.0.clients.lock().unwrap_or_else(|e| e.into_inner());
            *clients = (clients.0 - 1, Instant::now());
        }
    }

    pub fn run() {
        // `systemctl stop`, and with it an uninstall or an upgrade, ends the helper with SIGTERM: what the connected
        // app asked to have undone is undone first. The signals are blocked before anything else, so one that comes
        // early waits for the thread that takes them.
        let signals = stop_signals();
        unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &signals, std::ptr::null_mut()) };
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
        let shared = Arc::new(Shared::new(window, status));

        let stopping = shared.clone();
        std::thread::spawn(move || {
            let mut signal = 0;
            unsafe { libc::sigwait(&signals, &mut signal) };
            let _ec = release_all(&stopping);
            std::process::exit(0);
        });

        let watcher = shared.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(5));
            let clients = watcher.clients.lock().unwrap_or_else(|e| e.into_inner());
            if server::idle_should_exit(clients.0, clients.1.elapsed()) {
                std::process::exit(0);
            }
        });

        serve_all(listener, shared, authorized);
    }

    /// SIGTERM and SIGINT.
    fn stop_signals() -> libc::sigset_t {
        let mut signals: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::sigemptyset(&mut signals);
            libc::sigaddset(&mut signals, libc::SIGTERM);
            libc::sigaddset(&mut signals, libc::SIGINT);
        }
        signals
    }

    /// Undoes what every connected client asked to have undone, and keeps the EC locked so no request comes after.
    fn release_all(shared: &Shared) -> MutexGuard<'_, Option<Box<dyn EcAccess>>> {
        let mut ec = shared.ec.lock().unwrap_or_else(|e| e.into_inner());
        let sessions = std::mem::take(&mut *shared.sessions.lock().unwrap_or_else(|e| e.into_inner()));
        if let Some(ec) = ec.as_deref_mut() {
            for (_, mut releases) in sessions {
                server::apply(&mut releases, ec);
            }
        }
        ec
    }

    /// Whether polkit lets the connected process use the helper. Asked as soon as it connects, before anything is
    /// read from it, so its pid and start time are those of the process that connected.
    fn authorized(stream: &UnixStream) -> bool {
        peer(stream).is_ok_and(|(pid, uid)| polkit::authorized(pid, uid))
    }

    /// Accepts clients, each on a thread of its own: `authorize` first, then its requests.
    fn serve_all(listener: UnixListener, shared: Arc<Shared>, authorize: fn(&UnixStream) -> bool) {
        for stream in listener.incoming() {
            let stream = match stream {
                Ok(stream) => stream,
                Err(_) => {
                    // Out of file descriptors, say: wait instead of spinning.
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };
            // Too many waiting for authorization: this one is closed.
            let Some(pending) = Pending::admit(&shared) else { continue };
            let shared = shared.clone();
            let spawned = std::thread::Builder::new().spawn(move || client(stream, &shared, authorize, pending));
            if spawned.is_err() {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    fn client(stream: UnixStream, shared: &Arc<Shared>, authorize: fn(&UnixStream) -> bool, pending: Pending) {
        let allowed = authorize(&stream);
        drop(pending);
        if !allowed {
            refuse(&stream);
            return;
        }
        let _counted = Counted::new(shared);
        if let Err(error) = serve(&stream, shared) {
            eprintln!("A client's connection ended: {error}");
        }
    }

    /// Tells a client it isn't allowed, once it has sent its request (or after `REFUSE_WAIT`), so it reads the answer
    /// instead of finding the connection closed.
    fn refuse(stream: &UnixStream) {
        let Ok(reader) = stream.try_clone() else { return };
        if stream.set_read_timeout(Some(REFUSE_WAIT)).is_ok() {
            let _ = protocol::read_line::<Envelope>(&mut BufReader::new(reader));
        }
        let message = "The helper only answers users signed in at this laptop.".into();
        let _ = protocol::write_line(&mut &*stream, &Reply::Refused { message });
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
        let id = shared.next_session.fetch_add(1, Ordering::Relaxed);
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
            let reply = {
                let mut ec = shared.ec.lock().unwrap_or_else(|e| e.into_inner());
                let ec = ec.as_mut().map(|ec| -> &mut dyn EcAccess { ec.as_mut() });
                let reply = server::handle(envelope.request, &mut session, ec, &shared.status, Path::new(efivar::VARIABLE));
                shared.sessions.lock().unwrap_or_else(|e| e.into_inner()).insert(id, session.releases().clone());
                reply
            };
            if let Err(error) = protocol::write_line(&mut writer, &reply) {
                break Err(error);
            }
        };
        let mut ec = shared.ec.lock().unwrap_or_else(|e| e.into_inner());
        shared.sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        if let Some(ec) = ec.as_deref_mut() {
            server::release(&mut session, ec);
        }
        result
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::BufRead;

        fn shared() -> Arc<Shared> {
            Arc::new(Shared::new(None, Err(Unavailable::SecureBoot)))
        }

        /// Serves `shared` on a temporary socket, with `authorize` in place of polkit.
        fn start(name: &str, shared: &Arc<Shared>, authorize: fn(&UnixStream) -> bool) -> std::path::PathBuf {
            let path = std::env::temp_dir().join(format!("uucc-helper-main-test-{}-{name}.sock", std::process::id()));
            let _ = std::fs::remove_file(&path);
            let listener = UnixListener::bind(&path).unwrap();
            let shared = shared.clone();
            std::thread::spawn(move || serve_all(listener, shared, authorize));
            path
        }

        fn ask_status(stream: &mut UnixStream) -> Reply {
            ask(stream, protocol::Request::Status)
        }

        fn ask(stream: &mut UnixStream, request: protocol::Request) -> Reply {
            let envelope = Envelope { version: protocol::VERSION, request };
            protocol::write_line(stream, &envelope).unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
            serde_json::from_str(&line).unwrap()
        }

        fn clients(shared: &Shared) -> usize {
            shared.clients.lock().unwrap().0
        }

        #[test]
        fn unauthorized_clients_are_refused_and_not_counted() {
            let shared = shared();
            let path = start("refused", &shared, |_| false);
            // Clients that connect and send nothing: they hold no count, so the helper can still idle out.
            let silent: Vec<UnixStream> = (0..5).map(|_| UnixStream::connect(&path).unwrap()).collect();
            let mut asking = UnixStream::connect(&path).unwrap();
            let reply = ask_status(&mut asking);
            assert!(matches!(reply, Reply::Refused { .. }), "{reply:?}");
            assert_eq!(clients(&shared), 0);
            drop(silent);
        }

        #[test]
        fn authorized_client_is_served_and_counted() {
            let shared = shared();
            let path = start("served", &shared, |_| true);
            let mut client = UnixStream::connect(&path).unwrap();
            assert_eq!(ask_status(&mut client), Reply::Unavailable { reason: Unavailable::SecureBoot });
            assert_eq!(clients(&shared), 1);
            drop(client);
            let started = Instant::now();
            while clients(&shared) != 0 && started.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(clients(&shared), 0);
        }

        #[test]
        fn stopping_undoes_what_connected_clients_registered() {
            use protocol::Request;
            let shared = Arc::new(Shared::new(Some(Box::new(crate::ec::FakeEc([0; 0x1000]))), Ok(0xFE41_0000)));
            let path = start("stopping", &shared, |_| true);
            let mut client = UnixStream::connect(&path).unwrap();
            assert_eq!(ask(&mut client, Request::OnDisconnect { address: 0x7C6, mask: 0x04, value: 0 }), Reply::Done);
            assert_eq!(ask(&mut client, Request::Update { address: 0x7C6, mask: 0x04, value: 0x04 }), Reply::Done);
            // `systemctl stop`, an uninstall or an upgrade while the app is still connected: the custom fan curve
            // mustn't stay on with nothing watching the EC's safety protection.
            let mut ec = release_all(&shared);
            assert_eq!(ec.as_mut().unwrap().read(0x7C6) & 0x04, 0);
            drop(ec);
            drop(client);
        }

        #[test]
        fn connections_waiting_for_authorization_are_capped() {
            let shared = shared();
            let waiting: Vec<Pending> = (0..MAX_PENDING).map(|_| Pending::admit(&shared).expect("room")).collect();
            assert!(Pending::admit(&shared).is_none());
            drop(waiting);
            assert!(Pending::admit(&shared).is_some());
        }

        #[test]
        fn optional_paths_dont_stop_the_helper() {
            // systemd won't start a service whose ReadWritePaths entry is missing (no efivarfs on a legacy boot)
            // unless the entry starts with "-".
            let unit = include_str!("../../../packaging/linux/uucc-helper.service");
            let paths: Vec<&str> = unit.lines().filter_map(|l| l.strip_prefix("ReadWritePaths=")).flat_map(str::split_whitespace).collect();
            assert!(!paths.is_empty());
            assert!(paths.iter().all(|p| p.starts_with('-')), "{paths:?}");
        }
    }
}
