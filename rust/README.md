# Keyboard RGB 2.0 (Rust)

The Rust version of the app in the folder above, with the same features and a new interface:

- **Windows 11 look:** a navigation pane (Keyboard, Light bar, Timer & alerts, Settings), Settings-style cards and rows,
  toggle switches, sliders and a color picker, in your dark/light theme and accent color. The title bar blends
  in with the window.
- **Live previews:** a drawing of the Tulpar's keyboard whose keys glow in the zone colors, and the laptop's front edge
  with the light bar's 23 LEDs. They show what the hardware shows, including fades, the idle timer and effects drawn
  by the app.
- **Effect tiles:** each effect is a tile with a small animation of what it does, grouped into Lighting,
  Animated by this app, Sound and System status.
- **Windows 11 pop-up:** the Fn-key and toggle pop-ups are drawn like Windows 11's volume and brightness flyout: a
  rounded card above the taskbar with an icon, a bar in the accent color and the value. It slides up and fades in,
  its bar follows changes smoothly, then it fades out.
- **Settings save** every few seconds after a change, not only on exit.

It uses the same settings file (`%APPDATA%\MonsterKeyboardRGB\settings.json`), the same startup task and the same
single-instance lock as the C# version, so they can replace each other but never run at the same time.

## Building

Needs Rust (MSVC toolchain) and the Windows SDK (for the icon and manifest).

```
cargo build --release
```

The app is `target\release\KeyboardRGB.exe`, a single file with nothing else to install. It asks for administrator
rights, like the C# version (the Fn keys go through the BIOS WMI interface).

To look at the window without touching the hardware:

```
set KEYBOARD_RGB_NO_ADMIN=1
set CARGO_TARGET_DIR=target-demo
cargo build --release
target-demo\release\KeyboardRGB.exe --demo
```

`--demo` opens no devices and saves nothing. `--page N` opens on a page (0 Keyboard, 1 Light bar, 2 Timer & alerts,
3 Settings), `--tray` starts in the tray, `--osd-test` shows the pop-ups once.

## Code

| File | What it does |
| --- | --- |
| `src/device.rs` | The ITE lighting controllers over HID (commands, color correction, brightness tracking). |
| `src/effects.rs` | The effect list of each device. |
| `src/engine.rs` | The thread that runs effects drawn by the app, with sound, spectrum, key and status inputs. |
| `src/scenes.rs` | Those effects: light bar scenes, indicator flashes, keyboard Music/Spectrum/temperature. |
| `src/audio.rs`, `src/status.rs` | Core Audio (peak meter, loopback capture) and battery/CPU/temperature readings. |
| `src/hotkeys.rs` | Taking the Fn keys over from the EC through WMI. |
| `src/app.rs` | Everything the window, the tray and the keys do. |
| `src/preview.rs` | The keyboard drawing and the effect simulations for the previews. |
| `src/osd.rs`, `src/tray.rs` | The pop-up (Direct2D on a layered window) and the tray icon and menu. |
| `ui/*.slint` | The window, in Slint. |
