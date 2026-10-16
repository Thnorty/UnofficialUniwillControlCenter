# Fn keys and the performance key on Linux

Status: draft for review, 2026-10-16. Builds on [the Linux port](2026-10-15-linux-port-design.md).

## Goal

On Linux, the laptop's hotkeys reach the app the way they do on Windows: the keyboard light keys (down, up, cycle),
the light bar key on laptops that have one, the performance key (also while a custom fan curve is on), and the EC's own reports (Fan Boost changed, Fan Boost off,
fan safety protection). With that, the app no longer reads the EC every second to follow the mode and the fan safety
protection.

Success: on the user's Monster Tulpar under Fedora 44 (kernel 6.19), every press of those keys acts within a fraction
of a second, with or without a custom curve on, and the app does no per-second EC reads while the hotkeys work.

## What the hardware testing showed

From the laptop's DSDT and three read-only probes on the Fedora live USB (October 16):

- A hotkey makes the EC raise query `_Q53`. Linux's EC driver always runs it. `_Q53` reads the key's code from the EC's
  ACPI-space byte `0x7C` (`OSDF`), hands it to the WMI device (`AMW0.WMBC`), which stores it and raises WMI event
  `0xD2`; about 20 ms later `_Q53` clears `OSDF`.
- With no driver bound to the WMI event, the kernel's WMI core (`drivers/platform/wmi/core.c`) still evaluates `_WED`
  and sends an ACPI netlink event: class `wmi`, the WMI device's ACPI name (`PNP0C14:00`), event `0xd2`. Every press
  produced exactly one (9 of 9 with the app-present bit set).
- The EC's ACPI space shows in the EC window the helper maps at offset `0x400`: `OSDF` is window `0x47C` (as the fan
  speeds `0x464`/`0x465` are ACPI `0x64`/`0x65`). Around each event `0x47C` held the code from 23–45 ms before the
  netlink event until 24–53 ms after it.
- With the app-present bit (`0x741` bit 0) clear, the keyboard light keys produce nothing at all, and the performance
  key switches the mode in the EC itself and reports `0xBC` (188, Control Center's "WhisperUpdate"). With the bit set,
  the firmware leaves those keys to the app (Control Center's codes: 176 performance key, 177/178 keyboard light
  down/up, 185 cycle). The bit is also what makes the custom fan curve stick.
- Fedora's kernel has the upstream `uniwill-laptop` driver, but it binds only on the models in its DMI table (not this
  one), so the WMI event is unclaimed. Fedora's kernel has no `ec_sys`.

Still unverified: the codes `0x47C` holds with the bit set (expected 0xB0/0xB1/0xB2/0xB9, from the same `_Q53`). The
first hardware test of the build checks it.

## Approaches considered

1. **The helper listens to the kernel's ACPI netlink events and reads `0x47C` on each WMI `0xd2` event (chosen).**
   No kernel module, no polling; uses only what the helper already has (the EC window) plus a netlink socket.
2. Poll `0x47C` often enough to catch a code (it lasts about 70 ms): 30+ reads a second for as long as the app runs,
   and still racy. Rejected for battery and reliability.
3. A kernel module (our own, or loading upstream `uniwill-laptop` with `force=1`): upstream takes over the EC, which
   conflicts with the helper; our own module means DKMS and module signing for Secure Boot. Kept for later, with the
   Secure Boot work the README already mentions.

## Design

### Helper (`uucc-helper`)

- **Event listener:** at start, if the EC is usable, a thread opens a netlink socket (`AF_NETLINK`, generic netlink),
  resolves the `acpi_event` family, joins its multicast group and reads events. For an event with class `wmi` and
  type `0xd2`, it reads EC `0x47C` under the EC lock straight away; a non-zero value is a hotkey code, sent to every
  subscribed client. Zero (the code already gone, or another firmware's event) is dropped. The netlink message parsing
  is a pure function, unit-tested with captured bytes.
- **Subscription:** a new request, `Hotkeys`. An authorized client sends it as the first request on a connection of its
  own; the helper answers `Done`, and from then on writes `Hotkey { code }` lines on that connection, and nothing else.
  The connection counts as a client (no idle exit while the app runs). A client that can't take a line (gone, or its
  buffer full) is dropped from the subscribers, never blocking the listener.
