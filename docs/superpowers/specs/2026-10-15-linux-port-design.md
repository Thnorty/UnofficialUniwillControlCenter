# Linux support for Unofficial Uniwill Control Center

Date: 2026-10-15. Target version: 3.5.0.

## Goal

Run the app on Linux, first for the owner on Fedora (KDE Plasma, Wayland) on the Monster Tulpar T5 V21.4, and also for
other people on Ubuntu, Debian and other Uniwill laptops, now that the repository is public.

Success: on Fedora with Secure Boot off, the owner installs one package, and the modes, Fan Boost, profiles, power
limits, fan curves, battery care, the keyboard and light bar lighting, the BIOS settings and `uucc` work the way they
do on Windows. On a laptop where something can't work (Secure Boot on, another EC driver in charge, no EC device), the
app says what and why instead of failing, and everything else still works.

The Windows build keeps working from the same code, with no user-visible change.

## Findings that shaped the design

A read-only probe ran on the laptop from a Fedora 44 KDE live USB (kernel 6.19.10, Secure Boot off):

- **The EC is memory-mapped.** The firmware's EC methods on the `INOU0000` device (`\_SB.INOU.ECRR` / `ECRW`, the
  device Control Center's `\\.\ACPIDriver` binds to on Windows) read and write the byte at `0xFE410000 + address`. As
  root, mapping that page from `/dev/mem` read the expected values: Office mode and High capacity charging, matching
  the Windows settings, and both fans' live RPM. `/proc/iomem` shows the range as `pnp 00:06`, which Fedora's
  `CONFIG_IO_STRICT_DEVMEM` doesn't block.
- **Lighting:** the keyboard (`048D:CE00`, usage page `0xFF12`, firmware 11.04) and the light bar (`048D:6005`, usage
  page `0xFF03`, firmware 15.04) are `hidraw` devices on `hid-generic`, and both answered the firmware-version query
  over `HIDIOCSFEATURE`/`HIDIOCGFEATURE`.
- **BIOS variable:** `UniWillVariable-9f33f85c-13ca-4fd1-9c4a-96217722c593` is readable in efivarfs (180 bytes,
  attributes 7) and has the immutable flag, as efivarfs gives every variable.
- **Linux's own driver:** Fedora ships `uniwill-laptop` (Linux 6.19+). It didn't load here (Monster isn't in its DMI
  list). It reaches the same `ECRR`/`ECRW` methods, but it has no performance modes, fan curves or power limits, so it
  can't replace direct EC access.
- **Fn keys:** the firmware's WMI event GUID (`ABBC0F72-…`) is present with no driver bound, so no hotkey events reach
  user space without a driver.
- **Secure Boot:** with it on, Fedora and Ubuntu put the kernel in lockdown, which blocks `/dev/mem`. Then only a
  kernel module can reach the EC.

## Scope

**In 3.5.0 (Linux):** Office/Gaming/Turbo, Fan Boost, profiles per mode, power limits, TCC offset, GPU power, custom fan
curves with their safety rules, battery care and USB charging while off, the keyboard and light bar lighting
(firmware effects, effects the app draws, Smooth Wave, the status effects), saved setups, BIOS settings (GPU mode, core voltage offset, boot lighting, recovery), the Monitor
and Battery pages, `uucc`, the tray, start at login, backup and restore, the timer.

**Later releases:** Fn keys and pop-ups, Caps/Num Lock indicators, Spectrum, Music, Typing, the idle timer, plugins,
app profiles, a kernel module for Secure Boot, the BIOS's power limit table, a Copr/PPA.

