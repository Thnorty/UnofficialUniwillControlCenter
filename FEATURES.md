# Unofficial Uniwill Control Center — feature overview

An unofficial replacement for Uniwill's Control Center, made and tested on the Monster Tulpar T5 V21.4 against Control
Center v4.8.47.13 (installed as Control Center 3 4.9.47.11). It was called Tulpar Control before 3.1. It covers
Control Center's lighting, performance modes, fans and battery care, and a hardware monitor. This is a Uniwill/Tongfang platform with two ITE lighting controllers:
- the four-zone keyboard (ITE 8291, USB 048D:CE00, firmware 11.04);
- the light bar on the front edge (USB 048D:6005, firmware 15.04, Control Center's `MEZone_Lighbar` type);
- the embedded controller (EC), for the Fn keys, performance modes, fans and charging; this model uses Control
  Center's "RamFan 1.5" fan control (`MyFanManager_RamFan1p5`, `FanTable_Manager1p5`).

It talks to the lighting controllers directly over HID, and to the embedded controller through the BIOS WMI interface.
It does not need Control Center, its GCUBridge/GCUService service or its MQTT broker. It needs administrator rights;
the "Start with Windows" option starts it elevated in the tray through Task Scheduler. The exe doesn't ask for them
itself: a launch without them shows the running app's window (its tray window lets that one message through), or
starts the app through the startup task when that task starts the same exe, or else asks with a UAC prompt. It also
runs on Linux, with the differences listed in [On Linux](#on-linux).

## From Control Center

These work the way Control Center does them. The commands come from Control Center's decompiled service.

### Keyboard lighting (four-zone)
- **Effects:** Static (four zone colors), Breathing, Wave, Rainbow, Flash, Mix and Music: the full list Control Center
  offers for this keyboard. The keyboard's Rainbow is four fixed colors (red, green, blue, magenta) that don't move,
  and its Mix shows one color on the whole keyboard, going through the colors in turn.
- **Speed:** five steps, for every animated effect.
- **Wave direction:** left → right or right → left.
- **Effect colors:** Control Center's default seven-color palette for Breathing, Wave, Flash and Mix.
- **Music:** the four zone colors stay, and the brightness follows the peak level of the default audio output,
  with automatic gain.
- **White balance:** the same green and blue scaling Control Center applies, so colors look the same.
- **Saved in the controller:** the final lighting is saved to the controller when the app exits, so it stays without
  the app running.
- **Startup lighting (Save to BIOS):** Control Center's "Startup Mode". Saves the current effect, colors, brightness
  and speed as what the keyboard shows before Windows starts: the colors go to the controller's startup slots (9–15,
  command 14h) and the BIOS gets the 08h (control 3) and 1Ah commands it sends at boot, in the `RGBKeyboard08` and
  `RGBKeyboard1A` fields of Control Center's UEFI variable (`UniWillVariable`
  {9f33f85c-13ca-4fd1-9c4a-96217722c593}). Effects drawn by this app leave the zone colors. "Restore the laptop's
  own" puts back what the BIOS had before (Mix on this laptop).