- **No listener, no subscription:** if netlink can't be opened (an older kernel without the ACPI netlink family, or the
  EC is unavailable), `Hotkeys` is answered with `Error` (or `Unavailable`), and the app keeps today's behavior.
- **Protocol version 2** (the new request and reply); a different version gets `WrongVersion` as today.
- **systemd unit:** `PrivateNetwork=yes` must go: the kernel sends ACPI netlink events only to the initial network
  namespace, so the sandboxed helper would hear nothing. In its place: `RestrictAddressFamilies=AF_UNIX AF_NETLINK`
  and `IPAddressDeny=any`, so the helper still can't use the network.

### App

- **Hotkeys on Linux:** `hotkeys::event_thread` (today a stub that fails) opens the helper connection, sends `Hotkeys`,
  and passes each code to the same `pressed` callback Windows uses. Everything after that (keyboard light steps,
  performance key, Fan Boost, fan safety) is the existing Windows handling in `on_hotkey` / `on_power_hotkey`.
- **The app-present bit:** as on Windows, `Hotkeys::start` sets `0x741` bit 0 while the app runs; on Linux it also
  registers the helper release (`OnDisconnect 0x741 bit 0 → 0`), so a crash gives the keys back to the firmware.
  `fans::set_custom` stops setting and clearing the bit on Linux while the hotkeys are active (they own it); when the
  hotkeys aren't available it keeps doing so, as now.
- **The per-second checks** (`check_ec_mode`, and on Linux `check_fan_safety` at 1 s) run only while the hotkeys aren't
  active. With them active, mode changes come as 176 (the app switches, as on Windows) and safety as 187.
- **Reconnecting:** if the hotkey connection breaks (helper restarted), the app reconnects on the existing 5-second
  `check_hotkeys` timer, as `start_hotkeys` does when Control Center stops on Windows; the helper-restart handling
  already sets the app's settings again.
- **Settings page:** the "Fn keys" line says "handled instantly by this app" on Linux too when it works, and why not
  when it doesn't (from the helper's answer).
- **Not in this work:** pop-ups (Linux `osd` stays silent; a KDE/GNOME OSD is its own task), the Windows key lock,
  touchpad toggle and Caps/Num Lock lights.

### Error handling and safety

- A missed code (cleared before the read) loses that one key press; nothing else. The 24 ms margin measured is much
  longer than a read under the EC lock, but a helper busy with a slow BIOS variable write could miss one.
- Only authorized clients (polkit, as today) can subscribe; the helper sends only codes, never takes input on that
  connection.
- If the app quits or crashes, the helper clears the app-present bit, and the firmware has the keys again.

## Testing

- **Helper unit tests:** netlink parsing (an ACPI event message, an unrelated one, a truncated one); an `0xd2` event with
  `0x47C = 0xB2` sends `Hotkey { code: 178 }` to subscribers only; code 0 and other event types send nothing; a dead
  subscriber is dropped; `Hotkeys` without a listener is refused.
- **App tests (fake helper):** the Linux hotkey thread passes pushed codes to `pressed`; `Hotkeys::start` sets
  `0x741` bit 0 and registers its release; `set_custom` leaves the bit alone while the hotkeys are active; the
  per-second checks are skipped while they are.
- **Hardware (Fedora live USB):** each key a few times with the curve off and on: the keyboard brightness steps, the
  performance key switches modes (the app's display and the key's light agree), Fan Boost's state follows the EC.
  Screen brightness, volume, touchpad, airplane mode and webcam keys still work with the bit set. Stopping the helper
  gives the keys back to the firmware. `journalctl` shows no netlink errors.

## Docs

README and FEATURES ("On Linux"): Fn keys and the performance key work; what still doesn't (pop-ups, Windows key lock,
touchpad toggle, Caps/Num Lock lights). The Linux hardware test list gets the rows above. The Linux port spec's line
that Linux never sets the app-present bit is updated.
