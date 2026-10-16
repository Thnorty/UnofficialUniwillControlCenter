//! A stand-in for uucc-helper in tests: answers the helper's protocol on a temporary socket from registers and a
//! variable in memory, and records what it was asked.

use std::collections::BTreeMap;
use std::io::BufReader;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use super::helper_protocol::{self as protocol, Envelope, Reply, Request, Unavailable};

/// Tests that point UUCC_HELPER_SOCKET somewhere run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

pub fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Default)]
pub struct State {
    pub registers: Vec<u8>,
    pub status: Option<Unavailable>,
    /// The BIOS settings variable: attributes and bytes.
    pub variable: Option<(u32, Vec<u8>)>,
    /// (connection number, request), in order.
    pub received: Vec<(usize, Request)>,
    /// The latest connection's disconnect actions.
    pub releases: BTreeMap<u16, (u8, u8)>,
    /// Close the first connection after this many replies.
    pub close_first_after: Option<usize>,
}

pub struct FakeHelper {
    path: PathBuf,
    pub state: Arc<Mutex<State>>,
}

impl FakeHelper {
    /// Starts the fake and points the app at it. Hold `serial()` while it runs.
    pub fn start(name: &str, setup: impl FnOnce(&mut State)) -> FakeHelper {
        let path = std::env::temp_dir().join(format!("uucc-fake-helper-{}-{name}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let mut state = State { registers: vec![0; 0x1000], ..State::default() };
        setup(&mut state);
        let state = Arc::new(Mutex::new(state));
        let shared = state.clone();
        std::thread::spawn(move || {
            for (number, stream) in listener.incoming().enumerate() {
                let Ok(stream) = stream else { return };
                let state = shared.clone();
                std::thread::spawn(move || serve(number, stream, state));
            }
        });
        unsafe { std::env::set_var("UUCC_HELPER_SOCKET", &path) };
        FakeHelper { path, state }
    }
}

impl Drop for FakeHelper {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn serve(number: usize, stream: UnixStream, state: Arc<Mutex<State>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let mut replies = 0;
    state.lock().unwrap().releases.clear();
    while let Ok(Some(envelope)) = protocol::read_line::<Envelope>(&mut reader) {
        let reply = {
            let mut state = state.lock().unwrap();
            state.received.push((number, envelope.request.clone()));
            answer(&mut state, envelope.request)
        };
        protocol::write_line(&mut writer, &reply).unwrap();
        replies += 1;
        if number == 0 && state.lock().unwrap().close_first_after == Some(replies) {
            return;
        }
    }
}

fn answer(state: &mut State, request: Request) -> Reply {
    match request {
        Request::Status => match &state.status {
            Some(reason) => Reply::Unavailable { reason: reason.clone() },
            None => Reply::Ready,
        },
        Request::Read { address } => Reply::Byte { value: state.registers[address as usize] },
        Request::ReadMany { addresses } => Reply::Bytes { values: addresses.iter().map(|&a| state.registers[a as usize]).collect() },
        Request::Write { address, value } => {
            state.registers[address as usize] = value;
            Reply::Done
        }
        Request::Update { address, mask, value } => {
            let old = state.registers[address as usize];
            state.registers[address as usize] = (old & !mask) | (value & mask);
            Reply::Done
        }
        Request::OnDisconnect { address, mask, value } => {
            state.releases.insert(address, (mask, value));
            Reply::Done
        }
        Request::ClearOnDisconnect { address } => {
            state.releases.remove(&address);
            Reply::Done
        }
        Request::ReadVariable => match &state.variable {
            Some((attributes, bytes)) => Reply::Variable { attributes: *attributes, bytes: bytes.clone() },
            None => Reply::Error { message: "Couldn't read the BIOS settings: No such file or directory".into() },
        },
        Request::WriteVariable { attributes, bytes } => {
            state.variable = Some((attributes, bytes));
            Reply::Done
        }
        Request::Hotkeys => Reply::Error { message: "The fake helper has no hotkeys".into() },
    }
}