### Light bar
- **On/off switch**, and the light bar's Fn toggle key (hotkey code 175), with a pop-up.
- **Effects:** the firmware effects Control Center's light bar driver handles: Static color, Breathing, Wave
  (Control Center's "Colorwave", which has no direction), Rainbow, Mix and Thinking. Control Center's Flash is left
  out. On the bar:
  - Wave lights the colors up from the middle out to both ends, in bands with the first color in the middle, then
    starts over.
  - Rainbow is seven still sections, and Mix shows one color on the whole bar, going through the colors in turn,
    like the keyboard's.
  - Thinking is a star with a trail about five LEDs long. It runs to the right end and back, then takes the next
    color.
- **Music:** a level meter that grows from the middle of the bar outwards with the sound, set LED by LED (user
  mode, command 12h and output reports). Up to seven colors split each half into equal bands, the first in the
  middle. Control Center gives one color per LED instead, so its last color fills the rest.
- **Star Spark and Star Hitting:** Control Center draws these in software for its per-LED devices; here they are
  drawn LED by LED the same way. Star Spark twinkles every LED; Star Hitting bounces a ball of light per color.
- **Speed and brightness**, as on the keyboard.
- **Color correction:** the light bar's own correction (`WKDColor.cheatRGB_HIDLightbar`).
- **First run:** starts from what the light bar was showing (Control Center left it off, on Thinking).
- **Startup lighting (Save to BIOS):** as on the keyboard, in the `SmartLightbar08` and `SmartLightbar1A` fields.
  Effects drawn by this app leave their first color. The light bar's own is Thinking.

### Shared
- **Idle timer:** switches the backlight and the light bar off after a chosen time without keyboard or mouse input (Never,
  15 s … 1 h), with the same fade-out, and back on with the next input.

### Fn keys and pop-ups
- **Fn keyboard-light keys** (down, up, cycle): handled instantly. The app takes them over from the firmware the way
  Control Center's service does, by setting the "app present" bit in the embedded controller.
- **Windows key lock:** shows the current state and has a button to toggle it.
- **Touchpad:** toggles on and off, and the button follows the touchpad's Fn shortcut.
- **Pop-ups (OSD):** for keyboard brightness, touchpad on/off, Windows key lock/unlock, and Caps Lock and Num Lock
  (each can be turned off on the Timer & alerts page).

### Performance modes
- **Office, Gaming and Turbo:** the EC's fan mode byte (0x751) and the CPU power limits (PL1, PL2, PL4) the EC keeps
  as each mode's defaults, sent on the charger; on battery the EC picks its own limits, as Control Center leaves it.
  Turbo only shows where the EC reports it (0x49F bit 1).
- **Performance key** (hotkey code 176): switches modes the way Control Center does (Office → Gaming → Turbo → Office,
  or Office ↔ Gaming without Turbo), with a pop-up.
- **Fan Boost:** a switch, the tray menu and the Fan Boost key (codes 167 and 172), with a pop-up.
- **Set again** after sleep and when the charger is plugged in or out.
- Left at the firmware's: the fan switching speed. GPU overclocking and the `PowerMode` UEFI field Control Center
  also writes are left out.

### CPU and GPU tuning
Control Center's overclocking settings for Intel laptops:
- **Custom power limits per mode:** PL1 (sustained), PL2 (boost) and PL4 (peak) on the charger, from 10 W up to what
  the BIOS's Smart APC table allows (here 120, 120 and 165 W), read as Control Center does through its ACPI driver
  (`\\.\ACPIDriver`, IOCTL 0x9C40A500). Each limit is kept at least the one before it. Without the driver the
  limits go up to the highest mode's own.
- **TCC offset per mode:** makes the CPU start throttling that many °C below its maximum temperature (EC 0x786,
  0x80 | °C), up to the table's maximum (here 5 °C), starting from the mode's default (EC 0x7D8–0x7DA).
- **GPU power per mode** (NVIDIA): the configurable TGP raises the GPU's power limit from its default up to what the
  Smart APC table allows (here 80 W, up to 115 W), and Dynamic Boost gives it up to the table's extra (here 15 W)
  when the CPU needs less, so it can draw up to 130 W. As Control Center's SetUserProfile sends them on each mode
  change: EC 0x744 the watts above the default, 0x745/0x746 Dynamic Boost's target (255) and watts, and 0x743 bit 2
  TGP is the app's, bit 1 Dynamic Boost on, bit 0 set by the app. Without bit 2 the EC picks the TGP for the mode
  (+10 W in Office and Gaming and +35 W in Turbo here), which the page shows. The EC passes the values to the BIOS's own fields (0x788 CTWA,
  0x7C4 bit 3 DBEN, 0x7D4 CPUA, 0x7D5 DBAP) and raises _Q83/_Q84, which hand them to NVIDIA's driver within about
  2 s. Dynamic Boost is on and the TGP the EC's unless changed, as in Control Center's profiles; on battery both are
  left to the EC.
- **Profiles per mode:** as Control Center's five profiles per mode, each with its own power limits, TCC offset,
  GPU power and fan curve. Every mode starts with "Standard"; a new one starts as a copy of the active one, and changes on the
  page go into the active one. Switching sends it at once, with a pop-up.
