//! The app's end of the pipe `uucc` talks to. Only the Windows user running the app can connect, from the same PC.

use std::fs::File;
use std::mem::ManuallyDrop;
use std::os::windows::io::FromRawHandle;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree};
use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::IO::CancelSynchronousIo;
use windows::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentThreadId, OpenProcessToken, OpenThread, THREAD_TERMINATE};
use windows::core::{HSTRING, PWSTR};

use super::protocol::{Envelope, MAX_MESSAGE, Reply, VERSION, read_message, write_message};

/// How long a client may take to send its request.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Creates the pipe `name` (failing if anything already has it) and answers each request with `handle`, one client
/// at a time, on a thread of its own.
pub fn start(name: &str, handle: impl Fn(Envelope) -> Reply + Send + 'static) -> Result<()> {
    let pipe = create(name)?;
    let pipe = Pipe(pipe);
    std::thread::Builder::new().name("CLI server".into()).spawn(move || {
        let pipe = pipe;
        loop {
            // A client that connected between creating and connecting is fine too (ERROR_PIPE_CONNECTED).
            if let Err(e) = unsafe { ConnectNamedPipe(pipe.0, None) }
                && e.code() != windows::Win32::Foundation::ERROR_PIPE_CONNECTED.to_hresult()
            {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            serve(pipe.0, &handle);
            unsafe {
                let _ = DisconnectNamedPipe(pipe.0);
            }
        }
    })?;
    Ok(())
}

/// A handle moved to another thread.
struct Pipe(HANDLE);
unsafe impl Send for Pipe {}

fn serve(pipe: HANDLE, handle: &impl Fn(Envelope) -> Reply) {
    // The handle stays the server's; the File only borrows it.
    let mut file = ManuallyDrop::new(unsafe { File::from_raw_handle(pipe.0) });
    let reply = match with_timeout(|| read_message::<Envelope>(&mut *file)) {
        Ok(envelope) if envelope.version != VERSION => {
            Reply::Error { code: 1, message: "uucc and the app are different versions; reinstall the app.".into() }
        }
        Ok(envelope) => handle(envelope),
        // Sent nothing in time (or hung up): dropped without an answer, which it wouldn't read.
        Err(e) if matches!(e.kind(), std::io::ErrorKind::Interrupted | std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::BrokenPipe) => return,
        Err(e) if e.raw_os_error() == Some(995) => return, // ERROR_OPERATION_ABORTED: the timeout cancelled the read
        Err(e) => Reply::Error { code: 1, message: format!("couldn't read the request: {e}") },
    };
    if write_message(&mut *file, &reply).is_ok() {
        // Disconnecting throws away what the client hasn't read yet, so wait for it to hang up (it does once it has
        // the answer). Not FlushFileBuffers, which waits forever for a client that doesn't read.
        let _ = with_timeout(|| std::io::copy(&mut *file, &mut std::io::sink()));
    }
}

/// Runs `io` on this thread, cancelling its blocking reads or writes after `READ_TIMEOUT`.
fn with_timeout<T>(io: impl FnOnce() -> T) -> T {
    let this_thread = unsafe { OpenThread(THREAD_TERMINATE, false, GetCurrentThreadId()) }.map(Pipe);
    let (done, finished) = mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        let this_thread = this_thread;
        if finished.recv_timeout(READ_TIMEOUT).is_err()
            && let Ok(thread) = &this_thread
        {
            unsafe {
                let _ = CancelSynchronousIo(thread.0);
            }
        }
        if let Ok(thread) = this_thread {
            unsafe {
                let _ = CloseHandle(thread.0);
            }
        }
    });
    let result = io();
    let _ = done.send(());
    let _ = watchdog.join();
    result
}

fn create(name: &str) -> Result<HANDLE> {
    let descriptor = security_descriptor()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let pipe = unsafe {
        CreateNamedPipeW(
            &HSTRING::from(name),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            MAX_MESSAGE as u32,
            MAX_MESSAGE as u32,
            0,
            Some(&attributes),
        )
    };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    if pipe == INVALID_HANDLE_VALUE {
        bail!("Couldn't create {name}: {}", windows::core::Error::from_thread());
    }
    Ok(pipe)
}

