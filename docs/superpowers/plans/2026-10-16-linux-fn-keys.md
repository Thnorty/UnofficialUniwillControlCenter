# Fn Keys on Linux Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On Linux, the laptop's hotkeys (keyboard light keys, light bar key, performance key, Fan Boost and fan safety reports) reach the app as on Windows, through the helper, and the per-second mode and fan-safety checks stop while they do.

**Architecture:** `uucc-helper` listens to the kernel's ACPI netlink events; on a `wmi` event `0xd2` it reads the hotkey code from EC window `0x47C` and pushes it to subscribed clients over a dedicated connection (`Request::Hotkeys` → `Reply::Hotkey`). The app's Linux `hotkeys::event_thread` subscribes and feeds the existing Windows handling; `Hotkeys` owns the EC's app-present bit on Linux as it does on Windows.

**Tech Stack:** Rust (std + `libc` for the netlink socket; no new crates), Slint app, systemd unit.

**Spec:** `docs/superpowers/specs/2026-10-16-linux-fn-keys-design.md`

## Global Constraints

- No new crates; netlink through `libc` (already a dependency).
- Windows behavior unchanged: every change outside `#[cfg(target_os = "linux")]` code must leave Windows' hotkeys, fans and polls as they are.
- Helper protocol `VERSION` becomes `2`.
- Hotkey event: netlink family `acpi_event`, class `"wmi"`, type `0xd2`; the code is EC window byte `0x47C`; `0` means no code.
- systemd unit: remove `PrivateNetwork=yes`; add `RestrictAddressFamilies=AF_UNIX AF_NETLINK` and `IPAddressDeny=any`.
- Commit messages: plain, no attribution lines.
- Tests: Windows `cargo test`; Linux `wsl -e bash scripts/wsl-cargo.sh test` (targets: `--bin uucc-helper`, `--bin UnofficialUniwillControlCenter`).

## Review Focus

1. A code read as `0` (cleared before the read, or another firmware's `0xd2`) must send nothing — Task 2 test `zero_code_sends_nothing`.
2. A subscriber that went away must be dropped without blocking the listener or the other subscribers — Task 2 test `dead_subscriber_is_dropped`.
3. Events that aren't `wmi`/`0xd2` (AC adapter, battery, lid) must send nothing — Task 1 test `other_events_are_not_hotkeys`.
4. A helper restart while the app runs: the app's hotkey thread ends, and the app subscribes again and sets the app-present bit again — Task 3 test `hotkey_thread_reports_its_end`, Task 4 test `ended_hotkeys_are_started_again`.
5. With the hotkeys active, turning a custom curve off must leave the app-present bit set — Task 4 test `custom_curve_leaves_app_present_to_hotkeys`.

---

### Task 1: Helper — ACPI netlink events

**Files:**
- Create: `src/bin/uucc-helper/acpi_events.rs`
- Modify: `src/bin/uucc-helper/main.rs` (add `#[cfg(target_os = "linux")] mod acpi_events;`)

**Interfaces:**
- Produces:
  - `pub struct AcpiEvent { pub class: String, pub bus_id: String, pub kind: u32, pub data: u32 }` (derive `Debug, PartialEq`)
  - `pub fn parse(buf: &[u8]) -> Vec<AcpiEvent>` — every event in one `recv` buffer
  - `pub fn is_hotkey(event: &AcpiEvent) -> bool` — `class == "wmi" && kind == 0xd2`
  - `pub struct Listener` with `pub fn open() -> std::io::Result<Listener>` and `pub fn next(&mut self) -> std::io::Result<Vec<AcpiEvent>>` (blocks for the next datagram)

Message layout (as `fn-probe3.py` read it on the laptop): a datagram holds netlink messages, each a 16-byte `nlmsghdr` (u32 length first), a 4-byte `genlmsghdr`, then attributes (u16 length, u16 type, payload, padded to 4). Attribute type 1 is the event: `device_class[20]`, `bus_id[15]`, one pad byte, `u32 type`, `u32 data` (44 bytes, native endian; strings NUL-terminated). `open()`: `socket(AF_NETLINK, SOCK_RAW, NETLINK_GENERIC)`, bind, send `CTRL_CMD_GETFAMILY` (3) for `"acpi_event"` to `GENL_ID_CTRL` (0x10) with `NLM_F_REQUEST`, read the reply's `CTRL_ATTR_MCAST_GROUPS` (7; nested, each group's attribute 2 is its u32 id), `setsockopt(SOL_NETLINK = 270, NETLINK_ADD_MEMBERSHIP = 1, id)` for each.

