//! The kernel's ACPI events (generic netlink family `acpi_event`, what acpid listens to). The laptop's hotkeys come
//! as the WMI device's event 0xd2: the kernel sends it even with no WMI driver bound.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// One ACPI event: the device's class and ACPI name, the event, and its data.
#[derive(Debug, PartialEq)]
pub struct AcpiEvent {
    pub class: String,
    pub bus_id: String,
    pub kind: u32,
    pub data: u32,
}

/// A netlink message's header, and a generic netlink message's after it.
const NLMSG_HEADER: usize = 16;
const GENL_HEADER: usize = 4;
/// ACPI_GENL_ATTR_EVENT: struct acpi_genl_event { char device_class[20]; char bus_id[15]; u32 type; u32 data; }.
const ATTR_EVENT: u16 = 1;
const EVENT_SIZE: usize = 44;

/// The attributes after a generic netlink header, by type.
fn attributes(mut buf: &[u8]) -> Vec<(u16, &[u8])> {
    let mut found = Vec::new();
    while buf.len() >= 4 {
        let length = u16::from_ne_bytes([buf[0], buf[1]]) as usize;
        let kind = u16::from_ne_bytes([buf[2], buf[3]]) & 0x7FFF;
        if length < 4 || length > buf.len() {
            break;
        }
        found.push((kind, &buf[4..length]));
        buf = &buf[((length + 3) & !3).min(buf.len())..];
    }
    found
}

fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Every ACPI event in one datagram; anything truncated or unknown is skipped.
pub fn parse(mut buf: &[u8]) -> Vec<AcpiEvent> {
    let mut events = Vec::new();
    while buf.len() >= NLMSG_HEADER {
        let length = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        if length < NLMSG_HEADER + GENL_HEADER || length > buf.len() {
            break;
        }
        for (kind, payload) in attributes(&buf[NLMSG_HEADER + GENL_HEADER..length]) {
            if kind == ATTR_EVENT && payload.len() >= EVENT_SIZE {
                let u32_at = |at: usize| u32::from_ne_bytes([payload[at], payload[at + 1], payload[at + 2], payload[at + 3]]);
                events.push(AcpiEvent { class: text(&payload[..20]), bus_id: text(&payload[20..35]), kind: u32_at(36), data: u32_at(40) });
            }
        }
        buf = &buf[((length + 3) & !3).min(buf.len())..];
    }
    events
}

/// The laptop's hotkey: the WMI device's event 0xd2 (`_Q53` hands the code to `AMW0.WMBC`, which raises it).
pub fn is_hotkey(event: &AcpiEvent) -> bool {
    event.class == "wmi" && event.kind == 0xd2
}

const GENL_ID_CTRL: u16 = 0x10;
const CTRL_CMD_GETFAMILY: u8 = 3;
const CTRL_ATTR_FAMILY_NAME: u16 = 2;
const CTRL_ATTR_MCAST_GROUPS: u16 = 7;
const CTRL_ATTR_MCAST_GRP_ID: u16 = 2;
const SOL_NETLINK: libc::c_int = 270;
const NETLINK_ADD_MEMBERSHIP: libc::c_int = 1;

/// A netlink socket joined to the `acpi_event` family's multicast group.
pub struct Listener {
    socket: OwnedFd,
    buffer: Vec<u8>,
}