- **GPU mode (MUX switch):** Hybrid (MSHybrid), integrated graphics only (Intel or AMD) or NVIDIA graphics only,
  the BIOS setup's own option, from the next start. Saved in the UEFI variable's `OemDisplayMode` (4, 1 and 2;
  Control Center switches between 4 and 2 only), and only over a value the app knows; 255 means no MUX and hides the
  setting.
- **Core voltage offset:** written to the UEFI variable (`ICpuCoreVoltageOffsetValue` and
  `ICpuCoreVoltageOffsetNegativeValue`), so it takes effect after a restart. This BIOS allows 0 to +100 mV (range
  type 0); there's no undervolting.
- **Recovery:** if the laptop fails to start with the tuning, the BIOS starts without it and sets
  `OverClockRecoveryFlag`; the app then sets every mode back to its defaults and the voltage offset to 0, clears the
  flag and says so, as Control Center's `DetectRecoveryFlagFromBiosVariable` does.
- **Safe writes to the UEFI variable:** it's read first, only the fields being changed are written, with the same
  size (180 bytes) and attributes, and it's read back to check. Nothing is written while Control Center's service
  runs, since it rewrites the whole variable from its own copy. Memory overclocking isn't offered: this BIOS reports
  no support for it.

### Fans
- **Custom fan curve per mode:** Control Center's fan table editor. The EC's own table for each mode is read once
  (the same request Control Center makes: 0xF5F/0xF5D/0xF5E) and is the starting point; the curve is written to the
  EC's RAM tables (0xF00–0xF5F) and switched on with 0x7C6 bit 2. CPU and GPU fans each follow their own curve:
  the app also sets 0x7C5 bit 7 (Control Center's "Fan Control Respective", off by default there), without which the
  EC doesn't keep the two curves apart. With the graphics card off, the GPU fan stays at the left end of its curve,
  and the page says so. On Windows, where each EC call takes over 100 ms, only the registers that changed are
  written.
- **Fan safety protection:** while the EC's protection is on (0x7A5 bit 3, event 187) the fans go back to the EC's
  curve, as in Control Center.
- **Fan speeds** (RPM and duty), as Control Center's `FanInfo` reads them.

### Battery care
- **High capacity, Balanced and Stationary:** Control Center's three charging modes (`BatteryProtection2`, 0x7A6 bits
  4 and 5). The first run picks up the mode Control Center last set; it's set again after sleep.