- [ ] **Step 1: Write the failing tests** in `acpi_events.rs` with a test helper `fn message(class: &str, bus: &str, kind: u32, data: u32) -> Vec<u8>` that builds one netlink message as above:
  - `parses_a_wmi_event`: `parse(&message("wmi", "PNP0C14:00", 0xd2, 0))` == `vec![AcpiEvent { class: "wmi".into(), bus_id: "PNP0C14:00".into(), kind: 0xd2, data: 0 }]`, and `is_hotkey` of it is true.
  - `parses_several_in_one_buffer`: two messages concatenated give two events in order.
  - `other_events_are_not_hotkeys`: `("ac_adapter", "ACPI0003:00", 0x80, 1)`, `("battery", "PNP0C0A:00", 0x80, 1)` and `("wmi", "PNP0C14:00", 0xd0, 0)` are not hotkeys.
  - `truncated_messages_are_ignored`: `parse` of the first 30 bytes of a message, and of an empty buffer, is empty (no panic).
- [ ] **Step 2: Run** `wsl -e bash scripts/wsl-cargo.sh test --bin uucc-helper acpi_events` — Expected: FAIL (module items missing).
- [ ] **Step 3: Implement** `AcpiEvent`, `parse`, `is_hotkey`, `Listener::open` and `Listener::next` with `libc` (no `unsafe` beyond the socket calls).
- [ ] **Step 4: Run** the same command — Expected: PASS. Then add `#[test] #[ignore] fn opens_in_this_kernel() { assert!(Listener::open().is_ok()) }` and run `wsl -e bash scripts/wsl-cargo.sh test --bin uucc-helper opens_in_this_kernel -- --ignored` — Expected: PASS (WSL's kernel has the `acpi_event` family, as the probe showed).
- [ ] **Step 5: Commit** — `git commit -m "Helper: read the kernel's ACPI netlink events"`

### Task 2: Helper — hotkey subscription

**Files:**
- Modify: `src/hardware/helper_protocol.rs` (`VERSION = 2`; `Request::Hotkeys`; `Reply::Hotkey { code: u8 }`)
- Modify: `src/bin/uucc-helper/main.rs` (subscribers in `Shared`; the listener thread; `Hotkeys` in `serve`)
- Modify: `src/bin/uucc-helper/server.rs` (`Request::Hotkeys` in `handle` is unreachable there, like `Status`)
- Modify: `packaging/linux/uucc-helper.service` (Global Constraints)

**Interfaces:**
- Consumes: Task 1's `AcpiEvent`, `is_hotkey`, `Listener`.
- Produces:
  - `const HOTKEY_CODE: u16 = 0x47C;` in `main.rs`
  - `fn hotkey_code(event: &AcpiEvent, ec: &mut dyn EcAccess) -> Option<u8>` — `Some(code)` for a hotkey event with a non-zero `0x47C`
  - `fn broadcast(shared: &Shared, code: u8)` — writes `Reply::Hotkey { code }` to each subscriber; drops any whose write fails or would block
  - `Shared` gains `subscribers: Mutex<Vec<UnixStream>>` and `listening: AtomicBool`
  - Protocol: a client sends `Hotkeys` as its first request on a connection; the helper answers `Done` and then only `Hotkey` lines. With no listener: `Reply::Error { message: "The helper can't hear the laptop's hotkeys: <why>" }`; with no EC: the usual `Unavailable`.

- [ ] **Step 1: Write the failing tests** in `main.rs`'s test module:
  - `hotkey_events_read_the_code`: a `FakeEc` with `0x47C = 0xB2`: `hotkey_code(&wmi_d2, ec)` is `Some(0xB2)`.
  - `zero_code_sends_nothing`: `0x47C = 0` gives `None`; a non-hotkey event with `0x47C = 0xB2` gives `None`.
  - `subscribers_get_codes`: start the server (`start(...)` as the other tests, authorize `|_| true`, with a `FakeEc` and `listening` set), connect, send `Hotkeys`, read `Done`; `broadcast(&shared, 0xB0)`; the client reads `Reply::Hotkey { code: 0xB0 }`. A second client that sent only `Status` reads nothing more.
  - `dead_subscriber_is_dropped`: two subscribers; drop one; `broadcast` twice; the live one reads both codes and `shared.subscribers` holds one stream.
  - `hotkeys_without_a_listener_are_refused`: `listening` false: `Hotkeys` is answered with `Reply::Error` whose message starts "The helper can't hear the laptop's hotkeys".
- [ ] **Step 2: Run** `wsl -e bash scripts/wsl-cargo.sh test --bin uucc-helper` — Expected: FAIL (missing items).
- [ ] **Step 3: Implement.** In `run()`, after `Shared` exists and only if the EC is usable: `Listener::open()`; on success set `listening` and spawn a thread looping `next()`, taking the EC lock per hotkey event, then `broadcast`; on failure `eprintln!` the reason (kept for the `Error` reply). In `serve`, a first request of `Hotkeys` is answered there (not in `server::handle`), the stream's clone set non-blocking goes into `subscribers`, and the thread then reads until EOF (ignoring input) so the connection stays counted. Bump `VERSION`; update the unit file.
- [ ] **Step 4: Run** the same command — Expected: PASS (all helper tests).
- [ ] **Step 5: Commit** — `git commit -m "Helper: push hotkey codes to subscribed clients"`

### Task 3: App — Linux hotkey thread

**Files:**
- Modify: `src/hardware/ec_linux.rs` (`pub fn subscribe_hotkeys() -> Result<HotkeyStream>`)
- Modify: `src/hardware/fake_helper.rs` (`State.hotkeys: Vec<u8>` pushed after `Done` to a client that sent `Hotkeys`, then that connection closes; `State.hotkeys_error: Option<String>` answers `Hotkeys` with `Reply::Error` instead)
- Modify: `src/hardware/hotkeys.rs` (Linux `event_thread`; `Hotkeys.alive`; app-present release on Linux; `pub fn active() -> bool`)

**Interfaces:**
- Consumes: Task 2's protocol (`Request::Hotkeys`, `Reply::Hotkey { code }`, `Done`).
- Produces:
  - `pub struct HotkeyStream` in `ec_linux.rs`, implementing `Iterator<Item = u32>` (ends when the helper's connection does)
  - `pub fn subscribe_hotkeys() -> Result<HotkeyStream>` — its own connection to `socket()` (no answer timeout after `Done`); an `Error`/`Refused`/`Unavailable`/`WrongVersion` answer is an `Err` with the helper's message
  - `Hotkeys::alive(&self) -> bool` — false once the event thread has ended (both platforms)
  - `hotkeys::active() -> bool` — whether a `Hotkeys` holds the app-present bit (`CURRENT` is `Some`)

- [ ] **Step 1: Write the failing tests** (Linux, with `FakeHelper` and `serial()`):
  - `ec_linux::tests::hotkeys_arrive_in_order`: `FakeHelper` with `hotkeys = vec![0xB1, 0xB2, 0xB0]`: `subscribe_hotkeys().unwrap().collect::<Vec<_>>()` is `[177, 178, 176]`.
  - `ec_linux::tests::hotkeys_refused_say_why`: with `State.hotkeys_error = Some("The helper can't hear the laptop's hotkeys: test")` the `Err` message contains that text.
  - `hotkeys::tests::linux_hotkeys_set_and_release_app_present` (Linux): `Hotkeys::start(ec, ...)` with `0x741 = 0x80` sets it to `0x81` and registers release `(0x741, 0x01, 0)`; `release()` sets `0x80` and clears the release.
  - `hotkeys::tests::hotkey_thread_reports_its_end`: with pushed codes `[0xB2]`, the `pressed` callback gets `178`, and within 2 s `alive()` is false (the fake closes after its codes).
- [ ] **Step 2: Run** `wsl -e bash scripts/wsl-cargo.sh test --bin UnofficialUniwillControlCenter hotkey` — Expected: FAIL.
- [ ] **Step 3: Implement.** Linux `event_thread`: `subscribe_hotkeys()`; send `Ok(())`/`Err` on `listening` as Windows does; for each code, if `active` is set, `pressed(code)`; on end set `alive` false. In `Hotkeys::start`'s EC closure, on Linux, register `on_disconnect(AP_OEM_BYTE, AP_EXIST_BIT, 0)` before setting the bit; `release_through` on Linux also `clear_on_disconnect(AP_OEM_BYTE)`.
- [ ] **Step 4: Run** the same command, then the whole Linux suite and Windows `cargo test` — Expected: PASS.
- [ ] **Step 5: Commit** — `git commit -m "Linux: take the hotkeys from the helper, with the app-present bit, as on Windows"`

### Task 4: App — use the hotkeys on Linux

**Files:**
- Modify: `src/app/mod.rs` (`start_hotkeys` without the Linux early return; `check_hotkeys` restarts ended hotkeys; the 1-second Linux checks gated)
- Modify: `src/hardware/fans.rs` (`set_custom` leaves the app-present bit to `Hotkeys` while `hotkeys::active()`)

**Interfaces:**
- Consumes: Task 3's `hotkeys::active()`, `Hotkeys::alive()`.
- Produces: `fn ec_polling_due(hotkeys_active: bool) -> bool` in `src/app/mod.rs` (true when they aren't active).

- [ ] **Step 1: Write the failing tests:**
  - `fans::tests::custom_curve_leaves_app_present_to_hotkeys` (Linux, `FakeHelper`): with a `Hotkeys` started (so `0x741 = 0x81`), `set_custom(r, true)` then `set_custom(r, false)` leaves `0x741 = 0x81` and the `0x741` release registered by `Hotkeys` in place.
  - `app::tests::polls_only_without_hotkeys`: `ec_polling_due(false)` is true, `ec_polling_due(true)` is false.
  - `app::tests::ended_hotkeys_are_started_again`: extract the decision as `fn hotkeys_need_restart(present: bool, alive: bool) -> bool` and assert `(true, false) → true`, `(true, true) → false`, `(false, _) → false` (start_hotkeys already covers "none").
- [ ] **Step 2: Run** `wsl -e bash scripts/wsl-cargo.sh test --bin UnofficialUniwillControlCenter` and Windows `cargo test` — Expected: FAIL (missing functions).
- [ ] **Step 3: Implement.** `start_hotkeys`: remove the Linux early return (on Linux there is no Control Center check). `check_hotkeys`: if `hotkeys_need_restart(...)`, take the `Hotkeys`, `abandon()` it (the helper has already released the bit if it restarted) and `start_hotkeys()`. The `every(1000)` Linux block runs `check_fan_safety` and `check_ec_mode` only when `ec_polling_due(self.hotkeys.is_some())`. `fans::set_custom`: wrap the Linux app-present lines in `if !hotkeys::active()`.
- [ ] **Step 4: Run** both suites — Expected: PASS.
- [ ] **Step 5: Commit** — `git commit -m "Linux: the Fn keys and the performance key through the hotkeys; no per-second EC checks while they work"`

### Task 5: Docs and hardware test

**Files:**
- Modify: `README.md` (Linux: "What works" gains the Fn keyboard light keys and the performance key with a curve on; "Not on Linux yet" keeps pop-ups, Windows key lock, touchpad toggle, Caps/Num Lock lights)
- Modify: `FEATURES.md` ("On Linux": the Fn keys bullet, the performance key bullet and the custom fan curve bullet describe the helper's netlink listener and the app-present bit owned by the hotkeys)
- Modify: `docs/linux-hardware-test.md` (rows: each key with the curve off and on; screen brightness, volume, touchpad, airplane and webcam keys still work; stopping the helper gives the keys back to the firmware; `journalctl -u uucc-helper` shows no netlink error)
- Modify: `docs/superpowers/specs/2026-10-15-linux-port-design.md` (the line saying Linux never sets the app-present bit)
- Modify: `ui/power.slint` (drop the Linux note "the performance key doesn't work on Linux yet" while the hotkeys work; it needs an `in property <bool> hotkeys-active` set from the app)

- [ ] **Step 1:** Update the files above; the UI note shows only when `Platform.linux && root.curve-enabled && !root.hotkeys-active`. The app sets `hotkeys-active` true in `hotkeys_started` when it keeps the `Hotkeys`, and false wherever `self.hotkeys` becomes `None` (`check_hotkeys`, exit).
- [ ] **Step 2: Run** both suites and `wsl -e bash scripts/build-linux.sh` — Expected: tests PASS, the rpm and deb are built.
- [ ] **Step 3: Commit** — `git commit -m "Docs: the Fn keys on Linux"`
