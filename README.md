# Tulpar Control 3.0

A replacement for Control Center on the Monster Tulpar T5 V21.4. It does the keyboard and light bar lighting,
performance modes, Fan Boost, custom fan curves and battery care, and has a hardware monitor. The full feature list is
in `FEATURES.md`.

- **Windows 11 look:** a navigation pane (Keyboard, Light bar, Performance, Battery, Monitor, Timer & alerts,
  Settings), Settings-style cards and rows, toggle switches, sliders and a color picker, in your dark/light theme and
  accent color.
- **Live previews:** a drawing of the Tulpar's keyboard whose keys glow in the zone colors, and the light bar's 23
  LEDs, showing what the hardware shows.
- **Performance:** Office, Gaming and (where the laptop has it) Turbo mode, Fan Boost, and a fan curve editor per mode.
- **Battery:** High capacity, Balanced and Stationary charging, with the battery's health and charge rate.
- **Monitor:** CPU and GPU temperature, CPU load, fan speeds, memory and battery, with the last minute of each.
- **Windows 11 pop-ups** for the Fn keys, the performance key and Fan Boost.

Settings are kept in `%APPDATA%\TulparControl\settings.json`; the first run reads the old
`%APPDATA%\MonsterKeyboardRGB\settings.json` if there is one, and replaces Keyboard RGB's startup task.

## Building

Needs Rust (MSVC toolchain) and the Windows SDK (for the icon and manifest).

```
cargo build --release
```

The app is `target\release\TulparControl.exe`, a single file with nothing else to install. It asks for administrator
rights, because the Fn keys, the performance modes, the fans and battery care go through the BIOS WMI interface.

To look at the window without touching the hardware:

```
set TULPAR_NO_ADMIN=1
set CARGO_TARGET_DIR=target-demo
cargo build --release
target-demo\release\TulparControl.exe --demo
```

`--demo` opens no devices, sends nothing to the embedded controller, shows made-up readings and saves nothing.
`--page N` opens on a page (0 Keyboard, 1 Light bar, 2 Timer & alerts, 3 Settings, 4 Performance, 5 Battery,
6 Monitor), `--tray` starts in the tray, `--osd-test` shows the pop-ups once. `cargo test` checks the fan curve rules.

## Code

| Folder / file | What it does |
| --- | --- |
| `src/main.rs` | Start-up, the single-instance lock, the renderer and the crash handler. |
| `src/app/mod.rs` | The window, the tray, the lighting and the Fn keys: everything the UI thread does. |
| `src/app/power.rs` | Performance modes, Fan Boost, fan curves and battery care, and the performance keys. |
| `src/app/monitor.rs` | The readings thread and the Monitor page. |
| `src/app/bios.rs` | Startup lighting, the CPU voltage offset and recovery after a failed start, kept in the BIOS. |
| `src/lighting/` | The ITE controllers over HID (`device.rs`), the effect lists, the effects engine and scenes, audio, the previews and the keyboard hook. |
| `src/hardware/ec.rs` | The embedded controller's RAM through WMI, on one thread. |
| `src/hardware/hotkeys.rs` | Taking the Fn keys over from the EC and listening for hotkey events. |
| `src/hardware/performance.rs`, `fans.rs`, `battery.rs` | The EC registers for modes, power limits and TCC offset, fan tables and speeds, and charging. |
| `src/hardware/uefi.rs` | Control Center's UEFI variable, read and changed field by field. |
| `src/hardware/status.rs`, `system.rs` | Temperatures, loads, memory and battery readings; touchpad, startup task and registry. |
| `src/shell/` | The pop-up (Direct2D on a layered window) and the tray icon and menu. |
| `ui/*.slint` | The window, in Slint; `power.slint` has the Performance, Battery and Monitor pages. |
| `tools/` | PowerShell scripts used to test the embedded controller's behavior. |