/// Full access for this process's user and nobody else, at medium integrity: an object an elevated process creates
/// is otherwise labelled high, which keeps the user's ordinary programs from writing to it.
pub(crate) fn sddl_for(sid: &str) -> String {
    format!("D:P(A;;GA;;;{sid})S:(ML;;NW;;;ME)")
}

/// The descriptor from `sddl_for`, for this process's user. Free it with `LocalFree`.
fn security_descriptor() -> Result<PSECURITY_DESCRIPTOR> {
    let sid = user_sid()?;
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(&HSTRING::from(sddl_for(&sid)), SDDL_REVISION_1, &mut descriptor, None) }
        .context("Couldn't build the pipe's security descriptor")?;
    Ok(descriptor)
}

/// This process's user, as `S-1-5-21-…`.
fn user_sid() -> Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let mut size = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        let read = GetTokenInformation(token, TokenUser, Some(buffer.as_mut_ptr().cast()), size, &mut size);
        let _ = CloseHandle(token);
        read?;
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text)?;
        let sid = text.to_string()?;
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        Ok(sid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::protocol::{Envelope, Reply, Request, VERSION, read_message, write_message};
    use serde_json::json;
    use std::fs::{File, OpenOptions};
    use windows::Win32::System::Pipes::WaitNamedPipeW;
    use windows::core::HSTRING;

    fn test_pipe(test: &str) -> String {
        format!(r"\\.\pipe\uucc-test-{}-{test}", std::process::id())
    }

    /// Connects as `uucc` does: waits its turn while another client is being answered.
    fn connect(name: &str) -> File {
        for _ in 0..20 {
            match OpenOptions::new().read(true).write(true).open(name) {
                Ok(file) => return file,
                Err(e) if e.raw_os_error() == Some(231) => unsafe {
                    let _ = WaitNamedPipeW(&HSTRING::from(name), 5000); // ERROR_PIPE_BUSY
                },
                Err(e) => panic!("can't connect: {e}"),
            }
        }
        panic!("the pipe stayed busy");
    }

    fn ask(name: &str, envelope: &Envelope) -> Reply {
        let mut pipe = connect(name);
        write_message(&mut pipe, envelope).unwrap();
        read_message(&mut pipe).unwrap()
    }

    fn echo(envelope: Envelope) -> Reply {
        Reply::Ok(json!({ "got": format!("{:?}", envelope.request) }))
    }

    #[test]
    fn sddl_has_user_and_medium_label() {
        assert_eq!(sddl_for("S-1-5-21-1-2-3-1001"), "D:P(A;;GA;;;S-1-5-21-1-2-3-1001)S:(ML;;NW;;;ME)");
    }

    #[test]
    fn serves_a_request() {
        let name = test_pipe("serves");
        start(&name, echo).unwrap();
        let reply = ask(&name, &Envelope { version: VERSION, request: Request::Status });
        assert_eq!(reply, Reply::Ok(json!({ "got": "Status" })));
    }

    #[test]
    fn refuses_a_second_server() {
        let name = test_pipe("second");
        start(&name, echo).unwrap();
        assert!(start(&name, echo).is_err());
    }

    #[test]
    fn two_clients_both_answered() {
        let name = test_pipe("two");
        start(&name, |envelope| {
            std::thread::sleep(std::time::Duration::from_millis(300));
            echo(envelope)
        })
        .unwrap();
        let clients: Vec<_> = (0..2)
            .map(|_| {
                let name = name.clone();
                std::thread::spawn(move || ask(&name, &Envelope { version: VERSION, request: Request::Effects }))
            })
            .collect();
        for client in clients {
            assert_eq!(client.join().unwrap(), Reply::Ok(json!({ "got": "Effects" })));
        }
    }

    #[test]
    fn wrong_version_is_refused() {
        let name = test_pipe("version");
        start(&name, echo).unwrap();
        let reply = ask(&name, &Envelope { version: 99, request: Request::Status });
        assert!(matches!(reply, Reply::Error { code: 1, .. }), "{reply:?}");
    }

    #[test]
    fn a_silent_client_is_dropped() {
        let name = test_pipe("silent");
        start(&name, echo).unwrap();
        let _silent = connect(&name); // never writes
        let started = std::time::Instant::now();
        let reply = ask(&name, &Envelope { version: VERSION, request: Request::Status });
        assert_eq!(reply, Reply::Ok(json!({ "got": "Status" })));
        assert!(started.elapsed() < std::time::Duration::from_secs(8));
    }
}
