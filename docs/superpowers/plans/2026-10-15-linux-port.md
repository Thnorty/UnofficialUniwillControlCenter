# Linux Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build and package Unofficial Uniwill Control Center for Linux (Fedora, Ubuntu), with a small root helper for the EC and the BIOS variable, while the Windows build stays unchanged.

**Architecture:** The app runs as the signed-in user; each Windows-specific module gets a Linux sibling chosen with `cfg`. `uucc-helper` (third binary, root, socket-activated, polkit-checked) does EC reads/writes through `/dev/mem` and `UniWillVariable` reads/writes; lighting goes straight to `hidraw` through a udev rule. Packages (`.rpm`, `.deb`) install everything.

**Tech Stack:** Rust 2024, Slint 1.18 (winit, Skia), `libc` 0.2, `zbus` 5 (blocking API), `ksni` 0.3, `rfd` 0.17 (XDG portal), `libloading` 0.9, `cargo-deb`, `cargo-generate-rpm`.

**Spec:** `docs/superpowers/specs/2026-10-15-linux-port-design.md`

## Global Constraints

- The Windows build behaves exactly as before. After every task, on Windows: `cargo test` passes (44 app + 26 uucc tests before this plan, plus new ones) and `cargo build --release` succeeds.
- Linux builds and tests run in WSL Ubuntu 24.04 through `scripts/wsl-cargo.sh` (Task 1), with `CARGO_TARGET_DIR=$HOME/uucc-target` so Windows and Linux artifacts never mix. From Windows: `wsl -e bash scripts/wsl-cargo.sh test`.
- Platform modules: keep the Windows file as it is and add a Linux sibling, selected in the parent module:
  `#[cfg(windows)] pub mod x;` / `#[cfg(target_os = "linux")] #[path = "x_linux.rs"] pub mod x;`. Both expose the same public API the shared code uses. Items needed by only one platform are `cfg`-gated in place.
- Names (verbatim from the spec): helper binary `uucc-helper` (installed to `/usr/libexec/uucc-helper`); helper socket `/run/uucc-helper.sock`; polkit action `io.github.thnorty.uucc.hardware`; uucc socket `$XDG_RUNTIME_DIR/uucc.sock` (demo: `uucc-demo.sock`); settings `$XDG_CONFIG_HOME/UnofficialUniwillControlCenter` (default `~/.config/…`); installed app name `unofficial-uniwill-control-center` (the Cargo `[[bin]]` keeps its name; packages rename it).
- `UniWillVariable` GUID `9f33f85c-13ca-4fd1-9c4a-96217722c593`, 180 bytes. EC window: one 4 KB page at the base taken from the DSDT's `ECRR`.
- EC registers the helper lets clients write (spec, verbatim): `0x741`, `0x743`–`0x746`, `0x751`, `0x767`, `0x783`–`0x787`, `0x7A6`, `0x7C5`, `0x7C6`, `0xF00`–`0xF5F`. Reads: `0x000`–`0xFFF`.
- The repository is public: no personal data in code, tests, fixtures or commits. No attribution lines in commits. One commit per task, messages in the repo's style (`Area: what changed`).
- Code style: match the surrounding code (doc comments as full sentences, the existing naming, `anyhow` errors with user-readable messages).
- Work on branch `linux-port`; it merges to main only when the owner says so. No release is published without asking.

## Review Focus

1. **The helper exits or restarts while the app is connected** (idle timeout, package upgrade, `systemctl restart`): the helper never idles out with a client connected, and the app's next EC call reconnects once and registers its disconnect actions again. Tests: Task 5 `idle_exit_only_without_clients`, Task 6 `reconnects_after_helper_restart`.
2. **Two clients at once** (the app plus a second app instance or a test tool): a read-modify-write from one can't lose the other's bit. Test: Task 5 `updates_from_two_sessions_keep_both_bits`.
3. **`hidraw` permission denied** (package not installed, the udev rule not applied until replug or re-login, running from `cargo run`): the lighting status says which device and how to fix it, instead of "not found". Test: Task 8 `permission_error_names_the_fix`.
4. **A kernel without the lockdown LSM file** (`/sys/kernel/security/lockdown` missing, or securityfs not mounted): treated as no lockdown, not as an error. Test: Task 3 `missing_lockdown_file_means_no_lockdown`.
5. **Intel-only GPU mode or no NVIDIA GPU at all** (no PCI device with vendor `0x10de`): the GPU reads as off, and NVML is never loaded. Test: Task 9 `no_nvidia_device_reads_as_off`.

---

### Task 1: Build on Linux (scaffolding with stubs)

**Files:**
- Create: `scripts/wsl-cargo.sh`
- Modify: `Cargo.toml`, `build.rs`, `src/main.rs`, `src/hardware/mod.rs`, `src/shell/mod.rs`, `src/lighting/mod.rs`, `src/cli/mod.rs`, `src/plugins/mod.rs`, `src/app/mod.rs`, `src/app/{backup,plugins,setups}.rs`, `src/settings.rs`, `src/bin/uucc/main.rs`, `src/hardware/{battery,hotkeys,status,uefi}.rs`, `src/lighting/engine.rs`, `src/plugins/host.rs`
- Create (stubs): `src/hardware/{ec,gpu,system,status_platform}_linux.rs`, `src/shell/{instance,tray,osd,display}_linux.rs`, `src/lighting/{device,audio,keyhook}_linux.rs` as needed, `src/cli/server_linux.rs`, `src/bin/uucc/connect_linux.rs`, `src/plugins/{readings,downloads}_linux.rs`

**Interfaces:**
- Produces: `shell::Owner` (Windows: `HWND`; Linux: a unit struct `Owner`), used wherever `window_hwnd()` feeds dialogs; `App::window_owner(&self) -> Option<shell::Owner>` replaces `window_hwnd()` in shared code. `shell::tray::Icon` (Windows: `HICON`; Linux: `struct Icon { size: usize, argb: Vec<u8> }`), returned by `tray::make_icon` and taken by `Tray::set_icon`. `settings::folder(name)` resolves `XDG_CONFIG_HOME` (fallback `$HOME/.config`) on Linux, `%APPDATA%` on Windows.
- Every Linux stub has the Windows module's public signatures; hardware stubs return `Err(anyhow!("Not available on Linux yet"))`, readers return `None`/empty, UI-side stubs do nothing.

