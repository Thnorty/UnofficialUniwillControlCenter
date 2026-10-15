//! The app's end of the socket `uucc` talks to on Linux: in the user's runtime folder, readable and writable by that
//! user only.

use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::protocol::{Envelope, Reply, VERSION, read_message, write_message};

/// How long a client may take to send its request.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Opens the socket at `path` (replacing one left by an app that ended without removing it, but not one that's
/// answered) and answers each request with `handle`, one client at a time, on a thread of its own.
pub fn start(path: &Path, handle: impl Fn(Envelope) -> Reply + Send + 'static) -> Result<()> {
    if UnixStream::connect(path).is_ok() {
        bail!("{} is in use by another program", path.display());
    }
    let _ = std::fs::remove_file(path);
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).with_context(|| format!("Couldn't make {}", folder.display()))?;
    }
    let listener = UnixListener::bind(path).with_context(|| format!("Couldn't open {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).with_context(|| format!("Couldn't make {} private", path.display()))?;
    std::thread::Builder::new().name("CLI server".into()).spawn(move || {
        for stream in listener.incoming() {
            if let Ok(mut stream) = stream {
                serve(&mut stream, &handle);
            }
        }
    })?;
    Ok(())
}

fn serve(stream: &mut UnixStream, handle: &impl Fn(Envelope) -> Reply) {
    if stream.set_read_timeout(Some(READ_TIMEOUT)).is_err() {
        return;
    }
    let reply = match read_message::<Envelope>(stream) {
        Ok(envelope) if envelope.version != VERSION => {
            Reply::Error { code: 1, message: "uucc and the app are different versions; reinstall the app.".into() }
        }
        Ok(envelope) => handle(envelope),
        // Sent nothing in time (or hung up): dropped without an answer, which it wouldn't read.
        Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::UnexpectedEof | ErrorKind::BrokenPipe) => return,
        Err(e) => Reply::Error { code: 1, message: format!("couldn't read the request: {e}") },
    };
    let _ = write_message(stream, &reply);
}
