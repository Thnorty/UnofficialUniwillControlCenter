//! The helper's socket: each client is checked with polkit, then its requests are answered one by one. EC access is
//! serialized across clients, and what a client registered to undo is undone when it goes away.

use std::path::Path;
use std::time::Duration;

use crate::ec::EcAccess;
use crate::efivar;
use crate::protocol::{Reply, Request, Unavailable};
use crate::rules::{self, Releases};

/// How long the helper stays without clients before it exits; systemd starts it again on the next connection.
const IDLE: Duration = Duration::from_secs(60);

/// One client's state: what to undo when it goes away.
#[derive(Default)]
pub struct Session {
    releases: Releases,
}

impl Session {
    /// What to undo when the client goes away.
    pub fn releases(&self) -> &Releases {
        &self.releases
    }
}

fn not_written(address: u16) -> Reply {
    Reply::Refused { message: format!("Register 0x{address:03X} isn't one this app writes") }
}

fn not_read(address: u16) -> Reply {
    Reply::Refused { message: format!("Register 0x{address:03X} isn't in the EC's RAM") }
}

/// Answers one request. `ec` is the mapped EC (None when `status` says why it can't be used), `variable` the BIOS
/// settings variable's file.
pub fn handle(request: Request, session: &mut Session, ec: Option<&mut dyn EcAccess>, status: &Result<u64, Unavailable>, variable: &Path) -> Reply {
    let unavailable = || match status {
        Err(reason) => Reply::Unavailable { reason: reason.clone() },
        Ok(_) => Reply::Unavailable { reason: Unavailable::MapFailed { error: "the EC isn't mapped".into() } },
    };
    match request {
        Request::Status => match (status, &ec) {
            (Ok(_), Some(_)) => Reply::Ready,
            _ => unavailable(),
        },
        Request::ReadVariable => match efivar::read(variable) {
            Ok((attributes, bytes)) => Reply::Variable { attributes, bytes },
            Err(error) => Reply::Error { message: format!("Couldn't read the BIOS settings: {error}") },
        },
        Request::WriteVariable { attributes, bytes } => {
            let allowed = efivar::read(variable)
                .map_err(|error| format!("Couldn't read the BIOS settings: {error}"))
                .and_then(|(current_attributes, current)| rules::variable_write_allowed(current_attributes, &current, attributes, &bytes)
                    .map_err(|reason| format!("The helper refused that change to the BIOS settings: {reason}")));
            match allowed {
                Err(message) => Reply::Refused { message },
                Ok(()) => match efivar::write(variable, attributes, &bytes, &efivar::Ioctl) {
                    Ok(()) => Reply::Done,
                    Err(error) => Reply::Error { message: format!("Couldn't write the BIOS settings: {error}") },
                },
            }
        }
        ec_request => {
            let Some(ec) = ec else { return unavailable() };
            ec_reply(ec_request, session, ec)
        }
    }
}

fn ec_reply(request: Request, session: &mut Session, ec: &mut dyn EcAccess) -> Reply {
    match request {
        Request::Read { address } if rules::readable(address) => Reply::Byte { value: ec.read(address) },
        Request::Read { address } => not_read(address),
        Request::ReadMany { addresses } => match addresses.iter().find(|a| !rules::readable(**a)) {
            Some(&address) => not_read(address),
            None => Reply::Bytes { values: addresses.iter().map(|&a| ec.read(a)).collect() },
        },
        Request::Write { address, value } if rules::writable(address) => {
            ec.write(address, value);
            Reply::Done
        }
        Request::Update { address, mask, value } if rules::writable(address) => {
            update(ec, address, mask, value);
            Reply::Done
        }
        Request::OnDisconnect { address, mask, value } if rules::writable(address) => {
            session.releases.set(address, mask, value);
            Reply::Done
        }
        Request::ClearOnDisconnect { address } => {
            session.releases.clear(address);
            Reply::Done
        }
        Request::Write { address, .. } | Request::Update { address, .. } | Request::OnDisconnect { address, .. } => not_written(address),
        Request::Status | Request::ReadVariable | Request::WriteVariable { .. } => unreachable!("answered in handle"),
        Request::Hotkeys => Reply::Error { message: "Hotkeys must be the first request on a connection of its own".into() },
    }
}

/// Sets the bits in `mask` to those of `value`, leaving the others as the EC has them.
fn update(ec: &mut dyn EcAccess, address: u16, mask: u8, value: u8) {
    let old = ec.read(address);
    ec.write(address, (old & !mask) | (value & mask));
}

/// Undoes what the client asked to have undone when it went away.
pub fn release(session: &mut Session, ec: &mut dyn EcAccess) {
    apply(&mut session.releases, ec);
}

/// Undoes `releases`, leaving none.
pub fn apply(releases: &mut Releases, ec: &mut dyn EcAccess) {
    for (address, mask, value) in releases.take() {
        update(ec, address, mask, value);
    }
}