- **USB charging while off:** the ports keep powering devices while the laptop is off or asleep (EC 0x767 bit 4, as
  Control Center's USB charger switch). Shown where the EC or Control Center's settings say the laptop has it, and
  from then on, so it stays after Control Center is uninstalled; the first run keeps Control Center's choice.

### App behavior
- **Installer:** installs to Program Files with a Start menu entry, and by default starts the app with Windows. An
  upgrade closes the running app first (`--exit`, which hands the Fn keys and fans back). It tells an upgrade from an
  install by the version in its uninstall key: an upgrade goes straight to an "Update from X to Y" page (or Reinstall,
  or an older version), keeps the earlier choices and leaves the startup task as the app's switch has it. If the app
  crashes while exiting (seen rarely, in a system DLL), it ends at once and leaves `exit-crash.dmp` in its folder,
  instead of Windows Error Reporting holding it, and the installer, for about half a minute. Uninstalling removes the
  startup task and asks whether to remove the settings and plugins too (ticked to begin with), which also removes the
  hooks plugins added to Claude Code and the copy of its settings kept before them.
- **Terminal (`uucc`):** `uucc.exe` sets the mode, Fan Boost, profile, charge mode, setups, lighting, display
  colors and GPU mode, and prints the status with fresh readings, from any terminal without administrator rights.
  It asks the running app over a named pipe that only the signed-in user can open, so the window and tray follow;
  `--json` is for scripts and `--start` starts the app first. The installer adds it to `PATH` (an option, ticked). An
  update shows that option on its Ready page, as chosen before (ticked if it never was), or says it's in `PATH` already.
- **Tray:** runs in the tray and can start with Windows. The tray menu has the performance mode and Fan Boost.
  **Show in the tray** (Settings) turns the icon off: the app keeps running, notifications still show, starting it
  again opens the window. **Exit** at the bottom of Settings closes the app, with or without the icon.
- **First start:** before it sends anything to the laptop, a one-time question says the app is unofficial and what it
  writes to, and whether this laptop is the tested Monster Tulpar T5 V21.4 (from the firmware's maker and model).
  Cancel quits.
- **Earlier names:** the first start moves `%APPDATA%\TulparControl` (Tulpar Control) over, and replaces the startup
  task of Tulpar Control and of Keyboard RGB; settings from Keyboard RGB are read too. The single-instance lock and
  the window messages keep Tulpar Control's names, so an installer can close a running Tulpar Control.
- **Coexists with Control Center:** steps back and leaves the Fn keys, the modes, the fans and battery care alone
  while Control Center's service is running; the Performance and Battery pages then only show what it set.
- **Display color modes:** Control Center's Display page (Display page): Vibrant (1.5× saturation, as Control
  Center), Cinema, Low blue light and Custom, each with its own saturation, contrast, color temperature and red,
  green and blue levels. Like Control Center, a color matrix over the whole screen through the Magnification API
  (`MagSetFullscreenColorEffect`). Windows removes it when the app exits, even after a crash.
- **Back up and restore:** all settings in one JSON file (Settings page). Restoring checks the file, asks first, and
  restarts the app with the restored settings.
- **App profiles:** Control Center's per-game profiles, by setup: a program (picked as its `.exe`) switches to a saved
  setup while it runs, checked every two seconds. When it closes, what was on before it started comes back; if
  several are running, the one higher in the list wins. They can all be turned off with one switch (Setups page).

## Added beyond Control Center

- **No Control Center needed:** no service, UWP app or broker. It is a single small exe.
- **Windows 11 interface:** a Settings-style window in the system's dark/light theme and accent color, with live
  previews of the keyboard and the light bar, animated effect tiles and a color picker. The navigation pane collapses
  to its icons (with tooltips) with the ☰ button, and stays that way.
- **Smooth Wave:** the keyboard's firmware moves Wave's colors a whole zone at a time; while the app runs it draws
  Wave itself, blending the colors across the zones. After exit the firmware's Wave takes over again.
- **Fine brightness:** a 0–100% slider in 1% steps for the keyboard and the light bar, sent as the nearest of the
  controllers' 51 levels (0–50). Control Center only uses five levels.
- **Smooth brightness:** every brightness change fades (about 0.18 s) instead of jumping: the slider, the Fn keys,
  and coming back from the idle timer.
- **Fn key steps:** Off, 5%, 20%, 40%, 60%, 80% and 100%; from a value in between they go to the next step. The cycle
  key wraps to off after full brightness.
- **Editable effect colors:**
  - each palette effect can have 1–7 colors (Mix 2–7);
  - click a color to change it, right-click or **×** to remove it, **+** to add one, **Reset to default colors**;
  - each effect remembers its own colors.
- **Zone shortcuts:** "Set all" and ten quick color presets for the four zones.
- **Music controls:** the brightness slider sets the peak brightness, and the speed slider sets how fast the light
  falls back.
- **Pop-up style:** drawn like the Windows 11 volume and brightness flyout: a rounded card above the taskbar with an
  icon, a bar in the accent color and the value, sliding up and fading in. It follows the light/dark theme, doesn't
  take focus or block clicks, and appears on the screen with the mouse pointer.
- **Tray icon colors:** the tray and window icon show the current effect's colors.
- **Tray quick menu:** keyboard effect and brightness, light bar on/off, "Same as keyboard" and effect, and the timer,
  without opening the window.
- **Same as keyboard:** the light bar can follow the keyboard's effect, colors and speed, keeping its own brightness.
  The keyboard's four zones become four sections of the bar.
- **Spectrum:** a spectrum analyzer of the sound the PC plays (captured from the default output, analyzed with an
  FFT, with automatic gain):
  - keyboard: each zone follows a range of pitches, bass on the left;
  - light bar: mirrored, bass in the middle and treble at the ends, colored from the first color to the last.
- **Typing:** a keyboard hook reads only key positions, and it is only installed while a Typing effect runs. It works
  while this app's window has focus too.
  - light bar: each key press sends a ripple out from the matching spot on the bar;
  - keyboard: each key press lights its zone in the next color, and the light spreads to the zones beside it as it
    fades. Dark while you're not typing.
- **Status on the lights:**
  - light bar: Battery (level bar, a pulse while charging), CPU load, CPU temperature and GPU temperature gauges;
  - keyboard: CPU temperature and GPU temperature colors, blue when cool to red when hot.
  - The CPU temperature is the hottest ACPI thermal zone, the same reading Control Center uses. The GPU temperature
    comes from the NVIDIA driver (NVML), only while the GPU is on: with Optimus it switches off while nothing uses
    it, and asking the driver anything would switch it back on (about 25 W on the Tulpar), so the app checks its
    power state with Windows first and shows "no reading" (Off on the Monitor page) meanwhile.
- **Light bar indicators:** the bar flashes when Caps Lock (white) or Num Lock (blue) changes: opening from the middle for on, closing to the middle for off. Each can be
  turned off. Over effects this app draws the flash is drawn on top; over firmware effects the effect comes back
  after the flash.
- **Timer:** 5 to 60 minutes, from the window or the tray. When it ends the light bar blinks orange, with a pop-up
  and a notification.
- **Restore on start:**
  - restores all settings when it starts;
  - keeps a brightness changed with the Fn keys while the app was closed.
- **Brightness kept on exit:** about a second after the Fn keys are handed back, the embedded controller sets its
  own brightness. Control Center leaves the keyboard that way; this app puts your brightness back.
- **Fn keys handed back safely:** they go back to the firmware even if the app crashes, and are taken over again
  after sleep or resume.
- **Fan curve safety rules:** a custom curve always rises with the temperature, never goes more than 30% below the
  laptop's own curve, and never below it from 75 °C. When the app exits or crashes the fans go back to the EC's
  curve, since nothing watches the safety protection then.
- **Drag-to-edit fan curves** on a chart, with the laptop's curve drawn under yours.
- **Hardware monitor:** CPU and GPU temperature, CPU load, both fans, memory, battery and the power mode, each with a
  line of its last minute. It only reads while the page is open.
- **Battery details:** health (full charge against design capacity), capacities, charge or discharge rate, time
  left, voltage and charge cycles.
- **Saved setups:** the keyboard's and the light bar's lighting with the performance mode and Fan Boost, saved under
  a name and applied in one go from the Setups page or the tray's Setups menu, with a pop-up. Update saves what's on
  now into a setup.
- **Plugins:** small scripts that show things on the light bar, over the current effect (`PLUGINS.md`):
  - **Claude Code:** a sweep while Claude works, its task list filling the bar, a pulse when it needs you and a flash
    when it's done, through Claude Code hooks (added and removed with one button, with a backup) that run in the
    background and stay quiet while the app is closed.
  - **Downloads:** download progress from a Chrome/Edge extension, or a sweep while an unfinished download is in the
    Downloads folder.
  - **Example:** off at first; it fills the bar and uses every kind of setting, as a template for new plugins.
  - **Focus timer:** Pomodoro rounds: the bar fills while you focus and empties during the break, with a long break
    every few rounds, and Start, Pause, Skip and Stop buttons.
  - **Microphone** (off at first): the bar glows red while Windows' microphone is muted (always, or only while an
    app tries to listen), and if wanted while an app listens, with a Mute / unmute button. "In use" is Windows' own
    privacy record (the capability access manager's consent store).
  - **Media** (off at first): song progress from Windows' media controls (any player, browsers included), and the
    volume for two seconds when it changes.
  - **Notifications** (off at first): a flash for new notifications from chosen apps, each with its own color if
    wanted. It sees only which app sent one.
  - **Temperature alert** (off at first): the bar pulses red while the CPU or the GPU is over its limit.
  - Import more from a `.zip`. Each plugin's settings (colors, sliders, switches, lists, text) are on the Plugins
    page.
  - Plugins are sandboxed. They can't read files, use the network or start programs, beyond the permissions shown on
    their card. Events reach them through a local address (`127.0.0.1:47823`) that refuses web pages.

## Still missing

### Keyboard lighting
- **Separate brightness on charger and battery:** Control Center's service keeps one brightness for each and
  switches when the charger is plugged in or out. This app has one brightness.
- **Aurora:** not the light bar. Control Center's Aurora switch hands all lighting over to a separate third-party
  RGB program called Aurora, which isn't installed here. Not implemented.
- **Effects drawn by this app stop with it:** Music, Spectrum, Star Spark, Star Hitting, Typing and the status
  effects need the app running. When it exits, the keyboard keeps their static zone colors and the light bar their
  first color (or cyan). Wave goes back to the firmware's zone-by-zone Wave.
- **Night Light:** removed. It had no visible effect on this laptop, and Control Center doesn't offer it for this
  model.
- **Fn keys while the app is closed:** they still need several presses. The embedded controller counts from a hidden
  internal brightness level that hasn't been found.
- **Effects for other keyboard types:** Control Center's service also has effects for per-key and other keyboard
  types (Reactive, Ripple, Raindrop, Marquee, Spark, Neon, Gaming, per-key custom, …). Control Center doesn't offer
  them for four-zone keyboards, so they probably don't apply here.

### Rest of Control Center

Not implemented:
- **GPU tuning:** GPU core and memory overclocking, configurable TGP (here up to 35 W over the 80 W base), Dynamic
  Boost (15 W) and NVIDIA Whisper Mode. They go through the NVIDIA driver, and couldn't be built and tried without
  the GPU.
- **GPU modes:** dGPU mode (Hybrid, iGPU, Auto) and disabling NVIDIA Optimus.
- **Battery:** battery calibration. USB-C power delivery priority and starting up when the charger is connected
  aren't supported on this laptop: Control Center's own flags (`TypeCSupport`, `AcRecoverySwitchSupport`) say so and
  it hides them.
- **Display:** refresh rate, color calibration, and Control Center's Video, Internet and Photo modes (color-space
  conversions for a calibrated panel). Gamma isn't adjustable: Control Center sets it with gamma ramps, which stay
  after the app exits and clash with Night light.