impl Listener {
    pub fn open() -> io::Result<Listener> {
        let fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, libc::NETLINK_GENERIC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let socket = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        address.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        let bound = unsafe {
            libc::bind(fd, &address as *const libc::sockaddr_nl as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_nl>() as u32)
        };
        if bound != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut listener = Listener { socket, buffer: vec![0; 64 * 1024] };
        for group in listener.groups()? {
            let joined = unsafe {
                libc::setsockopt(fd, SOL_NETLINK, NETLINK_ADD_MEMBERSHIP, &group as *const u32 as *const libc::c_void, 4)
            };
            if joined != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(listener)
    }

    /// The `acpi_event` family's multicast groups, asked of the generic netlink controller.
    fn groups(&mut self) -> io::Result<Vec<u32>> {
        let name = b"acpi_event\0";
        let mut attribute = ((4 + name.len()) as u16).to_ne_bytes().to_vec();
        attribute.extend(CTRL_ATTR_FAMILY_NAME.to_ne_bytes());
        attribute.extend(name);
        attribute.resize((attribute.len() + 3) & !3, 0);
        let length = NLMSG_HEADER + GENL_HEADER + attribute.len();
        let mut request = (length as u32).to_ne_bytes().to_vec();
        request.extend(GENL_ID_CTRL.to_ne_bytes());
        request.extend((libc::NLM_F_REQUEST as u16).to_ne_bytes());
        request.extend(1u32.to_ne_bytes()); // sequence
        request.extend(0u32.to_ne_bytes()); // port: the kernel's
        request.extend([CTRL_CMD_GETFAMILY, 1, 0, 0]);
        request.extend(attribute);
        if unsafe { libc::send(self.socket.as_raw_fd(), request.as_ptr() as *const libc::c_void, request.len(), 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let reply = self.receive()?;
        if reply.len() < NLMSG_HEADER + GENL_HEADER {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a short netlink reply"));
        }
        if u16::from_ne_bytes([reply[4], reply[5]]) == libc::NLMSG_ERROR as u16 {
            let code = i32::from_ne_bytes([reply[16], reply[17], reply[18], reply[19]]);
            return Err(io::Error::from_raw_os_error(-code));
        }
        let length = (u32::from_ne_bytes([reply[0], reply[1], reply[2], reply[3]]) as usize).min(reply.len());
        let groups = attributes(&reply[NLMSG_HEADER + GENL_HEADER..length])
            .into_iter()
            .filter(|(kind, _)| *kind == CTRL_ATTR_MCAST_GROUPS)
            .flat_map(|(_, nested)| attributes(nested).into_iter().map(|(_, group)| group).collect::<Vec<_>>())
            .filter_map(|group| attributes(group).into_iter().find(|(kind, value)| *kind == CTRL_ATTR_MCAST_GRP_ID && value.len() >= 4))
            .map(|(_, id)| u32::from_ne_bytes([id[0], id[1], id[2], id[3]]))
            .collect::<Vec<_>>();
        if groups.is_empty() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "the kernel's acpi_event family has no group"));
        }
        Ok(groups)
    }

    fn receive(&mut self) -> io::Result<Vec<u8>> {
        let n = unsafe { libc::recv(self.socket.as_raw_fd(), self.buffer.as_mut_ptr() as *mut libc::c_void, self.buffer.len(), 0) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(self.buffer[..n as usize].to_vec())
    }

    /// The events in the next datagram; blocks until one comes.
    pub fn next(&mut self) -> io::Result<Vec<AcpiEvent>> {
        let datagram = self.receive()?;
        Ok(parse(&datagram))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One netlink message as the kernel sends it: nlmsghdr, genlmsghdr, then the event attribute.
    fn message(class: &str, bus: &str, kind: u32, data: u32) -> Vec<u8> {
        let mut event = vec![0u8; 44];
        event[..class.len()].copy_from_slice(class.as_bytes());
        event[20..20 + bus.len()].copy_from_slice(bus.as_bytes());
        event[36..40].copy_from_slice(&kind.to_ne_bytes());
        event[40..44].copy_from_slice(&data.to_ne_bytes());
        let mut attribute = ((4 + event.len()) as u16).to_ne_bytes().to_vec();
        attribute.extend(1u16.to_ne_bytes());
        attribute.extend(event);
        let length = 16 + 4 + attribute.len();
        let mut bytes = (length as u32).to_ne_bytes().to_vec();
        bytes.extend([0u8; 12]); // type, flags, sequence, port
        bytes.extend([1u8, 1, 0, 0]); // genlmsghdr: command, version
        bytes.extend(attribute);
        bytes
    }

    fn event(class: &str, bus: &str, kind: u32, data: u32) -> AcpiEvent {
        AcpiEvent { class: class.into(), bus_id: bus.into(), kind, data }
    }

    #[test]
    fn parses_a_wmi_event() {
        let events = parse(&message("wmi", "PNP0C14:00", 0xd2, 0));
        assert_eq!(events, vec![event("wmi", "PNP0C14:00", 0xd2, 0)]);
        assert!(is_hotkey(&events[0]));
    }

    #[test]
    fn parses_several_in_one_buffer() {
        let mut bytes = message("ac_adapter", "ACPI0003:00", 0x80, 1);
        bytes.extend(message("wmi", "PNP0C14:00", 0xd2, 0));
        assert_eq!(parse(&bytes), vec![event("ac_adapter", "ACPI0003:00", 0x80, 1), event("wmi", "PNP0C14:00", 0xd2, 0)]);
    }

    #[test]
    fn other_events_are_not_hotkeys() {
        assert!(!is_hotkey(&event("ac_adapter", "ACPI0003:00", 0x80, 1)));
        assert!(!is_hotkey(&event("battery", "PNP0C0A:00", 0x80, 1)));
        assert!(!is_hotkey(&event("wmi", "PNP0C14:00", 0xd0, 0)));
    }

    #[test]
    #[ignore] // needs a kernel with the acpi_event family (WSL's has it)
    fn opens_in_this_kernel() {
        assert!(Listener::open().is_ok());
    }

    #[test]
    fn truncated_messages_are_ignored() {
        assert!(parse(&message("wmi", "PNP0C14:00", 0xd2, 0)[..30]).is_empty());
        assert!(parse(&[]).is_empty());
    }
}
