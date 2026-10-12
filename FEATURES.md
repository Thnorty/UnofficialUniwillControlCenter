# Keyboard RGB — feature overview

A replacement for the lighting part of Control Center (v4.8.47.13, installed as Control Center 3 4.9.47.11) on the
Monster Tulpar T5 V21.4. This is a Uniwill/Tongfang platform with two ITE lighting controllers:
- the four-zone keyboard (ITE 8291, USB 048D:CE00, firmware 11.04);
- the light bar on the front edge (USB 048D:6005, firmware 15.04, Control Center's `MEZone_Lighbar` type).

It talks to the keyboard controller directly over HID, and to the embedded controller through the BIOS WMI interface.
It does not need Control Center, its GCUBridge/GCUService service or its MQTT broker. It needs administrator rights;
the "Start with Windows" option starts it elevated in the tray through Task Scheduler.

## From Control Center

These work the way Control Center does them. The commands come from Control Center's decompiled service.

### Keyboard lighting (four-zone)
- **Effects:** Static (four zone colors), Breathing, Wave, Rainbow, Flash, Mix and Music: the full list Control Center
  offers for this keyboard.
- **Speed:** five steps, for every animated effect.
- **Wave direction:** left → right or right → left.
- **Effect colors:** Control Center's default seven-color palette for Breathing, Wave, Flash and Mix.
- **Music:** the four zone colors stay, and the brightness follows the peak level of the default audio output,
  with automatic gain.
- **White balance:** the same green and blue scaling Control Center applies, so colors look the same.
- **Saved in the controller:** the final lighting is saved to the controller when the app exits, so it stays without
  the app running.
### Light bar
- **On/off switch**, and the light bar's Fn toggle key (hotkey code 175), with a pop-up.
- **Effects:** the firmware effects Control Center's light bar driver handles: Static color, Breathing, Wave
  (Control Center's "Colorwave", which has no direction), Rainbow, Flash, Mix and Thinking.
- **Music:** a level meter that grows from the middle of the bar outwards with the sound, set LED by LED (user
  mode, command 12h and output reports), colored by distance from the middle with up to seven colors.
- **Speed and brightness**, as on the keyboard.
- **Color correction:** the light bar's own correction (`WKDColor.cheatRGB_HIDLightbar`).
- **First run:** starts from what the light bar was showing (Control Center left it off, on Thinking).

### Shared
- **Idle timer:** switches the backlight and the light bar off after a chosen time without keyboard or mouse input (Never,
  15 s … 1 h), with the same fade-out, and back on with the next input.

### Fn keys and pop-ups
- **Fn keyboard-light keys** (down, up, cycle): handled instantly. The app takes them over from the firmware the way
  Control Center's service does, by setting the "app present" bit in the embedded controller.
- **Windows key lock:** shows the current state and has a button to toggle it.
- **Touchpad:** toggles on and off, and the button follows the touchpad's Fn shortcut.
- **Pop-ups (OSD):** for keyboard brightness, touchpad on/off and Windows key lock/unlock.

### App behavior
- **Tray:** runs in the tray and can start with Windows.
- **Coexists with Control Center:** steps back and leaves the Fn keys alone while Control Center's service is running.

## Added beyond Control Center

- **No Control Center needed:** no service, UWP app or broker. It is a single small exe.
- **Fine brightness:** a 0–100% slider covering the controllers' full 0–50 range, for the keyboard and the light
  bar. Control Center only uses five levels.
- **Smooth brightness:** every brightness change fades (about 0.18 s) instead of jumping: the slider, the Fn keys,
  and coming back from the idle timer.
- **Fn keys step by 20%:** 10 of 50 per press. The cycle key wraps to off after full brightness.
- **Editable effect colors:**
  - each palette effect can have 1–7 colors;
  - click a color to change it, right-click to remove it, **+** to add one, **Default** to reset;
  - each effect remembers its own colors.
- **Zone shortcuts:** "Set all" and ten quick color presets for the four zones.
- **Music controls:** the brightness slider sets the peak brightness, and the speed slider sets how fast the light
  falls back.
- **Pop-up style:** a continuous bar with a percentage. It follows the light/dark theme, doesn't take focus or
  block clicks, and appears on the screen with the mouse pointer.
- **Tray icon colors:** the tray and window icon show the current effect's colors.
- **Restore on start:**
  - restores all settings when it starts;
  - keeps a brightness changed with the Fn keys while the app was closed;
  - migrates settings from older versions of this app.
- **Brightness kept on exit:** about a second after the Fn keys are handed back, the embedded controller sets its
  own brightness. Control Center leaves the keyboard that way; this app puts your brightness back.
- **Fn keys handed back safely:** they go back to the firmware even if the app crashes, and are taken over again
  after sleep or resume.

## Still missing

### Keyboard lighting
- **Save to BIOS:** Control Center's boot-time lighting effect, which writes the `RGBKeyboard08` UEFI variable and
  the controller's "welcome" slots. Left out on purpose because it writes firmware variables.
- **Separate brightness on charger and battery:** Control Center's service keeps one brightness for each and
  switches when the charger is plugged in or out. This app has one brightness.
- **Aurora:** not the light bar. Control Center's Aurora switch hands all lighting over to a separate third-party
  RGB program called Aurora, which isn't installed here. Not implemented.
- **Light bar Star Spark and Star Hitting:** Control Center drives these in software, one LED at a time. Not
  implemented yet; the LED-by-LED mode they need is now in place (used by Music).
- **Night Light:** removed. It had no visible effect on this laptop, and Control Center doesn't offer it for this
  model.
- **Fn keys while the app is closed:** they still need several presses. The embedded controller counts from a hidden
  internal brightness level that hasn't been found.
- **Effects for other keyboard types:** Control Center's service also has effects for per-key and other keyboard
  types (Reactive, Ripple, Raindrop, Marquee, Spark, Neon, Gaming, per-key custom, …). Control Center doesn't offer
  them for four-zone keyboards, so they probably don't apply here.

### Rest of Control Center

None of this is implemented. I haven't checked which of these Control Center actually shows for this model.
- **Performance profiles:** Silent, Balanced, Performance, Enthusiast and Overboost, plus the performance-profile
  pop-up.
- **Fans:** Fan Boost and custom fan control.
- **CPU and GPU tuning:** power limits and temperature targets, GPU overclocking, Dynamic Boost and NVIDIA Whisper
  Mode.
- **GPU modes:** dGPU mode (Hybrid, iGPU, Auto) and disabling NVIDIA Optimus.
- **Battery:**
  - charging profiles (Eco, Balanced, High Capacity) and battery calibration;
  - USB-C power delivery priority, powered USB while off, automatic boot on AC.
- **Display:** refresh rate, color modes and calibration, low blue light.
- **Monitoring:** system dashboard, hardware monitoring, In-Game OSD and Quick Information Box.
- **Keys:**
  - Fn Lock;
  - Caps Lock, Num Lock and Scroll Lock pop-ups;
  - key remapping, Game Mode and application profiles.
- **Device switches:** Wi-Fi, Bluetooth and webcam, and touchpad active-area settings.
- **Light bars and LED strips:** only on models that have them.
- **Firmware updates** for the keyboard controller.

## Building

```
dotnet publish -c Release -r win-x64 --self-contained false -p:PublishSingleFile=true -o publish
```

The output is `publish\MonsterKeyboardRGB.exe`, which needs the .NET 10 Desktop Runtime. Settings are kept in
`%APPDATA%\MonsterKeyboardRGB\settings.json`. Scripts that were used to test the embedded controller behavior are
in `tools\`.