- [ ] **Step 1: Set up WSL (needs the owner for `sudo`).** Ask the owner to run in WSL:
  `sudo apt install -y libfontconfig1-dev libfreetype-dev libxkbcommon-dev libwayland-dev libdbus-1-dev libudev-dev clang rpm`
  Then install Rust without sudo: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y`.
- [ ] **Step 2: Write `scripts/wsl-cargo.sh`**: `cd` to the repo's `/mnt/c/...` path from the script's own location, `export CARGO_TARGET_DIR="$HOME/uucc-target"`, source `$HOME/.cargo/env`, run `cargo "$@"`.
- [ ] **Step 3: Run it to see the failures.** `wsl -e bash scripts/wsl-cargo.sh build` — Expected: FAIL, `windows`/`wmi` don't compile on Linux.
- [ ] **Step 4: `Cargo.toml`**: move `windows` and `wmi` to `[target.'cfg(windows)'.dependencies]`; add `[target.'cfg(target_os = "linux")'.dependencies]` with `libc = "0.2"` (more Linux crates arrive with the tasks that use them). `build.rs`: call `embed_resource::compile` only when `CARGO_CFG_TARGET_OS == "windows"`.
- [ ] **Step 5: Platform split and stubs** following the Global Constraints convention. Windows-only features that stay out of the Linux build (`shell::display`, `lighting::audio`, `lighting::keyhook`, `hardware::hotkeys::Hotkeys` with its WMI listener, `plugins::readings`/`downloads`): Linux stubs that report "not available". `hotkeys::{release_current, without_app_present}` and the code constants stay shared. `battery::control_center_*` and `system::control_center_running` return `None`/`false` on Linux. `main.rs`: the Linux path has no elevation check and shows errors with `eprintln!` until Task 10 adds dialogs. `app/mod.rs`: `cfg` the HWND, DWM, cursor-monitor and `GetLastInputInfo` spots (Linux: no title-bar styling, no idle detection, `fit_to_screen` does nothing). `engine.rs`/`host.rs`: COM init only on Windows. Mark Windows-only tests `#[cfg(windows)]`.
- [ ] **Step 6: Run the Linux build and tests.** `wsl -e bash scripts/wsl-cargo.sh test` — Expected: builds; all shared tests pass.
- [ ] **Step 7: Run the demo window under WSLg.** `wsl -e bash scripts/wsl-cargo.sh run --bin UnofficialUniwillControlCenter -- --demo` — Expected: the window opens (icons may show as boxes until Task 12).
- [ ] **Step 8: Windows still green.** `cargo test` and `cargo build --release` on Windows — Expected: same pass counts as before.
- [ ] **Step 9: Commit** `Linux: the crate builds on Linux, with stubs for the Windows-only parts`.

### Task 2: Helper protocol and write rules

**Files:**
- Create: `src/hardware/helper_protocol.rs` (shared by app and helper, like `cli/protocol.rs` is shared with `uucc`), `src/bin/uucc-helper/main.rs` (minimal: `mod rules;` + `fn main() {}` for now), `src/bin/uucc-helper/rules.rs`
- Modify: `Cargo.toml` (`[[bin]] name = "uucc-helper"`, `path = "src/bin/uucc-helper/main.rs"`), `src/hardware/mod.rs` (`#[cfg(target_os = "linux")] pub mod helper_protocol;`)

**Interfaces:**
- Produces (`helper_protocol.rs`): `pub const VERSION: u32 = 1; pub const SOCKET: &str = "/run/uucc-helper.sock"; pub const MAX_LINE: usize = 64 * 1024;`
  `pub struct Envelope { pub version: u32, pub request: Request }`
  `#[serde(tag = "type")] pub enum Request { Status, Read { address: u16 }, ReadMany { addresses: Vec<u16> }, Write { address: u16, value: u8 }, Update { address: u16, mask: u8, value: u8 }, OnDisconnect { address: u16, mask: u8, value: u8 }, ClearOnDisconnect { address: u16 }, ReadVariable, WriteVariable { attributes: u32, bytes: Vec<u8> } }`
  `#[serde(tag = "type")] pub enum Reply { Ready, Unavailable { reason: Unavailable }, Byte { value: u8 }, Bytes { values: Vec<u8> }, Done, Variable { attributes: u32, bytes: Vec<u8> }, Refused { message: String }, Error { message: String }, WrongVersion { helper: u32 } }`
  `pub enum Unavailable { SecureBoot, NoEcDevice, DriverInCharge { driver: String }, MapFailed { error: String } }` with `fn message(&self) -> String` (copy below)
  `pub fn write_line(w: &mut impl Write, value: &impl Serialize) -> io::Result<()>`, `pub fn read_line<T: DeserializeOwned>(r: &mut impl BufRead) -> io::Result<Option<T>>` (`None` at EOF; error over `MAX_LINE`).
- Produces (`rules.rs`): `pub fn writable(address: u16) -> bool`, `pub fn readable(address: u16) -> bool`; `pub struct Releases` with `fn set(&mut self, address: u16, mask: u8, value: u8)`, `fn clear(&mut self, address: u16)`, `fn take(&mut self) -> Vec<(u16, u8, u8)>` (sorted by address; latest `set` per address wins).
- `Unavailable::message` copy (exact):
  - SecureBoot: "Secure Boot is on, so Linux blocks direct access to the embedded controller. Modes, fans, power limits and battery care need Secure Boot off for now: see the README's Linux section."
  - NoEcDevice: "This laptop's firmware has no embedded controller interface this app knows (INOU0000 with ECRR)."
  - DriverInCharge: "Linux's {driver} driver controls the embedded controller, so this app leaves it alone."
  - MapFailed: "Couldn't reach the embedded controller: {error}"

