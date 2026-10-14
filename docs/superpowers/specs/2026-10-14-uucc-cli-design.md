# `uucc`: a terminal command for Unofficial Uniwill Control Center

Date: 2026-10-14. Target version: 3.4.0.

## Goal

Let people read the laptop's status and change its main settings from a terminal, for three uses the owner named:

- **Scripts and automation:** Task Scheduler, AutoHotkey, Stream Deck and similar tools calling a command, with
  output a script can parse and an exit code it can check.
- **Reading status:** the readings the Monitor page shows, and the current settings, for a status bar or a script.
- **Quick changes by hand:** typing a command instead of opening the window.

Full parity with the window (fan curves, CPU/GPU tuning, plugins, timers) is not a goal of this version.

Success: from an ordinary (non-administrator) terminal, `uucc mode turbo` switches the running app to Turbo exactly as
clicking the Turbo card does (the window, tray and pop-up follow), `uucc status --json` prints the readings, and every
failure gives a clear message and a documented exit code.

## Approach

A separate console program, `uucc.exe`, talks to the running app over a Windows named pipe. The app keeps doing all the
hardware work: it runs with administrator rights and owns the keyboard, light bar, fans and EC, so a second process
must never touch them. Rejected alternatives:

- Arguments to the app's own exe: it is a windowed (GUI subsystem) program, so `cmd` and PowerShell don't wait for it,
  its output lands in the middle of the prompt, and exit codes are unreliable.
- A JSON API on the plugins' HTTP port (127.0.0.1:47823): every program on the PC, including other Windows users',
  could change fans and power limits.

## Commands

A command without a value prints the current setting; with a value it sets it and prints the result.

| Command | Does |
|---|---|
| `uucc status` | Everything at once: mode, Fan Boost, the mode's profile, CPU and GPU temperature and load (GPU "off" while it sleeps), fan speeds, battery level and state, charge mode, keyboard and light bar lighting, display, GPU mode. |
| `uucc mode [office\|gaming\|turbo]` | The performance mode. Turbo only where the laptop has it. |
| `uucc fan-boost [on\|off]` | Fan Boost. |
| `uucc profile [NAME]` | Lists the current mode's profiles (marking the active one), or switches to one. |
| `uucc keyboard [on\|off] [--effect NAME] [--brightness 0-100] [--speed SPEED] [--colors HEX,HEX…]` | The keyboard's lighting. |
| `uucc lightbar [on\|off] [same options] [--linked on\|off]` | The light bar's lighting; `--linked` is "Same as the keyboard". Fails with code 3 on a laptop without one. |
| `uucc effects` | The effects each device supports: ID, name, and how many colors each takes. |
| `uucc charge [full\|balanced\|long-life]` | The battery charge mode (High capacity, Balanced, Stationary in the app). |
| `uucc setup [NAME]` | Lists the saved setups, or applies one, as the Setups page's Apply does. |
| `uucc display [on\|off] [--mode NAME]` | Display colors on or off, and the mode: Vibrant, Cinema, Low blue light, Custom. |
| `uucc gpu-mode [hybrid\|intel\|nvidia]` | The GPU mode (MUX switch). Setting it says a restart is needed, as the window does. Code 3 where the laptop has no MUX. |

Values:

- **Names** (modes, profiles, setups, effects, display modes, charge modes) match case-insensitively, and a unique
  prefix is enough (`uucc mode t`). An unknown or ambiguous name fails with code 1 and lists the valid ones. Effects
  match by ID (`starspark`) or name (`"Star Spark"`).
- **`--speed`**: `slowest`, `slow`, `medium`, `fast`, `fastest`, or 1-5.
- **`--colors`**: `#rrggbb` or `rrggbb`, separated by commas: the effect's colors, as the window edits them (the
  zones' colors for Static, the palette for effects that have one). Each effect takes between its own minimum and
  maximum number of colors, which `uucc effects` shows; too few or too many fails with code 1. For an effect without
  colors it fails with code 1.
- Options on the lighting commands can be combined; they are applied in the order effect, colors, brightness, speed,
  on/off, linked, and if one fails the ones after it aren't applied.

Options for every command:

- `--json`: prints the reply as JSON (one object) instead of text.
- `--start`: if the app isn't running, starts it in the tray first. Without it, `uucc` fails with code 2.
- `--help` (also `uucc help [COMMAND]`) and `--version`.

Exit codes:

