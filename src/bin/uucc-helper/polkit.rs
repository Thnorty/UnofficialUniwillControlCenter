//! Whether a client may use the helper: polkit's answer for the action `io.github.thnorty.uucc.hardware`, which by
//! default allows users signed in at the laptop (an active local session) without a password, and nobody else.

use std::collections::HashMap;

use zbus::zvariant::Value;

const ACTION: &str = "io.github.thnorty.uucc.hardware";

/// A process's start time (field 22 of `/proc/<pid>/stat`), which polkit pairs with the pid so a reused pid can't
/// pass for the process that asked. The name in field 2 may hold spaces and parentheses, so fields are counted after
/// the last `)`.
pub fn start_time(stat: &str) -> Option<u64> {
    let after_name = &stat[stat.rfind(')')? + 1..];
    // Field 3 (state) is the first after the name, so field 22 is the 20th.
    after_name.split_whitespace().nth(19)?.parse().ok()
}

/// Whether polkit lets the process `pid` (run by `uid`) use the helper. Any failure counts as no.
pub fn authorized(pid: u32, uid: u32) -> bool {
    check(pid, uid).unwrap_or(false)
}

fn check(pid: u32, uid: u32) -> Result<bool, Box<dyn std::error::Error>> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let start = start_time(&stat).ok_or("no start time")?;
    let connection = zbus::blocking::Connection::system()?;
    let mut subject: HashMap<&str, Value> = HashMap::new();
    subject.insert("pid", Value::from(pid));
    subject.insert("start-time", Value::from(start));
    subject.insert("uid", Value::from(uid as i32));
    let details: HashMap<&str, &str> = HashMap::new();
    let reply = connection.call_method(
        Some("org.freedesktop.PolicyKit1"),
        "/org/freedesktop/PolicyKit1/Authority",
        Some("org.freedesktop.PolicyKit1.Authority"),
        "CheckAuthorization",
        &(("unix-process", subject), ACTION, details, 0u32, ""),
    )?;
    // (is_authorized, is_challenge, details)
    let (is_authorized, _, _): (bool, bool, HashMap<String, String>) = reply.body().deserialize()?;
    Ok(is_authorized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_time_survives_spaces_in_the_name() {
        // pid (comm) state ppid pgrp session tty tpgid flags minflt cminflt majflt cmajflt utime stime cutime cstime
        // priority nice threads itrealvalue starttime ...
        let stat = "1234 (a b) c) S 1 1234 1234 0 -1 4194560 100 0 0 0 5 3 0 0 20 0 1 0 987654 12345 67 18446744073709551615";
        assert_eq!(start_time(stat), Some(987654));
        assert_eq!(start_time("garbage"), None);
    }
}
