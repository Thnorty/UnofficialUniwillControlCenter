//! The embedded controller's RAM on Linux, reached through the app's root helper (uucc-helper) over its socket. The
//! helper does each request whole, including an update's read-modify-write, and undoes what the app asked it to when
//! the app goes away (a crash included).
//!
//! The app keeps one connection to the helper. A connection that breaks (the helper restarted) is opened again once
//! per request, and the undo actions registered so far are sent again before the request.

use std::collections::BTreeMap;
use std::io::{self, BufReader};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};

use super::helper_protocol::{self as protocol, Envelope, Reply, Request};

/// Why the EC can't be used at all, in words for the person: Secure Boot, a missing or different helper, and so on.
#[derive(Debug, Clone, PartialEq)]
pub struct EcUnavailable(pub String);

impl std::fmt::Display for EcUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EcUnavailable {}

const NOT_RUNNING: &str = "The app's helper (uucc-helper) isn't installed or running: reinstall the package, or run \
    `sudo systemctl enable --now uucc-helper.socket`.";
const OTHER_VERSION: &str = "The app and its helper are different versions: reinstall the package.";
/// How long the app waits for one answer from the helper.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5);

/// The connection to the helper, and the undo actions it holds for this app.
#[derive(Default)]
struct Link {
    connection: Option<(PathBuf, BufReader<UnixStream>, UnixStream)>,
    releases: BTreeMap<u16, (u8, u8)>,
}

static LINK: Mutex<Option<Link>> = Mutex::new(None);

/// How many connections to the helper have been opened.
static OPENED: AtomicU64 = AtomicU64::new(0);

/// How many connections to the helper have been opened. A new one after the first means the helper restarted, and
/// it undid the app's settings as the old connection broke.
pub fn connections_opened() -> u64 {
    OPENED.load(Ordering::Acquire)
}

fn socket() -> PathBuf {
    std::env::var_os("UUCC_HELPER_SOCKET").map(PathBuf::from).unwrap_or_else(|| protocol::SOCKET.into())
}

impl Link {
    /// Sends one request and reads its answer on the open connection, opening one if needed.
    fn exchange(&mut self, request: &Request) -> io::Result<Reply> {
        let path = socket();
        if self.connection.as_ref().is_some_and(|(open, _, _)| *open != path) {
            // Another helper (only in tests): what the old one holds isn't this one's.
            *self = Link::default();
        }
        if self.connection.is_none() {
            let stream = UnixStream::connect(&path)?;
            stream.set_read_timeout(Some(ANSWER_TIMEOUT))?;
            self.connection = Some((path, BufReader::new(stream.try_clone()?), stream));
            OPENED.fetch_add(1, Ordering::AcqRel);
            for (&address, &(mask, value)) in &self.releases.clone() {
                self.send(&Request::OnDisconnect { address, mask, value })?;
            }
        }
        self.send(request)
    }

    fn send(&mut self, request: &Request) -> io::Result<Reply> {
        let (_, reader, writer) = self.connection.as_mut().expect("connected");
        let result = protocol::write_line(writer, &Envelope { version: protocol::VERSION, request: request.clone() })
            .and_then(|()| protocol::read_line(reader))
            .and_then(|reply| reply.ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "the helper went away")));
        if result.is_err() {
            self.connection = None;
        }
        result
    }
}

/// Whether the EC may become reachable without a restart, so connecting is worth trying again: the helper wasn't
/// running, or was another version (being updated). Secure Boot, a missing EC or a driver in charge need a restart.
pub fn may_come_back(reason: &str) -> bool {
    reason.contains(NOT_RUNNING) || reason.contains(OTHER_VERSION)
}