| Code | Meaning |
|---|---|
| 0 | Done. |
| 1 | Bad command or value. The message says what's valid. |
| 2 | The app isn't running (and `--start` wasn't given), or it didn't start within 15 s. |
| 3 | The app couldn't do it: the feature isn't on this laptop, the hardware refused, or the app is busy (for example, Control Center's service has the Fn keys). It prints the app's own message. |

## Architecture

### Programs and code layout

- `src/bin/uucc.rs`: the console program (console subsystem). Parses arguments, connects to the pipe, sends one
  request, formats the reply as text or JSON, sets the exit code. It doesn't link Slint or any hardware code.
- `src/cli/protocol.rs`: the request and reply types, shared by both programs (`uucc.rs` includes it with
  `#[path]`; the app with `mod cli`). Plain serde enums, serialized as one JSON object per message:

  ```rust
  enum Request { Status, Mode(Option<String>), FanBoost(Option<bool>), Profile(Option<String>),
                 Lighting { device: Device, changes: LightingChanges }, Effects, Charge(Option<String>),
                 Setup(Option<String>), Display { on: Option<bool>, mode: Option<String> }, GpuMode(Option<String>) }
  struct Envelope { version: u32, request: Request }                   // version 1
  enum Reply { Ok(Value), Error { code: u8, message: String } }        // Value: the command's result
  ```

  Names travel as typed by the user; the app resolves them, since only it knows the profiles, setups and effects.
  The result of each command is a small struct (e.g. `ModeResult { mode, available }`) defined in `protocol.rs`, so
  `uucc` formats it and `--json` prints it unchanged.
- `src/cli/server.rs` (app side): the pipe server thread.
- `src/app/cli.rs` (app side): `App::run_cli(Request) -> Reply`, which resolves names, calls the existing App methods
  and builds the results.

### The pipe

- Name: `\\.\pipe\UnofficialUniwillControlCenter` (the demo: `\\.\pipe\UnofficialUniwillControlCenter.Demo`).
- Created by the app with `FILE_FLAG_FIRST_PIPE_INSTANCE` (if another program holds the name, the app logs it and
  runs without the CLI rather than share it) and `PIPE_REJECT_REMOTE_CLIENTS`.
- Security descriptor: full access for the app's own user (the SID of its token, which is the signed-in user, since the
  app runs elevated as that user) and nobody else, with a **Medium** mandatory integrity label (`S:(ML;;NW;;;ME)`).
  The label is needed because an object an elevated process creates gets a High label by default, which stops the
  user's normal (Medium) programs from writing to it.
- One request per connection: `uucc` writes one JSON line, the app writes one JSON line back and closes. Requests are
  limited to 64 KB; a client that sends nothing for 5 s is dropped.

### Inside the app

1. The server thread accepts a connection and reads the request.
2. It passes it to the UI thread with `slint::invoke_from_event_loop` and `with_app`, exactly as window callbacks
   run, and waits for the reply on a channel (up to 15 s, then code 3, "The app didn't answer").
3. `App::run_cli` resolves names against the app's state, then calls the same methods the window's callbacks call:
   `set_mode`, `set_fan_boost`, `select_profile`, `select_effect`, `set_light`, `set_speed`, color and palette edits,
   `set_light_bar_on`, `set_linked`, `set_charge_mode`, `apply_setup`, `set_display_on`, `set_display_mode`,
   `set_gpu_mode`. So the window, tray and pop-ups follow as if clicked, and settings are saved the same way.
4. **Errors:** the app reports failures by showing a message at the top of the window (`set_status` with `error`).
   While `run_cli` runs, `set_status` also records an error message into a field `cli_error`; if one is recorded,
   the reply is code 3 with that message. Features that aren't available (Turbo, a light bar, a MUX, BIOS settings
   that can't be written) are checked up front and give code 3 with a message saying so.
5. **Results** are read back from the app's state after the change, so they show what the laptop is set to now (for
   example, the mode actually applied).

### Readings for `uucc status`

The monitor thread only samples (once a second) while a page that shows readings is open. A status request sets
`monitor.wanted_until` to 5 s from now (the thread samples while either the page wants it or that time hasn't
passed), and the server thread, not the UI thread, waits up to 2.5 s for a sample taken after the request, then asks
the UI thread for the reply. CPU load needs two samples; if the first fresh sample has none, it waits for one more.
Values that can't be read are `null` in JSON and "–" in text. The GPU shows "off" while it sleeps (it isn't woken).

### `--start`

`uucc` looks for `UnofficialUniwillControlCenter.exe` in its own folder (as installed). If the startup task exists,
it runs it (`schtasks /Run`, no UAC prompt); otherwise it starts the exe with `--tray` through `runas` (a UAC
prompt). It then waits up to 15 s for the pipe to accept a connection. A first start on a laptop that has never
accepted the app's risk notice shows that notice, as any first start does.

### Demo

The demo app (`--demo`) serves the demo pipe, with its fake hardware. `uucc` uses that pipe when the environment
variable `UUCC_DEMO=1` is set. This is for testing without administrator rights and without touching the hardware;
it isn't documented in the README.

## Installer

- `uucc.exe` is installed next to the app.
- A new task on the setup choices page: "Add the uucc command to PATH (for terminals and scripts)", ticked by default.
  It adds `{app}` to the system PATH (`HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment`, `Path`),
  only if it isn't there, and broadcasts the environment change. Updates keep the choice (the tasks page is skipped
  on an update and the previous tasks are used; an install from before 3.4.0 gets the default, ticked).
- The uninstaller removes `{app}` from PATH again.
- `installer/build.ps1` builds both programs (`cargo build --release` already builds every binary target).

## Docs

- README: a "Terminal (uucc)" section with one example per command, the options, and the exit codes.
- `uucc --help` and `uucc help COMMAND` print the same usage text.
- FEATURES.md: a line under App behavior.

## Testing

- **Unit tests (uucc):** parsing every command and option, prefixes, `--speed` names and numbers, `--colors` parsing,
  bad values giving exit code 1 and the right message.
- **Unit tests (protocol):** each request and reply round-trips through JSON, and the JSON of a few is pinned, so the
  two programs can't drift apart.
- **Unit tests (app):** name resolution (exact, case-insensitive, prefix, ambiguous, unknown).
- **End to end against the demo:** the demo app plus `UUCC_DEMO=1 uucc …` for every command, including errors,
  `--json` output, and the app not running (code 2).
- **Access from a normal terminal to the elevated app:** one read-only `uucc status` run by the owner from a
  non-administrator terminal against the real app, to confirm the pipe's security descriptor.

## Out of scope

Fan curves, power limits, TCC and GPU tuning, the voltage offset, plugins, timers and alerts, app profiles, settings
import and export, and watching readings continuously (`status` prints one sample; a script can loop).