- **Monitoring:** In-Game OSD and Quick Information Box.
- **Keys:**
  - Fn Lock;
  - a Scroll Lock pop-up;
  - key remapping, Game Mode and application profiles.
- **Device switches:** Wi-Fi, Bluetooth and webcam, and touchpad active-area settings.
- **Light bars and LED strips:** only on models that have them.
- **Firmware updates** for the keyboard controller.

## On Linux

Since 3.5.0 the app also runs on Linux (see the README's Linux section for installing it and for Secure Boot). It runs
as the signed-in user; a small root helper, `uucc-helper`, reaches the embedded controller by mapping the firmware's
EC window (the one its `ECRR`/`ECRW` methods use) and writes the BIOS settings variable through efivarfs. The
lighting controllers are used directly over `hidraw`. Where Linux differs:

- **Fn keys:** the keyboard light keys, the light bar key, the performance key and the EC's Fan Boost and fan safety
  reports work as on Windows. The firmware's `_Q53` hands each hotkey to its WMI device as event 0xd2; with no WMI
  driver bound, the kernel still sends that as an ACPI netlink event. The helper listens for it and reads the code
  from the EC (ACPI byte 0x7C, window 0x47C, there for about 20 ms after the event), and passes it to the app over a
  connection of its own. As on Windows, the app holds the EC's app-present bit (0x741 bit 0) while it runs; the
  helper clears it if the app goes away, and the firmware has the keys again. The Windows key lock works too: the
  switch asks the EC to flip it (trigger byte 0x767 bit 0), and the EC reports the result as code 64 or 65, as on
  Windows. Still missing: the touchpad toggle and the Caps Lock and Num Lock pop-ups and lights on the light bar.
- **Touchpad key:** it sends Ctrl+Super+F24, Windows 11's touchpad toggle shortcut. KDE takes that as the touchpad
  toggle by itself; on GNOME the app gives GNOME's own "Toggle touchpad" shortcut (`org.gnome.settings-daemon.plugins.
  media-keys touchpad-toggle`) that combination while it's empty, so GNOME toggles the touchpad and shows its pop-up.
  The usual keyboard layouts give F24 no name GNOME matches, so the shortcut names the key by its code:
  `<Ctrl><Super>0xca`.
- **Pop-ups:** KDE Plasma's own, through its pop-up service (`org.kde.osdService` on the session bus): its keyboard
  brightness pop-up for the keyboard light keys, and its text pop-up with a Breeze icon for the rest (`lock` and
  `object-unlocked` for the Windows key, `battery-profile-powersave`, `-balanced` and `-performance` for Office, Gaming
  and Turbo, `chronometer` for the timer, `brightness-high` for the light bar, `temperature-cold` for Fan Boost and
  `view-list-icons` for setups). They're sent from a thread of their own, in order.
- **Pop-ups on GNOME:** GNOME Shell keeps its pop-up call (`ShowOSD`) to its own components, so the packages install a
  small GNOME Shell extension (`uucc-osd@thnorty.github.io`, GNOME 45 to 50) that shows them with GNOME's own pop-up:
  `Show(icon, label, level)` on `io.github.thnorty.uucc.Osd`. The keyboard light keys get a bar with
  `keyboard-brightness-symbolic`; the rest Adwaita's symbolic icons (`changes-prevent` and `changes-allow`, the
  `power-profile-*` icons, `alarm`, `display-brightness`, `weather-windy` and `view-list`). The app adds the extension
  to GNOME's enabled list the first time (not if the user turned it off), and GNOME loads it at the next sign-in.
  Other desktops have no pop-ups.
- **Without the hotkeys** (a kernel without the ACPI netlink family): the app reads the mode byte (0x751) every
  second to follow the performance key, checks the EC's safety flag every second while a custom curve is on, and sets
  the app-present bit only while a custom curve is on (the EC keeps to the RAM fan tables only with it); the
  performance key then does nothing while a curve is on, and the fan curve page says so. The helper turns the custom
  curve off (and the bits that go with it) if the app ends without doing it.
- **The helper stopping or restarting** while the app runs: it undoes the app's settings as the connection breaks.
  The app checks every few seconds; stopped, the Performance and Battery pages say so until it's back, and either way
  the app then sets everything again.
- **Power limits:** Windows reads their range from the BIOS's power limit table through Control Center's driver; the
  Linux build can't read that table yet, so PL1, PL2 and PL4 go up to the highest mode's own defaults, and the TCC
  offset and GPU power (configurable TGP, Dynamic Boost) aren't offered.
- **Display colors:** never on Linux. Wayland has no whole-screen color matrix an app can set (the Magnification API's
  equivalent), and the compositor's own night light covers the rest.
- **Effects:** Typing (no global key hook), Music and Spectrum (no sound capture yet) are left out; the rest, Smooth
  Wave and the status effects included, work as on Windows.
- **Also left out for now:** turning the lights off when idle, plugins and app profiles.
- **Readings:** CPU load and memory from `/proc`, the CPU temperature from the hottest ACPI thermal zone (or
  `coretemp`), the battery and charger from the kernel's `power_supply` class, the NVIDIA GPU's power state from sysfs
  (so the GPU isn't woken to ask) and its temperature from NVML when the NVIDIA driver is installed.
- **Desktop:** the tray icon is a StatusNotifierItem (KDE Plasma; GNOME with the AppIndicator extension), dialogs use
  kdialog or zenity and the desktop's file picker, the theme and accent color follow the desktop's settings portal,
  and start at login is an XDG autostart entry, turned on at the first start as the Windows installer does by
  default (turning it off in Settings stays off).

## Building

The app is written in Rust. Build it with `cargo build --release`; the output is `target\release\UnofficialUniwillControlCenter.exe`, a
single exe with nothing to install. See `README.md` for the demo mode and a map of the code. Settings are kept in
`%APPDATA%\UnofficialUniwillControlCenter\settings.json`. Scripts that were used to test the embedded controller behavior are in
`tools\`.

Version 1 (Keyboard RGB) was written in C#; it's in the git history.