- [ ] **Step 1: Write the failing tests** in `rules.rs`: `every_register_the_app_writes_is_writable` (each address of the Global Constraints list, including both ends of each range); `neighbors_are_not_writable` (`0x740, 0x742, 0x747, 0x750, 0x752, 0x766, 0x768, 0x782, 0x788, 0x7A5, 0x7A7, 0x7C4, 0x7C7, 0xEFF, 0xF60` → false); `reads_cover_the_window` (`0x000`, `0xFFF` true; `0x1000` false); `releases_keep_the_latest_per_address` (set `0x7C6` mask 4 value 0, then value 4 → `take()` = `[(0x7C6, 4, 4)]`, and empty after); `cleared_release_is_not_applied`. In `helper_protocol.rs`: `request_round_trips_as_one_json_line`, `oversized_line_is_an_error`.
- [ ] **Step 2: Run them.** `wsl -e bash scripts/wsl-cargo.sh test --bin uucc-helper` and `… test helper_protocol` — Expected: FAIL (items missing).
- [ ] **Step 3: Implement** the items above.
- [ ] **Step 4: Run them.** Same commands — Expected: PASS. On Windows the helper's `main.rs` compiles to an empty `fn main` (its modules are `#[cfg(target_os = "linux")]`), and `cargo test` is unaffected.
- [ ] **Step 5: Commit** `Linux helper: the protocol and the registers it may write`.

### Task 3: Helper EC window and status