/// Whether the helper should exit: nobody connected, for a while.
pub fn idle_should_exit(clients: usize, idle_for: Duration) -> bool {
    clients == 0 && idle_for >= IDLE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ec::FakeEc;
    use std::path::PathBuf;

    fn ready() -> Result<u64, Unavailable> {
        Ok(0xFE41_0000)
    }

    fn no_variable() -> PathBuf {
        PathBuf::from("/nonexistent/uucc-test-variable")
    }

    fn ask(session: &mut Session, ec: &mut FakeEc, request: Request) -> Reply {
        handle(request, session, Some(ec), &ready(), &no_variable())
    }

    #[test]
    fn status_reports_unavailable_reason() {
        let mut session = Session::default();
        assert_eq!(handle(Request::Status, &mut session, None, &Err(Unavailable::SecureBoot), &no_variable()), Reply::Unavailable {
            reason: Unavailable::SecureBoot
        });
        let mut ec = FakeEc([0; 0x1000]);
        assert_eq!(ask(&mut session, &mut ec, Request::Status), Reply::Ready);
        // EC requests without a usable EC get the reason too.
        let reply = handle(Request::Read { address: 0x751 }, &mut session, None, &Err(Unavailable::NoEcDevice), &no_variable());
        assert_eq!(reply, Reply::Unavailable { reason: Unavailable::NoEcDevice });
    }

    #[test]
    fn write_outside_rules_is_refused() {
        let mut session = Session::default();
        let mut ec = FakeEc([0; 0x1000]);
        let reply = ask(&mut session, &mut ec, Request::Write { address: 0x750, value: 1 });
        assert!(matches!(reply, Reply::Refused { .. }), "{reply:?}");
        assert_eq!(ec.0[0x750], 0);
        let reply = ask(&mut session, &mut ec, Request::OnDisconnect { address: 0x750, mask: 1, value: 0 });
        assert!(matches!(reply, Reply::Refused { .. }), "{reply:?}");
        let reply = ask(&mut session, &mut ec, Request::Read { address: 0x1000 });
        assert!(matches!(reply, Reply::Refused { .. }), "{reply:?}");
    }

    #[test]
    fn update_changes_only_the_mask() {
        let mut session = Session::default();
        let mut ec = FakeEc([0; 0x1000]);
        ec.0[0x751] = 0xA0;
        assert_eq!(ask(&mut session, &mut ec, Request::Update { address: 0x751, mask: 0x40, value: 0x40 }), Reply::Done);
        assert_eq!(ec.0[0x751], 0xE0);
        assert_eq!(ask(&mut session, &mut ec, Request::Read { address: 0x751 }), Reply::Byte { value: 0xE0 });
    }

    #[test]
    fn updates_from_two_sessions_keep_both_bits() {
        let (mut a, mut b) = (Session::default(), Session::default());
        let mut ec = FakeEc([0; 0x1000]);
        ask(&mut a, &mut ec, Request::Update { address: 0x7C6, mask: 0x04, value: 0x04 });
        ask(&mut b, &mut ec, Request::Update { address: 0x7C6, mask: 0x01, value: 0x01 });
        assert_eq!(ec.0[0x7C6], 0x05);
    }

    #[test]
    fn disconnect_applies_releases() {
        let mut session = Session::default();
        let mut ec = FakeEc([0; 0x1000]);
        ask(&mut session, &mut ec, Request::Update { address: 0x7C6, mask: 0x04, value: 0x04 });
        assert_eq!(ask(&mut session, &mut ec, Request::OnDisconnect { address: 0x7C6, mask: 0x04, value: 0 }), Reply::Done);
        ec.0[0x7C6] |= 0x01; // another bit, which stays
        release(&mut session, &mut ec);
        assert_eq!(ec.0[0x7C6], 0x01);
    }

    #[test]
    fn clear_on_disconnect_skips_release() {
        let mut session = Session::default();
        let mut ec = FakeEc([0; 0x1000]);
        ec.0[0x7C6] = 0x04;
        ask(&mut session, &mut ec, Request::OnDisconnect { address: 0x7C6, mask: 0x04, value: 0 });
        assert_eq!(ask(&mut session, &mut ec, Request::ClearOnDisconnect { address: 0x7C6 }), Reply::Done);
        release(&mut session, &mut ec);
        assert_eq!(ec.0[0x7C6], 0x04);
    }

    #[test]
    fn read_many_returns_in_order() {
        let mut session = Session::default();
        let mut ec = FakeEc([0; 0x1000]);
        ec.0[0x464] = 0x08;
        ec.0[0x465] = 0x39;
        let reply = ask(&mut session, &mut ec, Request::ReadMany { addresses: vec![0x465, 0x464] });
        assert_eq!(reply, Reply::Bytes { values: vec![0x39, 0x08] });
    }

    #[test]
    fn variable_requests_use_the_file() {
        let path = std::env::temp_dir().join(format!("uucc-server-variable-{}", std::process::id()));
        let mut contents = 7u32.to_le_bytes().to_vec();
        contents.extend([0u8; crate::efivar::SIZE]);
        std::fs::write(&path, &contents).unwrap();
        let mut session = Session::default();
        let reply = handle(Request::ReadVariable, &mut session, None, &Err(Unavailable::SecureBoot), &path);
        assert_eq!(reply, Reply::Variable { attributes: 7, bytes: vec![0; crate::efivar::SIZE] });
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn variable_writes_the_app_wouldnt_make_are_refused() {
        let path = std::env::temp_dir().join(format!("uucc-server-variable-refused-{}", std::process::id()));
        let mut contents = 7u32.to_le_bytes().to_vec();
        contents.extend([0u8; crate::efivar::SIZE]);
        std::fs::write(&path, &contents).unwrap();
        let mut session = Session::default();
        let bytes = vec![0x5A; crate::efivar::SIZE];
        let reply = handle(Request::WriteVariable { attributes: 0, bytes }, &mut session, None, &Err(Unavailable::SecureBoot), &path);
        assert!(matches!(reply, Reply::Refused { .. }), "{reply:?}");
        assert_eq!(std::fs::read(&path).unwrap(), contents);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn idle_exit_only_without_clients() {
        assert!(!idle_should_exit(1, Duration::from_secs(600)));
        assert!(!idle_should_exit(0, Duration::from_secs(59)));
        assert!(idle_should_exit(0, Duration::from_secs(60)));
    }
}