/// Sends `request` to the helper and returns its answer; a broken connection is opened again once. A helper that
/// can't be reached or speaks another version gives an `EcUnavailable` error.
pub(crate) fn request(request: Request) -> Result<Reply> {
    let mut link = LINK.lock().unwrap_or_else(|e| e.into_inner());
    let link = link.get_or_insert_with(Link::default);
    let reply = link.exchange(&request).or_else(|_| link.exchange(&request));
    let reply = match reply {
        Ok(reply) => reply,
        Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {
            return Err(EcUnavailable(NOT_RUNNING.into()).into());
        }
        Err(error) => bail!("The app's helper didn't answer: {error}"),
    };
    // Keep what the helper now holds, for a reconnection.
    match (&request, &reply) {
        (Request::OnDisconnect { address, mask, value }, Reply::Done) => {
            link.releases.insert(*address, (*mask, *value));
        }
        (Request::ClearOnDisconnect { address }, Reply::Done) => {
            link.releases.remove(address);
        }
        _ => {}
    }
    match reply {
        Reply::WrongVersion { .. } => Err(EcUnavailable(OTHER_VERSION.into()).into()),
        reply => Ok(reply),
    }
}

/// The laptop's hotkey codes, as the helper hears them; ends when the helper's connection does (it stopped).
pub struct HotkeyStream {
    reader: BufReader<UnixStream>,
}

impl Iterator for HotkeyStream {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        match protocol::read_line::<Reply>(&mut self.reader) {
            Ok(Some(Reply::Hotkey { code })) => Some(code as u32),
            _ => None,
        }
    }
}

/// Subscribes to the hotkeys on a connection of its own, which then carries only their codes.
pub fn subscribe_hotkeys() -> Result<HotkeyStream> {
    let stream = match UnixStream::connect(socket()) {
        Ok(stream) => stream,
        Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {
            return Err(EcUnavailable(NOT_RUNNING.into()).into());
        }
        Err(error) => bail!("The app's helper didn't answer: {error}"),
    };
    stream.set_read_timeout(Some(ANSWER_TIMEOUT))?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream.try_clone()?);
    protocol::write_line(&mut writer, &Envelope { version: protocol::VERSION, request: Request::Hotkeys })?;
    match protocol::read_line::<Reply>(&mut reader)? {
        Some(Reply::Done) => {
            stream.set_read_timeout(None)?; // the next code may be hours away
            Ok(HotkeyStream { reader })
        }
        Some(Reply::WrongVersion { .. }) => Err(EcUnavailable(OTHER_VERSION.into()).into()),
        Some(other) => Err(unexpected(other)),
        None => bail!("The app's helper went away"),
    }
}

/// A reply that isn't the answer expected, as an error.
fn unexpected(reply: Reply) -> anyhow::Error {
    match reply {
        Reply::Unavailable { reason } => EcUnavailable(reason.message()).into(),
        Reply::Refused { message } | Reply::Error { message } => anyhow!(message),
        other => anyhow!("The app's helper answered {other:?}"),
    }
}

type Job = Box<dyn FnOnce(&Registers) + Send>;

/// A handle to the EC thread; clones share it. The thread ends when the last handle is dropped.
#[derive(Clone)]
pub struct Ec {
    jobs: Sender<Job>,
}

impl Ec {
    /// Checks that the helper can reach the EC and starts the EC thread.
    pub fn connect() -> Result<Ec> {
        match request(Request::Status)? {
            Reply::Ready => {}
            Reply::Refused { message } => return Err(EcUnavailable(message).into()),
            other => return Err(unexpected(other)),
        }
        let (jobs, receiver) = mpsc::channel::<Job>();
        std::thread::Builder::new().name("EC".into()).spawn(move || {
            for job in receiver {
                job(&Registers { _private: () });
            }
        })?;
        Ok(Ec { jobs })
    }

    /// Runs `job` on the EC thread and waits at most `timeout` for its result.
    pub fn run_for<R: Send + 'static>(&self, timeout: Duration, job: impl FnOnce(&Registers) -> Result<R> + Send + 'static) -> Result<R> {
        let (tx, rx) = mpsc::channel();
        self.jobs
            .send(Box::new(move |registers| {
                tx.send(job(registers)).ok();
            }))
            .map_err(|_| anyhow!("The EC thread stopped"))?;
        rx.recv_timeout(timeout).map_err(|_| anyhow!("The EC didn't answer"))?
    }

    /// Runs `job` on the EC thread without waiting.
    pub fn spawn(&self, job: impl FnOnce(&Registers) + Send + 'static) {
        self.jobs.send(Box::new(job)).ok();
    }
}