**Never on Linux:** display color modes (Wayland has no whole-screen color matrix an app can set), coexistence with
Control Center (it doesn't exist on Linux).

Features that aren't in the Linux build are hidden (pages, tiles, settings), not shown broken.

## Approach

The app runs as the signed-in user. A small root helper does only what needs root. Rejected:

- **The whole app as root:** the tray, notifications and autostart belong to the user's session, and KDE and GNOME
  discourage root GUI apps on Wayland.
- **A root daemon with the window, tray and `uucc` as its clients:** the most Linux-like, but it means rewriting most
  of `app/mod.rs` around a new protocol, and Linux would part from the Windows design for little gain on a one-user
  laptop.

## Architecture

### Programs

| Program | Runs as | Does |
|---|---|---|
| `unofficial-uniwill-control-center` | the user | The window, tray, lighting engine, modes, fan safety rules, settings and the `uucc` server: the same app as on Windows. |
| `uucc` | the user | Unchanged apart from its transport. |
| `uucc-helper` | root, started on demand by systemd | EC reads and writes, `UniWillVariable` reads and writes, and the EC status. Nothing else. |

A udev rule gives the user at the laptop (`TAG+="uaccess"`) the two ITE lighting `hidraw` devices, so lighting doesn't
go through the helper.

### Code layout

The Windows-specific code is in `hardware/`, `shell/`, `lighting/{device,audio,keyhook}.rs`, `cli/server.rs`,
`bin/uucc/connect.rs`, `plugins/{readings,downloads,host}.rs`, `main.rs` and a few spots in `app/mod.rs`. Each piece
used on Linux gets a Linux version beside it, picked with `#[cfg(target_os = "windows")]` / `"linux"`; the
`windows`/`wmi` dependencies and `embed-resource` become Windows-only in `Cargo.toml`, and the Linux crates
Linux-only. `app/`, `settings.rs`, `color.rs`, `cli/protocol.rs`, `lighting/{effects,engine,scenes,preview}.rs` stay
shared. Modules for features that are Windows-only or later are compiled only on Windows, and the UI hides what they
drive through properties the app sets at start (for example `linux: true` in the UI's globals).

`hardware::ec` keeps its API (`Ec::connect`, `run_for`, `spawn`, `Registers::{read, write, update, bit}`). On Linux,
`Registers` is a client of the helper's socket. The code that uses it (performance, fans, battery, hotkeys) stays
shared.

`uucc-helper` is a third `[[bin]]` in the same crate, built only on Linux, at `src/bin/uucc-helper/`. Its protocol
types live in a module both the helper and the app compile.

### The helper

**Socket.** `/run/uucc-helper.sock`, opened by `uucc-helper.socket` (systemd socket activation, `SocketMode=0666`);
the service exits after 60 s with no clients. Requests and replies are JSON lines, like `uucc`'s protocol, with a
version field; a different version gets a clear error.

**Who may use it.** For each connection the helper takes the peer's pid and uid (`SO_PEERCRED`) and asks polkit
(`org.freedesktop.PolicyKit1.Authority.CheckAuthorization`, subject `unix-process` with the pid's start time) for the
action `io.github.thnorty.uucc.hardware`, whose defaults are `allow_active: yes`, `allow_inactive: no`,
`allow_any: no`: a user signed in at the laptop is allowed without a password; remote and background users are
refused. Admins can change that with a polkit rule.

**Requests.**

| Request | Reply |
|---|---|
| `Status` | `Ready`, or `Unavailable` with a reason: `SecureBoot` (lockdown is `integrity` or `confidentiality`), `NoEcDevice` (no `INOU0000` or no `ECRR` base found), `DriverInCharge` (`uniwill-laptop` is bound to `INOU0000`), `MapFailed` (with the OS error). |
| `Read { address }` | the byte. |
| `ReadMany { addresses }` | the bytes, for the fan tables and the monitor. |
| `Write { address, value }` | ok, or refused. |
| `Update { address, mask, value }` | ok: a read-modify-write done in the helper, so it can't interleave with another client's. |
| `OnDisconnect { address, mask, value }` / `ClearOnDisconnect { address }` | ok. |
| `ReadVariable` | the attributes and 180 bytes of `UniWillVariable`. |
| `WriteVariable { attributes, bytes }` | ok, or the error. |

**EC window.** The base address is read from the DSDT (`/sys/firmware/acpi/tables/DSDT`): the `ECRR` method's body is
`Add (DWordConst base, Arg0, Local0)`, AML `14 <pkglen> "ECRR" 01 72 0C <base, 4 bytes LE> 68 …`. The helper finds the
method, checks those bytes, and takes the base; without a match the status is `NoEcDevice`. It maps one 4 KB page
from `/dev/mem` (`O_SYNC`, `MAP_SHARED`) and reads and writes single bytes with volatile accesses, as `MMRW` does
with `ByteAcc`. All EC access is serialized in one thread, as on Windows.

**Writable registers** (everything the app writes today; anything else is refused):
`0x741` (app present), `0x743`–`0x746` (GPU power), `0x751` (mode and Fan Boost), `0x767` (trigger byte), `0x783`–`0x787`
(power limits, TCC, fan switch speed), `0x7A6` (battery care), `0x7C5`, `0x7C6` (fan table control), `0xF00`–`0xF5F`
(fan tables and their request bytes). Reads are allowed anywhere in `0x000`–`0xFFF`. The list lives in one place in the
helper and has a test.

**Release on disconnect.** When the app turns on something that must not outlive it, it registers what undoes it:
custom fan curve on → `OnDisconnect { 0x7C6, mask 0x04, value 0 }` (the fans go back to the EC's curve); later, when Fn
keys arrive, the app-present bit. When the connection closes without a `ClearOnDisconnect`, after a crash or a kill,
the helper applies them. A clean exit undoes them itself first, as `hardware::release_current` does now.

**BIOS variable.** `ReadVariable` reads the efivarfs file (4 bytes of attributes, then the data). `WriteVariable`
clears the immutable flag (`FS_IOC_GETFLAGS`/`FS_IOC_SETFLAGS`), writes attributes and data in one `write`, and
restores the flag, even on failure. The variable's name and GUID are fixed in the helper. The shared `uefi` code keeps
its rules: read first, change only its own fields, the same size and attributes, read back to check.

### Inside the app on Linux

- **EC status:** the app asks the helper for `Status` at start. Unless `Ready`, the Performance and Battery pages show
  why (one line each, with a link to the README's Linux section) instead of their controls, the tray omits the mode
  items, and `uucc` replies to EC commands with exit code 3 and the reason. If the helper isn't installed or the socket
  refuses, the reason says so.
- **Lighting** (`lighting/device.rs`): enumerate `/sys/class/hidraw/*`, match vendor `048D`, the product IDs and
  the first usage page and usage of the report descriptor (`0xFF12`/`0xFF03`, usage 1), open `/dev/hidrawN`
  read-write, and send feature reports with `HIDIOCSFEATURE`/`HIDIOCGFEATURE` (9 bytes, report ID 0) and output
  reports with `write`. The feature and output report lengths come from the report descriptor. The commands are
  unchanged.
- **Readings** (`hardware/status.rs`, `gpu.rs`, `system.rs`), all without root:

  | Reading | Source |
  |---|---|
  | CPU temperature | the hottest `/sys/class/thermal/thermal_zone*` of type `acpitz`, as Control Center's counter on Windows; `coretemp`'s package temperature if there is none |
  | CPU load | `/proc/stat` |
  | Memory | `/proc/meminfo` |
  | Battery, AC | `/sys/class/power_supply` (`type` Battery/Mains): capacity, status, energy or charge now/full/design, power or current, voltage, cycle count |
  | GPU power state | `power_state` of the NVIDIA PCI function (vendor `0x10de`, class `0x03xxxx`); none present (Intel-only GPU mode) reads as off |
  | GPU temperature | NVML from `libnvidia-ml.so.1`, loaded at run time, only while the GPU is on, as on Windows |
  | Laptop maker and model | `/sys/class/dmi/id/sys_vendor` and `product_name` |

- **Set again after sleep and charger changes:** logind's `PrepareForSleep(false)` on the system bus, and the AC
  adapter's `online` value checked once a second (cheap, no root). They trigger the same code Windows' power messages
  do.
- **Fan safety without events:** while a custom curve is on, the app reads `0x7A5` bit 3 once a second and reacts as
  it does to event 187.
- **Fn keys:** not taken over in 3.5.0; the app doesn't set the app-present bit, so the firmware handles the
  keyboard-light keys itself. The brightness the app shows follows the controller, as it does on Windows after Fn
  keys were used while the app was closed.
- **Power limit ranges:** without Control Center's driver, the existing fallback applies (each limit up to the highest
  mode's own).

### Desktop integration

| On Windows | On Linux |
|---|---|
| Tray (Shell_NotifyIcon) | StatusNotifierItem over D-Bus (`ksni`), same menu and colored icon. KDE shows it; Ubuntu's GNOME shows it through its AppIndicator extension; stock GNOME needs that extension (README). |
| Toast notifications | freedesktop notifications over D-Bus. |
| Pop-ups (OSD) | Not in 3.5.0 (they come with the Fn keys). Whatever showed a pop-up for a tray, `uucc` or setup action shows nothing extra. |
| Start with Windows | `~/.config/autostart/unofficial-uniwill-control-center.desktop` with `--tray`, from the same Settings switch and `--enable-startup` / `--disable-startup`. |
| Single instance, `--exit` | A lock (`flock`) on `$XDG_RUNTIME_DIR/unofficial-uniwill-control-center.lock` and a control socket beside it: a second start asks the running app to show its window; `--exit` asks it to quit and waits up to 30 s. |
| `uucc` named pipe | `$XDG_RUNTIME_DIR/uucc.sock`, mode 0600 (that folder is the user's only). Same protocol, commands, `--json`, exit codes; `NoPipe` becomes "no socket". `--start` runs the app with `--tray`. |
| Elevation (UAC relaunch) | None: the app runs as the user. |
| File pickers, message boxes | The desktop's own dialogs through the XDG portal (`rfd`). The crash and start-error boxes too. |
| First-start warning | Same wording, plus: "On Linux it reaches the embedded controller through a small helper that runs as root." |
| Fonts | The system's UI font for text. The 38 icon glyphs come from Microsoft's **Fluent UI System Icons** font (MIT), embedded with Slint's font import; `Theme`'s icon properties get Linux code points. Segoe Fluent Icons is never shipped. |
| Title bar colors (DWM) | Skipped; KDE draws the title bar. |
| Settings folder | `$XDG_CONFIG_HOME/UnofficialUniwillControlCenter` (default `~/.config/…`), the same files and backup format, so a backup moves between the two. The `%APPDATA%` migrations stay Windows-only. |
| Opening files and folders | `xdg-open`. |

## Packaging

Built in WSL Ubuntu 24.04, so the binaries need glibc 2.39 or newer (Fedora 40+, Ubuntu 24.04+, Debian 13). Each
release attaches `unofficial-uniwill-control-center-3.5.0-1.x86_64.rpm` (`cargo-generate-rpm`) and
`unofficial-uniwill-control-center_3.5.0-1_amd64.deb` (`cargo-deb`) to the same GitHub release as the Windows
installer. They contain:

| File | Path |
|---|---|
| App | `/usr/bin/unofficial-uniwill-control-center` |
| CLI | `/usr/bin/uucc` |
| Helper | `/usr/libexec/uucc-helper` |
| systemd units | `/usr/lib/systemd/system/uucc-helper.{socket,service}` |
| polkit action | `/usr/share/polkit-1/actions/io.github.thnorty.uucc.policy` |
| udev rule | `/usr/lib/udev/rules.d/70-uucc-lighting.rules` |
| Desktop entry, icon | `/usr/share/applications/…desktop`, `/usr/share/icons/hicolor/256x256/apps/…png` |

After install: `systemctl daemon-reload`, `systemctl enable --now uucc-helper.socket`, `udevadm control --reload` and
`udevadm trigger` for the two devices. Uninstall stops and disables the socket and removes the files; the user's
settings and autostart entry stay, as is usual for Linux packages.

The service is sandboxed: `ProtectSystem=strict`, `ProtectHome=yes`, `PrivateNetwork=yes`, `NoNewPrivileges=yes`,
`DevicePolicy=closed` with `DeviceAllow=/dev/mem rw`, `ReadWritePaths=/sys/firmware/efi/efivars` (the only writable
path), and `CapabilityBoundingSet=CAP_SYS_RAWIO CAP_LINUX_IMMUTABLE` (`/dev/mem`, and the efivarfs immutable flag).

## Docs

- **README:** a **Linux** section: supported distributions, installing the `.rpm` or `.deb`, what works and what's
  Windows-only for now, the tray on GNOME, and **Secure Boot**: with it on, the kernel's lockdown blocks the EC, so
  modes, fans, power limits and battery care need it off in 3.5.0 (check with `mokutil --sb-state`); lighting, BIOS
  settings and `uucc` work either way; a kernel module for Secure Boot is planned.
- **FEATURES.md:** Linux notes where behavior differs (Fn keys, pop-ups, power limit ranges, display colors).
- **uucc docs:** the socket instead of the pipe. On Linux `uucc display` replies with exit code 1 and "Display color
  modes aren't available on Linux", and `uucc help` leaves it out.

## Testing

- **Unit tests** run on Windows and, through WSL, on Linux. New Linux tests:
  - the helper's write allowlist (every register the app writes is allowed; neighbors aren't);
  - release on disconnect: registered actions run on a drop, not after `ClearOnDisconnect`;
  - finding the `ECRR` base in an AML sample (the method's bytes from this laptop's DSDT; a method with a different
    body gives no base);
  - the efivarfs write sequence against a temporary file (flag cleared, data written, flag restored on success and on
    failure), with the flag calls behind a small trait;
  - `hidraw` matching from a report descriptor (the two descriptors the probe recorded);
  - the Linux readers parsing sample `/proc/stat`, `/proc/meminfo` and `power_supply` contents.
- **Demo mode** (`--demo`) under WSLg to check the window, the icons and the hidden pages without hardware.
- **On the laptop** (Fedora live USB, `sudo dnf install ./….rpm`, gone after reboot), a written checklist: each mode
  with the fans and PLs following; Fan Boost; a fan curve on and off; battery care; USB charging; keyboard and light
  bar effects incl. Smooth Wave and status effects; reading the BIOS settings (writing them only after a backup of the
  variable); `uucc status` and a few changes; the tray menu; killing the app (`kill -9`) with a custom curve on and
  checking `0x7C6` bit 2 went off; Secure Boot detection by `Status` (unit-tested with a fake lockdown file, since the
  live USB has Secure Boot off).
- **Windows:** the existing tests and `scripts/cli-e2e.ps1` still pass; a release build of the installer is smoke-tested.

## Out of scope

Everything in "Later releases" above; ARM and 32-bit builds; Flatpak/Snap/AppImage (sandboxes and a root helper don't
mix well); X11-only features; translations.