**Files:**
- Create: `src/bin/uucc-helper/ec.rs`, `src/bin/uucc-helper/testdata/ecrr.bin` (the 31 bytes from this laptop's DSDT at the `ECRR` method: `14 1a 45 43 52 52 01 72 0c 00 00 41 fe 68 60 70 4d 4d 52 57 60 00 00 00 61 a4 61 14 16 45 43` — firmware AML only, no personal data)

**Interfaces:**
- Produces: `pub fn ecrr_base(dsdt: &[u8]) -> Option<u64>` — find the bytes `"ECRR"` preceded by a Method op (`0x14` + PkgLength of 1–4 bytes) and followed by the flags byte, then require `72 0C <u32 LE> 68`; return the `u32`.
  `pub struct Paths { pub lockdown: PathBuf, pub inou: PathBuf, pub dsdt: PathBuf }` with `Paths::system()` = `/sys/kernel/security/lockdown`, `/sys/bus/acpi/devices/INOU0000:00`, `/sys/firmware/acpi/tables/DSDT`.
  `pub fn check(paths: &Paths) -> Result<u64, Unavailable>` — in this order: lockdown file contains `[integrity]` or `[confidentiality]` → `SecureBoot` (missing or unreadable file → no lockdown); `inou` missing → `NoEcDevice`; `inou/physical_node/driver` exists → `DriverInCharge { driver: <basename of its target> }`; DSDT unreadable or `ecrr_base` `None` → `NoEcDevice`; else the base.
  `pub struct EcWindow` with `fn map(base: u64) -> io::Result<EcWindow>` (open `/dev/mem` `O_RDWR | O_SYNC`, `mmap` 4096 bytes `MAP_SHARED` at `base`), `fn read(&self, address: u16) -> u8`, `fn write(&self, address: u16, value: u8)` — single-byte `read_volatile`/`write_volatile`; callers have checked `rules`. `impl Drop` unmaps.
  `pub trait EcAccess { fn read(&mut self, a: u16) -> u8; fn write(&mut self, a: u16, v: u8); }` implemented by `EcWindow` and, in tests, by `struct FakeEc([u8; 4096])`.

- [ ] **Step 1: Failing tests**: `finds_the_base_in_this_laptops_ecrr` (`ecrr_base(include_bytes!("testdata/ecrr.bin"))` = `Some(0xFE41_0000)`); `other_method_body_gives_none` (same bytes with `0x72` changed to `0x70` → `None`); `ecrr_without_method_op_gives_none`; with temp dirs for `Paths`: `lockdown_integrity_means_secure_boot` (`none [integrity] confidentiality`), `lockdown_none_passes` (`[none] integrity confidentiality`), `missing_lockdown_file_means_no_lockdown` (Review Focus 4), `no_inou_device`, `bound_driver_is_named` (a symlink `physical_node/driver` → `…/uniwill` gives `DriverInCharge { driver: "uniwill" }`), `ready_returns_the_base`.
- [ ] **Step 2: Run** `wsl -e bash scripts/wsl-cargo.sh test --bin uucc-helper` — Expected: FAIL.
- [ ] **Step 3: Implement** `ec.rs`.
- [ ] **Step 4: Run** — Expected: PASS.
- [ ] **Step 5: Commit** `Linux helper: finds the EC window in the DSDT and checks it can be used`.

### Task 4: Helper BIOS variable

**Files:**
- Create: `src/bin/uucc-helper/efivar.rs`

**Interfaces:**
- Produces: `pub const VARIABLE: &str = "/sys/firmware/efi/efivars/UniWillVariable-9f33f85c-13ca-4fd1-9c4a-96217722c593";`
  `pub trait Immutable { fn get(&self, file: &File) -> io::Result<bool>; fn set(&self, file: &File, on: bool) -> io::Result<()>; }` — `struct Ioctl` implements it with `FS_IOC_GETFLAGS`/`FS_IOC_SETFLAGS` and `FS_IMMUTABLE_FL` (`0x10`).
  `pub fn read(path: &Path) -> io::Result<(u32, Vec<u8>)>` — first 4 bytes LE attributes, then the data; error unless the data is 180 bytes.
  `pub fn write(path: &Path, attributes: u32, bytes: &[u8], flags: &impl Immutable) -> io::Result<()>` — reject unless `bytes.len() == 180`; open read-write; remember the flag; clear it; write attributes + data in one `write_all` at offset 0; restore the flag even when the write failed; return the write's result.

- [ ] **Step 1: Failing tests** with a temp file and `struct FakeFlag(Cell<bool>, Cell<u32>)` (records set calls): `reads_attributes_and_data`; `wrong_size_is_rejected` (179 bytes → error, file untouched); `write_clears_and_restores_the_flag` (starts immutable → after write immutable again, two `set` calls, file = attrs + data); `flag_restored_when_write_fails` (a fake whose file is opened read-only, or a path that's a directory, → error and the flag restored).
- [ ] **Step 2: Run** — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — Expected: PASS.
- [ ] **Step 5: Commit** `Linux helper: reads and writes UniWillVariable`.

### Task 5: Helper server, polkit and systemd units

**Files:**
- Create: `src/bin/uucc-helper/server.rs`, `src/bin/uucc-helper/polkit.rs`, `packaging/linux/uucc-helper.socket`, `packaging/linux/uucc-helper.service`, `packaging/linux/io.github.thnorty.uucc.policy`
- Modify: `src/bin/uucc-helper/main.rs`, `Cargo.toml` (Linux deps: `zbus = { version = "5", default-features = false, features = ["blocking-api"] }`)

**Interfaces:**
- Consumes: Task 2 protocol and `rules`, Task 3 `check`/`EcWindow`/`EcAccess`, Task 4 `efivar`.
- Produces: `pub struct Session { releases: Releases }`; `pub fn handle(request: Request, session: &mut Session, ec: Option<&mut dyn EcAccess>, status: &Result<u64, Unavailable>, variable: &Path) -> Reply` — `Status` → `Ready`/`Unavailable`; EC requests without a usable EC → `Unavailable`; a write/update/on-disconnect to a non-writable address → `Refused { message: "Register 0x… isn't one this app writes" }`; unreadable address → `Refused`; `Update` reads, masks and writes inside one call. `pub fn release(session: &mut Session, ec: &mut dyn EcAccess)` applies `releases.take()` as updates.
  `pub fn idle_should_exit(clients: usize, idle_for: Duration) -> bool` — true only when `clients == 0 && idle_for >= 60 s`.
  `polkit.rs`: `pub fn start_time(stat: &str) -> Option<u64>` (field 22 of `/proc/<pid>/stat`, parsed after the last `)`); `pub fn authorized(pid: u32, uid: u32) -> bool` — `org.freedesktop.PolicyKit1.Authority.CheckAuthorization` on the system bus with subject `("unix-process", {"pid": u32, "start-time": u64, "uid": i32})`, action `io.github.thnorty.uucc.hardware`, empty details, flags 0, cancellation id `""`; any error → false.
  `main.rs`: take the listening socket from systemd (`LISTEN_PID` = own pid and `LISTEN_FDS` = 1 → fd 3); otherwise bind `UUCC_HELPER_SOCKET` (env var, default `SOCKET`) for development. One thread per client; EC access through one `Mutex` (shared by all clients); a client's first message must match `VERSION` (else `WrongVersion`) and its peer (`SO_PEERCRED`) must pass `authorized` (else `Refused { message: "The helper only answers users signed in at this laptop." }` and close). On client EOF or error, `release`. The process exits when `idle_should_exit`. `check` runs once at start; `EcWindow::map` only when it returned a base (`MapFailed` otherwise).
- Units (copy into the files): socket `ListenStream=/run/uucc-helper.sock`, `SocketMode=0666`, `WantedBy=sockets.target`; service `ExecStart=/usr/libexec/uucc-helper`, `ProtectSystem=strict`, `ProtectHome=yes`, `PrivateNetwork=yes`, `NoNewPrivileges=yes`, `DevicePolicy=closed`, `DeviceAllow=/dev/mem rw`, `ReadWritePaths=/sys/firmware/efi/efivars`, `CapabilityBoundingSet=CAP_SYS_RAWIO CAP_LINUX_IMMUTABLE`. Policy: action `io.github.thnorty.uucc.hardware`, description "Change the laptop's fans, performance modes, battery care and BIOS settings", defaults `allow_any no`, `allow_inactive no`, `allow_active yes`.

- [ ] **Step 1: Failing tests** in `server.rs` with `FakeEc` and a temp variable file: `status_reports_unavailable_reason`; `write_outside_rules_is_refused` (`0x750` → `Refused`, fake unchanged); `update_changes_only_the_mask`; `updates_from_two_sessions_keep_both_bits` (Review Focus 2: session A sets bit 2 of `0x7C6`, session B sets bit 0, both through `handle` on the same `FakeEc` → `0x05`); `disconnect_applies_releases` (`OnDisconnect 0x7C6 mask 4 value 0` after setting bit 2 → `release` clears it); `clear_on_disconnect_skips_release`; `read_many_returns_in_order`; `idle_exit_only_without_clients` (Review Focus 1: `(1, 10 min)` false, `(0, 59 s)` false, `(0, 60 s)` true). In `polkit.rs`: `start_time_survives_spaces_in_the_name` (a stat line whose comm is `(a b) c)`).
- [ ] **Step 2: Run** — Expected: FAIL.
- [ ] **Step 3: Implement** `server.rs`, `polkit.rs`, `main.rs`, and the three packaging files.
- [ ] **Step 4: Run** — Expected: PASS; `wsl -e bash scripts/wsl-cargo.sh build --bin uucc-helper` builds.
- [ ] **Step 5: Commit** `Linux helper: the socket server, polkit check and systemd units`.

### Task 6: The app's EC client

**Files:**
- Modify: `src/hardware/ec_linux.rs` (replace the stub), `src/hardware/fans.rs`

**Interfaces:**
- Consumes: Task 2 protocol; Task 5 `handle` (tests run a fake helper thread that answers with it and a `FakeEc`).
- Produces: same API as Windows `ec.rs`: `Ec::connect() -> Result<Ec>`, `run_for`, `spawn`, `Registers::{read, write, update, bit}`. `Registers::update` sends `Update` (atomic in the helper). Linux-only: `Registers::on_disconnect(&self, address: u16, mask: u8, value: u8) -> Result<()>`, `Registers::clear_on_disconnect(&self, address: u16) -> Result<()>`, `Registers::read_many(&self, addresses: &[u16]) -> Result<Vec<u8>>`. `pub struct EcUnavailable(pub String)` (an `std::error::Error`) returned by `connect` inside the `anyhow::Error` when the helper replies `Unavailable` (its `message()`), can't be reached ("The app's helper (uucc-helper) isn't installed or running: reinstall the package, or run `sudo systemctl enable --now uucc-helper.socket`."), refuses (its message), or speaks another version ("The app and its helper are different versions: reinstall the package."). The socket path is `UUCC_HELPER_SOCKET` if set, else `helper_protocol::SOCKET`. A broken connection is reopened once per call, and the disconnect actions registered so far are sent again first.
- `fans.rs`: on Linux, `set_custom(ec, true)` also calls `ec.on_disconnect(RAM_FAN_CONTROL, RAM_FAN_BIT, 0)`, and `set_custom(ec, false)` calls `ec.clear_on_disconnect(RAM_FAN_CONTROL)`. `apply_custom`'s table writes use `write` per byte as now (or `read_many` for reads where the code reads tables).

- [ ] **Step 1: Failing tests** (fake helper on a temp `UUCC_HELPER_SOCKET`; tests that set the env var run serially through one `Mutex`): `reads_and_writes_through_the_helper`; `unavailable_reason_reaches_connect` (fake status `SecureBoot` → `connect` error downcasts to `EcUnavailable` with the SecureBoot message); `missing_helper_says_how_to_fix`; `reconnects_after_helper_restart` (Review Focus 1: the fake closes the first connection after one reply; the next `read` succeeds on a new connection and the fake received the earlier `OnDisconnect` again); `custom_curve_registers_its_release` (`fans::set_custom(true)` → the fake's session has `0x7C6` registered).
- [ ] **Step 2: Run** `wsl -e bash scripts/wsl-cargo.sh test ec_linux` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — Expected: PASS; Windows `cargo test` PASS.
- [ ] **Step 5: Commit** `Linux: the app reaches the EC through uucc-helper`.

### Task 7: The BIOS variable on Linux

**Files:**
- Modify: `src/hardware/uefi.rs`

**Interfaces:**
- Consumes: Task 6's helper connection (a `pub(crate) fn request(request: Request) -> Result<Reply>` in `ec_linux.rs` for one-off requests).
- Produces: `uefi.rs` reads and writes through two private functions per platform: `fn read_raw() -> Result<(u32, Vec<u8>)>` and `fn write_raw(attributes: u32, bytes: &[u8]) -> Result<()>`. Windows: the current `GetFirmwareEnvironmentVariableExW`/`Set…` code moved into them (with the privilege step). Linux: `ReadVariable`/`WriteVariable` through the helper. `read`, `set_startup`, `set_voltage_offset`, `set_gpu_mode`, `clear_recovery` keep their signatures and rules.

- [ ] **Step 1: Failing test** (Linux, fake helper with a temp variable file holding the probe's 180 bytes and attributes 7): `read_goes_through_the_helper` (`read()` returns exactly the file's bytes and attributes); `set_gpu_mode_writes_only_its_field` (after `set_gpu_mode(Hybrid)`, the file differs from before only at the byte(s) `gpu_mode()` reads, and `read()?.gpu_mode() == Some(GpuMode::Hybrid)`).
- [ ] **Step 2: Run** — Expected: FAIL.
- [ ] **Step 3: Implement** the split; Windows behavior unchanged.
- [ ] **Step 4: Run** Linux and Windows tests — Expected: PASS.
- [ ] **Step 5: Commit** `Linux: BIOS settings through uucc-helper`.

### Task 8: Lighting over hidraw

**Files:**
- Create: `src/lighting/hid.rs` (Windows transport, moved out of `device.rs`), `src/lighting/hid_linux.rs`, `packaging/linux/70-uucc-lighting.rules`
- Modify: `src/lighting/device.rs`, `src/lighting/mod.rs`; remove `src/lighting/device_linux.rs` (the Task 1 stub)

**Interfaces:**
- Produces (both platforms): `pub struct HidDevice` with `pub fn find(vendor: u16, products: &[u16], usage_page: u16, usage: u16) -> Result<Option<HidDevice>>`, `pub fn feature_length(&self) -> usize`, `pub fn output_length(&self) -> usize` (both including the report ID byte), `pub fn set_feature(&self, report: &[u8]) -> Result<()>`, `pub fn get_feature(&self, report: &mut [u8]) -> Result<()>`, `pub fn write_output(&self, report: &[u8]) -> Result<()>`. `device.rs` keeps its commands and uses only this API.
- Linux: `pub fn descriptor_info(descriptor: &[u8]) -> DescriptorInfo { usage_page: u16, usage: u16, feature_length: usize, output_length: usize }` — first Usage Page and Usage of the top level; lengths = 1 + Σ(report size × count / 8) over Feature (`0xB1`) / Output (`0x91`) main items, tracking global Report Size (`0x75`) and Report Count (`0x95`). Enumerate `/sys/class/hidraw/*/device/uevent` (`HID_ID=0003:0000VVVV:0000PPPP`) and `report_descriptor`; open `/dev/hidrawN` read-write; feature reports with `HIDIOCSFEATURE(len)`/`HIDIOCGFEATURE(len)` (`_IOC(_IOC_READ|_IOC_WRITE, 'H', 0x06/0x07, len)`), output with `write`. Opening a matching device with `EACCES` fails with: "No permission to open {path}. Install the package (its udev rule gives you access to the lighting controllers), then sign out and back in."
- udev rule (copy): `SUBSYSTEM=="hidraw", ATTRS{idVendor}=="048d", ATTRS{idProduct}=="ce00|6000|6001|6002|6003|6004|6005|6006|6007|6008|600a|6010", TAG+="uaccess"`

- [ ] **Step 1: Failing tests** (Linux): `keyboard_descriptor` (`0612ff0901a101150026ff0075089540092081020921910209229508b102c0` → `0xFF12`, `1`, feature `9`, output `65`); `light_bar_descriptor` (`0603ff…` same lengths, page `0xFF03`); `permission_error_names_the_fix` (Review Focus 3: the error built from `EACCES` for `/dev/hidraw3` contains "/dev/hidraw3" and "udev rule"). Windows: the existing lighting behavior is covered by building and the Task 15 checklist.
- [ ] **Step 2: Run** — Expected: FAIL.
- [ ] **Step 3: Implement**: first move the Windows transport into `hid.rs` (no behavior change), then `hid_linux.rs` and the rule.
- [ ] **Step 4: Run** Linux and Windows tests; on Windows run the app once and check the keyboard and light bar still light (owner) — Expected: PASS / lights work.
- [ ] **Step 5: Commit** `Lighting: the HID transport split out, and hidraw on Linux`.

### Task 9: Readings on Linux

**Files:**
- Modify: `src/hardware/status.rs` (platform parts moved behind functions), `src/hardware/gpu_linux.rs`, `src/hardware/system_linux.rs`, `src/app/monitor.rs`
- Create: `src/hardware/status_linux.rs` (readers), `src/hardware/nvml_linux.rs`

**Interfaces:**
- Produces: `SystemStatus::update`, `power_now`, `on_ac`, `memory` unchanged in signature. `pub struct BatteryReader` with `new()` and `read(&mut self) -> BatteryDetails` on both platforms (Windows keeps its cached `WMIConnection` inside); `monitor.rs` uses it instead of its own WMI connection.
  Linux parsers (pure, tested): `fn cpu_times(proc_stat: &str) -> Option<(u64, u64)>` (idle+iowait, total of the `cpu` line); `fn mem_used_total(meminfo: &str) -> Option<(u64, u64)>` (bytes: `MemTotal − MemAvailable`, `MemTotal`); `fn hottest_acpitz(root: &Path) -> Option<f32>` (`thermal_zone*/type` = `acpitz`, `temp` m°C; fallback `hwmon*/name` = `coretemp`, `temp1_input`); `fn battery(root: &Path) -> (Option<u8>, bool, bool)` and `fn battery_details(root: &Path) -> BatteryDetails` over `power_supply/*` (`type` Battery/Mains; mWh from `energy_*` µWh, or `charge_*` µAh × `voltage_min_design` µV; rate from `power_now` or `current_now × voltage_now`, negative when `status` is Discharging; `cycle_count` 0 → None). `gpu::nvidia_on() -> Option<bool>` over `root/bus/pci/devices/*` (`vendor` `0x10de`, `class` starting `0x03`): `power_state` `D0` → `Some(true)`, `D3*` → `Some(false)`; no such device → `Some(false)`. `system::laptop_model()` from `dmi/id`. Production calls pass `/proc` and `/sys`.
  `nvml_linux.rs`: the same `Nvml` API as Windows (`load() -> Option<Nvml>`, `temperature(&mut self) -> Option<f32>`) over `libloading` and `libnvidia-ml.so.1` (`nvmlInit_v2`, `nvmlDeviceGetHandleByIndex_v2`, `nvmlDeviceGetTemperature`). `Cargo.toml` Linux deps: `libloading = "0.9"`.

- [ ] **Step 1: Failing tests** with fixture strings and temp trees: `cpu_line_parses` (`cpu  100 0 50 800 50 0 0 0 0 0` → `(850, 1000)`); `memory_uses_available`; `hottest_acpitz_wins` (zones 52000 and 61000 → 61.0); `coretemp_when_no_acpitz`; `battery_from_charge_and_voltage` (this laptop's values: `charge_full_design 4100000`, `charge_full 2500000`, `voltage_min_design 11400000` → designed 46740 mWh, full 28500 mWh); `discharge_rate_is_negative`; `no_nvidia_device_reads_as_off` (Review Focus 5); `nvidia_d3cold_is_off`; `nvidia_d0_is_on`.
- [ ] **Step 2: Run** — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** Linux and Windows tests — Expected: PASS.
- [ ] **Step 5: Commit** `Linux: temperatures, CPU load, memory, battery and GPU state from /proc and /sys`.

### Task 10: Shell basics on Linux: one instance, dialogs, autostart, theme

**Files:**
- Modify: `src/shell/instance_linux.rs`, `src/shell/mod.rs` (Linux `open_path`, `pick_file`, `pick_save_file`, `confirm`), `src/hardware/system_linux.rs` (`startup`, `dark_mode`, `accent_color`), `src/main.rs`, `Cargo.toml` (`rfd = { version = "0.17", default-features = false, features = ["xdg-portal"] }`)

**Interfaces:**
- Produces: `instance::acquire() -> bool` — `flock(LOCK_EX | LOCK_NB)` on `$XDG_RUNTIME_DIR/unofficial-uniwill-control-center.lock` (demo: `…-demo.lock`), then bind a control socket beside it (`….sock`); a second start connects and sends `show`, then returns false. `instance::close_running(wait) -> bool` sends `exit` and waits for the lock to free. `instance::control_messages() -> Receiver<Control>` with `enum Control { Show, Exit }`, consumed by the tray in Task 11. `is_elevated()` → true on Linux; `open_elevated` unused.
  `system::startup` on Linux: `enable()` writes `$XDG_CONFIG_HOME/autostart/unofficial-uniwill-control-center.desktop` (`[Desktop Entry]`, `Type=Application`, `Name=Unofficial Uniwill Control Center`, `Exec=<current exe> --tray`, `Icon=unofficial-uniwill-control-center`, `X-GNOME-Autostart-enabled=true`); `disable()` deletes it; `is_enabled()`/`starts_this_exe()` read it; `run_now()` true; `migrate`/`remove_legacy_entries` do nothing.
  `dark_mode()`/`system_dark_mode()`: the portal setting `org.freedesktop.appearance color-scheme` (1 = dark) over zbus, falling back to false; `accent_color()`: the portal's `accent-color` if present, else `None`.
  `main.rs` Linux: `accept_risk` uses `rfd::MessageDialog` with the same text plus the sentence "On Linux it reaches the embedded controller through a small helper that runs as root." inserted after the sentence ending "embedded controller."; crash and start errors show `rfd` error dialogs.

- [ ] **Step 1: Failing tests** (Linux, temp `XDG_RUNTIME_DIR`/`XDG_CONFIG_HOME`, serial): `second_acquire_signals_show` (first `acquire` true; second false; the first's `control_messages` receives `Show`); `close_running_sends_exit`; `autostart_file_has_tray_flag` (`enable()` then the file contains `Exec=` + exe + ` --tray`); `disable_removes_autostart`; `risk_text_mentions_the_helper`.
- [ ] **Step 2: Run** — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** Linux and Windows tests — Expected: PASS.
- [ ] **Step 5: Commit** `Linux: one instance, start at login, desktop dialogs and the system theme`.

### Task 11: Tray, notifications and power events on Linux

**Files:**
- Modify: `src/shell/tray_linux.rs`, `Cargo.toml` (`ksni = "0.3"`, blocking feature as its docs require)

**Interfaces:**
- Consumes: Task 10 `instance::control_messages()`.
- Produces: the Windows `Tray` API on Linux: `Tray::create(tip, menu, event) -> Result<Tray>`, `set_icon(Icon)`, `set_hidden(bool)`, `restore()`, `notify(title, text)`; `TrayEvent` unchanged. The `menu` and `event` closures stay on the UI thread (thread-local, as on Windows); the ksni thread and the watcher threads post to it with `slint::invoke_from_event_loop`. The menu is rebuilt from `menu()` whenever the app calls `restore()` or `set_icon` (ksni `Handle::update`); `MenuItem::check` maps to a checkmark item; a left click (`activate`) sends `Open`. `notify` uses `org.freedesktop.Notifications.Notify` over zbus (app name "Unofficial Uniwill Control Center", 5 s). Events: `Control::Show` → `Open`, `Control::Exit` → `Exit`; logind `PrepareForSleep(false)` on the system bus → `Resumed`; the Mains `online` value (Task 9 reader) polled once a second, a change → `PowerChanged`; the portal's `SettingChanged` for `color-scheme` → `ThemeChanged`. `make_icon(pixels, size) -> Icon` converts RGBA to ARGB32 big-endian as StatusNotifierItem wants.

- [ ] **Step 1: Failing tests**: `icon_pixels_become_argb` (one red opaque pixel `[255,0,0,255]` → `[255,255,0,0]`); `menu_items_map_to_ksni` (an item, a checked check, a disabled check → their ksni equivalents in order); `ac_change_is_reported_once` (a pure `fn ac_changed(last: &mut Option<bool>, now: bool) -> bool`: first call false, same value false, a change true).
- [ ] **Step 2: Run** — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** tests — Expected: PASS. Manual (WSLg has no tray; on the live USB in Task 15): icon, menu, click, notifications.
- [ ] **Step 5: Commit** `Linux: the tray, notifications, and sleep and charger events`.

### Task 12: The app on Linux: hidden features, EC status, fan safety, icons

**Files:**
- Modify: `ui/theme.slint` (a `Platform` global or properties on `Theme`), `ui/app.slint`, `ui/pages.slint`, `ui/power.slint`, `ui/setups.slint`, `ui/plugins.slint`, `ui/display.slint` as needed, `src/app/mod.rs`, `src/app/power.rs`, `src/app/cli.rs`, `src/bin/uucc/args.rs`
- Create: `ui/fonts/FluentSystemIcons-Regular.ttf` (from github.com/microsoft/fluentui-system-icons, MIT; add its `LICENSE` beside it as `ui/fonts/FluentSystemIcons-LICENSE.txt`)

**Interfaces:**
- Consumes: Task 6 `EcUnavailable`, Task 11 tray.
- Produces: UI global `Platform { in property <bool> linux; in property <string> ec-unavailable; }`. When `linux`: the Display and Plugins pages are left out of the navigation; Spectrum, Music and Typing tiles are hidden; the idle timer, pop-up switches, Caps/Num Lock indicators, Windows key lock, touchpad, app profiles and Control Center notes are hidden; "Start with Windows" reads "Start at login". When `ec-unavailable` is not empty: the Performance and Battery pages show that text (with a "Linux setup" link to the README's Linux section on GitHub) instead of their controls; the tray menu leaves out its mode and Fan Boost items; `uucc` EC commands reply exit code 3 with that text. On Linux the app doesn't start the hotkeys, the plugin host, display colors or app profiles. `uucc display` on Linux: exit code 1 with "Display color modes aren't available on Linux", and `uucc help` leaves it out.
  Fan safety: on Linux, while a custom curve is on, `power.rs` reads `fans::safety_protection` once a second (on the existing timer) and runs the same code as hotkey code 187.
  Icons: on Linux `Theme.icon-font` is "FluentSystemIcons-Regular" (imported in `app.slint` with `import "fonts/FluentSystemIcons-Regular.ttf";`) and each of the 38 glyph properties gets the Fluent System Icons code point of the same icon (looked up in the font's `FluentSystemIcons-Regular.json`); Windows keeps Segoe Fluent Icons. Text fonts on Linux: leave `Theme.font`/`display-font` empty so the system font is used.

- [ ] **Step 1: Failing tests**: `display_command_unavailable_on_linux` (`args`/`cli` test, `#[cfg(target_os = "linux")]`); `ec_commands_report_unavailable_reason` (`app/cli.rs` handler with `ec-unavailable` set → `Reply` failure code 3 with the text); `safety_check_runs_only_with_custom_curve` (a pure decision fn in `power.rs`).
- [ ] **Step 2: Run** — Expected: FAIL.
- [ ] **Step 3: Implement**; check every glyph renders in the WSLg demo window (`… run --bin UnofficialUniwillControlCenter -- --demo`) and that the hidden pages are gone; set `ec-unavailable` by hand in demo to see the Performance page message.
- [ ] **Step 4: Run** Linux and Windows tests; open the Windows app once (owner) to confirm nothing hidden there — Expected: PASS; Windows unchanged.
- [ ] **Step 5: Commit** `Linux: the app hides what Linux lacks, shows why the EC is off, and draws its icons`.

### Task 13: uucc over a Unix socket

**Files:**
- Modify: `src/cli/server_linux.rs`, `src/bin/uucc/connect_linux.rs`, `src/cli/protocol.rs` (`socket_path(demo) -> PathBuf` on Linux next to `pipe_name`)

**Interfaces:**
- Produces: `cli::server::start(name, handle)` on Linux binds `$XDG_RUNTIME_DIR/uucc.sock` (demo `uucc-demo.sock`), removes a stale socket first, `chmod 0600`, and serves clients one at a time with the shared `read_message`/`write_message` and the same 5 s read timeout. `connect::send`/`send_with_timeout` on Linux: `NotRunning` when the app's lock (Task 10) isn't held; `NoPipe` when it's held but the socket refuses; `--start` runs `unofficial-uniwill-control-center --tray` (found next to `uucc`, else on `PATH`) and waits up to 15 s for the socket. `Failure::message` on Linux: `NoPipe` names `~/.config/UnofficialUniwillControlCenter/uucc.log`; `DidntStart` says "open it once from your app menu" instead of "from Start".

- [ ] **Step 1: Failing tests** (temp `XDG_RUNTIME_DIR`, serial): `round_trip_over_the_socket` (server with an echo handler; `send_with_timeout` returns its reply); `socket_is_private` (mode `0600`); `not_running_without_lock`; `stale_socket_is_replaced`.
- [ ] **Step 2: Run** — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** Linux and Windows tests; on Windows run `scripts/cli-e2e.ps1` — Expected: PASS, 33/33.
- [ ] **Step 5: Commit** `uucc on Linux: the app's socket in the user's runtime folder`.

### Task 14: Packages

**Files:**
- Create: `packaging/linux/unofficial-uniwill-control-center.desktop`, `packaging/linux/postinst`, `packaging/linux/prerm`, `packaging/linux/postrm`, `scripts/build-linux.sh`
- Modify: `Cargo.toml` (`[package.metadata.deb]`, `[package.metadata.generate-rpm]`)

**Interfaces:**
- Consumes: the files from Tasks 5 and 8.
- Produces: `scripts/build-linux.sh` runs `cargo build --release` for the three binaries, then `cargo deb --no-build` and `cargo generate-rpm`, and copies `unofficial-uniwill-control-center_3.5.0-1_amd64.deb` and `unofficial-uniwill-control-center-3.5.0-1.x86_64.rpm` to `dist/`. Contents and paths exactly as the spec's Packaging table (app renamed to `/usr/bin/unofficial-uniwill-control-center`, icon from `res/app.png` at `/usr/share/icons/hicolor/256x256/apps/unofficial-uniwill-control-center.png`). Scripts: after install `systemctl daemon-reload; systemctl enable --now uucc-helper.socket; udevadm control --reload; udevadm trigger --subsystem-match=hidraw`; before removal `systemctl disable --now uucc-helper.socket uucc-helper.service`; after removal `systemctl daemon-reload`. Dependencies: deb `libfontconfig1, libxkbcommon0, polkitd | policykit-1, systemd`; rpm `fontconfig, libxkbcommon, polkit, systemd`. Desktop entry: `Name=Unofficial Uniwill Control Center`, `Exec=unofficial-uniwill-control-center`, `Icon=unofficial-uniwill-control-center`, `Categories=Settings;HardwareSettings;`.

- [ ] **Step 1: Install the tools** in WSL: `cargo install cargo-deb cargo-generate-rpm`.
- [ ] **Step 2: Write the check first**: `scripts/build-linux.sh --check` lists both packages' files (`dpkg-deb -c`, `rpm -qlp`) and fails unless all spec paths are present. Run it — Expected: FAIL (no packages).
- [ ] **Step 3: Implement** the metadata, scripts and desktop file.
- [ ] **Step 4: Run** `wsl -e bash scripts/build-linux.sh` then `--check` — Expected: both packages built; every path present; `dpkg-deb -I` shows the dependencies.
- [ ] **Step 5: Commit** `Linux packages: .deb and .rpm with the helper, its units, the polkit action and the udev rule`.

### Task 15: Docs, version and the hardware checklist

**Files:**
- Modify: `README.md`, `FEATURES.md`, the uucc section of the README, `Cargo.toml` (`version = "3.5.0"`), `installer/UnofficialUniwillControlCenter.iss` (version)
- Create: `docs/linux-hardware-test.md`

**Interfaces:**
- Produces: README **Linux** section per the spec's Docs (supported distributions and glibc 2.39+, installing each package, what works and what's Windows-only for now, the tray on GNOME, **Secure Boot** with `mokutil --sb-state` and that a kernel module is planned). FEATURES.md Linux notes (Fn keys, pop-ups, power limit ranges, display colors). The checklist file lists the spec's on-laptop checks, each with what to do and what to see, including backing up the BIOS variable (`sudo cp /sys/firmware/efi/efivars/UniWillVariable-… ~/`) before any BIOS write, and the `kill -9` check (`0x7C6` bit 2 off afterwards, read with the probe or `uucc status`).

- [ ] **Step 1: Write the docs and the checklist.**
- [ ] **Step 2: Run** Windows `cargo test` and the Linux tests once more — Expected: PASS.
- [ ] **Step 3: Commit** `Docs: Linux support, Secure Boot, and a hardware test checklist; version 3.5.0`.
- [ ] **Step 4: Hardware run (owner)**: copy both packages to the Ventoy stick, boot Fedora, `sudo dnf install ./unofficial-uniwill-control-center-3.5.0-1.x86_64.rpm`, and work through `docs/linux-hardware-test.md`. Fix what fails, each fix with its test first, and commit per fix.