/// The EC's RAM, on the EC thread.
pub struct Registers {
    _private: (),
}

impl Registers {
    pub fn read(&self, address: u16) -> Result<u8> {
        match request(Request::Read { address })? {
            Reply::Byte { value } => Ok(value),
            other => Err(unexpected(other)),
        }
    }

    /// Several registers in one request.
    #[allow(dead_code)] // the helper offers it; nothing reads tables in one go yet
    pub fn read_many(&self, addresses: &[u16]) -> Result<Vec<u8>> {
        match request(Request::ReadMany { addresses: addresses.to_vec() })? {
            Reply::Bytes { values } => Ok(values),
            other => Err(unexpected(other)),
        }
    }

    pub fn write(&self, address: u16, value: u8) -> Result<()> {
        done(request(Request::Write { address, value })?)
    }

    /// Sets the bits in `mask` to those of `value`, leaving the others; the helper does it as one step.
    pub fn update(&self, address: u16, mask: u8, value: u8) -> Result<()> {
        done(request(Request::Update { address, mask, value })?)
    }

    pub fn bit(&self, address: u16, bit: u8) -> Result<bool> {
        Ok(self.read(address)? & (1 << bit) != 0)
    }

    /// Whether the helper is still there and can reach the EC.
    pub fn check(&self) -> Result<()> {
        match request(Request::Status)? {
            Reply::Ready => Ok(()),
            Reply::Refused { message } => Err(EcUnavailable(message).into()),
            other => Err(unexpected(other)),
        }
    }

    /// Has the helper set the bits in `mask` of `address` to those of `value` when the app goes away.
    pub fn on_disconnect(&self, address: u16, mask: u8, value: u8) -> Result<()> {
        done(request(Request::OnDisconnect { address, mask, value })?)
    }

    /// Takes back `on_disconnect` for `address`.
    pub fn clear_on_disconnect(&self, address: u16) -> Result<()> {
        done(request(Request::ClearOnDisconnect { address })?)
    }
}

fn done(reply: Reply) -> Result<()> {
    match reply {
        Reply::Done => Ok(()),
        other => Err(unexpected(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::fake_helper::{FakeHelper, serial};
    use crate::hardware::helper_protocol::{Request, Unavailable};

    fn unavailable_message(error: anyhow::Error) -> String {
        error.downcast_ref::<EcUnavailable>().unwrap_or_else(|| panic!("not EcUnavailable: {error:#}")).0.clone()
    }

    #[test]
    fn reads_and_writes_through_the_helper() {
        let _serial = serial();
        let helper = FakeHelper::start("rw", |s| s.registers[0x751] = 0xA0);
        let ec = Ec::connect().unwrap();
        let result = ec.run_for(Duration::from_secs(5), |r| {
            r.write(0x741, 0x12)?;
            r.update(0x751, 0x40, 0x40)?;
            Ok((r.read(0x751)?, r.bit(0x751, 6)?, r.read_many(&[0x741, 0x751])?))
        });
        assert_eq!(result.unwrap(), (0xE0, true, vec![0x12, 0xE0]));
        let state = helper.state.lock().unwrap();
        assert_eq!(state.registers[0x741], 0x12);
        // The update went as one request, so the helper does the read-modify-write.
        assert!(state.received.iter().any(|(_, r)| *r == Request::Update { address: 0x751, mask: 0x40, value: 0x40 }));
    }

    #[test]
    fn unavailable_reason_reaches_connect() {
        let _serial = serial();
        let _helper = FakeHelper::start("secure-boot", |s| s.status = Some(Unavailable::SecureBoot));
        let error = Ec::connect().err().expect("connect should fail");
        assert!(!may_come_back(&format!("{error:#}"))); // Secure Boot needs a restart
        assert_eq!(unavailable_message(error), Unavailable::SecureBoot.message());
    }

    #[test]
    fn missing_helper_says_how_to_fix() {
        let _serial = serial();
        let path = std::env::temp_dir().join(format!("uucc-ec-test-{}-missing.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        unsafe { std::env::set_var("UUCC_HELPER_SOCKET", &path) };
        let error = Ec::connect().err().expect("connect should fail");
        assert!(may_come_back(&format!("{error:#}"))); // the app tries again until the helper runs
        let message = unavailable_message(error);
        assert!(message.contains("sudo systemctl enable --now uucc-helper.socket"), "{message}");
    }

    #[test]
    fn a_stopped_helper_is_noticed() {
        let _serial = serial();
        let helper = FakeHelper::start("stopped", |s| s.close_first_after = Some(1)); // just Status
        let ec = Ec::connect().unwrap();
        drop(helper); // `systemctl stop uucc-helper.socket uucc-helper.service`
        let error = ec.run_for(Duration::from_secs(5), |r| r.check()).err().expect("the helper is gone");
        assert!(may_come_back(&format!("{error:#}")), "{error:#}");
    }

    #[test]
    fn hotkeys_arrive_in_order() {
        let _serial = serial();
        let _helper = FakeHelper::start("hotkeys", |s| s.hotkeys = vec![0xB1, 0xB2, 0xB0]);
        assert_eq!(subscribe_hotkeys().unwrap().collect::<Vec<_>>(), vec![177, 178, 176]);
    }

    #[test]
    fn hotkeys_refused_say_why() {
        let _serial = serial();
        let _helper = FakeHelper::start("hotkeys-refused", |s| s.hotkeys_error = Some("The helper can't hear the laptop's hotkeys: test".into()));
        let error = subscribe_hotkeys().err().expect("refused");
        assert!(format!("{error:#}").contains("The helper can't hear the laptop's hotkeys: test"), "{error:#}");
    }

    #[test]
    fn reconnects_after_helper_restart() {
        let _serial = serial();
        // The first connection answers Status and OnDisconnect, then goes away like a restarted helper.
        let helper = FakeHelper::start("restart", |s| {
            s.close_first_after = Some(2);
            s.registers[0x751] = 0x33;
        });
        let ec = Ec::connect().unwrap();
        ec.run_for(Duration::from_secs(5), |r| r.on_disconnect(0x7C6, 0x04, 0)).unwrap();
        let opened = connections_opened();
        let value = ec.run_for(Duration::from_secs(5), |r| r.read(0x751)).unwrap();
        assert_eq!(value, 0x33);
        // The old helper undid the app's settings as the connection broke, so the app sets them again.
        assert_eq!(connections_opened(), opened + 1);
        let state = helper.state.lock().unwrap();
        let later: Vec<&Request> = state.received.iter().filter(|(n, _)| *n > 0).map(|(_, r)| r).collect();
        assert_eq!(later, [&Request::OnDisconnect { address: 0x7C6, mask: 0x04, value: 0 }, &Request::Read { address: 0x751 }]);
        assert_eq!(state.releases.get(&0x7C6), Some(&(0x04, 0)));
    }

    #[test]
    fn custom_curve_registers_its_release() {
        let _serial = serial();
        let helper = FakeHelper::start("custom", |_| {});
        let ec = Ec::connect().unwrap();
        ec.run_for(Duration::from_secs(5), |r| crate::hardware::fans::set_custom(r, true)).unwrap();
        {
            let state = helper.state.lock().unwrap();
            assert_eq!(state.registers[0x7C6] & 0x04, 0x04);
            assert_eq!(state.releases.get(&0x7C6), Some(&(0x04, 0)));
        }
        ec.run_for(Duration::from_secs(5), |r| crate::hardware::fans::set_custom(r, false)).unwrap();
        assert_eq!(helper.state.lock().unwrap().releases.get(&0x7C6), None);
    }
}
