# Tulpar Control plugins

A plugin shows something on the light bar: Claude Code at work, a download's progress, a build, anything that can
send an HTTP request. Plugins are small [Rhai](https://rhai.rs) scripts that run inside the app. A script can't
read files, use the network or start programs. Beyond the light bar and the app, it can only do what its listed
permissions allow, and the window shows those permissions on the plugin's card.

Two plugins come with the app:

- **Claude Code:**
  - a sweep while Claude works;
  - its task list filling the bar as tasks get done;
  - a pulse when it needs you (a permission prompt or a question);
  - a flash when it's finished.

  Press **Connect Claude Code** once. This adds HTTP hooks to `%USERPROFILE%\.claude\settings.json`, and a copy of
  the file as it was is kept next to it as `settings.json.tulpar-backup`. **Disconnect** removes only those hooks.
  The hooks post to the app while it runs. While it's closed they fail quietly, and Claude Code carries on.
- **Downloads:**
  - With the browser extension, the bar fills as a download progresses. To install it in Chrome or Edge:
    1. Open `chrome://extensions` (or `edge://extensions`) and turn on Developer mode.
    2. Choose **Load unpacked**, and pick the folder that **Open the browser extension** opens.
  - Without the extension, a sweep runs while an unfinished download (`.crdownload`, `.part`, …) is in your
    Downloads folder.

## The Plugins page

- Turn plugins on and off with each card's switch.
- Press a plugin's buttons (its actions).
- **Settings** shows the plugin's own settings, such as colors, sliders, switches, lists and text, with **Reset to
  defaults**.
- **Import plugin…** takes a `.zip`, or a folder's `plugin.json`. An imported plugin that asks for permissions starts
  off, so you see them before you turn it on. So does a new version that asks for more than before.
- **Remove** deletes an imported plugin. The two that come with the app can only be turned off.

Plugins live in `%APPDATA%\TulparControl\plugins\<id>\`, and their settings are in the app's `settings.json`.

## Making a plugin

A plugin is a folder, or a `.zip` of one, with a `plugin.json` and a script (`main.rhai` unless the manifest names
another). It can hold other files too, like the Downloads plugin's browser extension. It can have at most 500
files and 20 MB.

### plugin.json

```json
{
  "id": "build-status",
  "name": "Build status",
  "version": "1.0.0",
  "author": "You",
  "description": "Red while the build fails, a sweep while it runs.",
  "script": "main.rhai",
  "permissions": [],
  "settings": [
    { "key": "color", "label": "Running", "type": "color", "default": "#2D9CDB" },
    { "key": "flash", "label": "Flash when done", "description": "A flash in green or red.", "type": "toggle", "default": true },
    { "key": "speed", "label": "Fade", "type": "slider", "min": 0.5, "max": 5, "step": 0.5, "unit": "s", "default": 2 },
    { "key": "style", "label": "Style", "type": "choice", "options": ["Sweep", "Pulse"], "default": "Sweep" },
    { "key": "name", "label": "Project", "type": "text", "default": "" }
  ],
  "actions": [
    { "id": "test", "label": "Try it" }
  ]
}
```

- **`id`:**
  - 1–40 lowercase letters, digits and `-`;
  - it names the plugin's folder and its events address.
- **`settings`:** `type` is one of the following:
  - `toggle` (true/false);
  - `slider` (a number, with `min`, `max`, optional `step` and `unit`);
  - `color` (`"#RRGGBB"`);
  - `choice` (one of `options`);
  - `text`.

  A setting can also have a `description`.
- **`actions`:** each becomes a button on the plugin's card, and pressing one calls `on_action(id)`.
- **`permissions`:**
  - `claude-hooks`: may add and remove its own hooks in Claude Code's settings.
  - `downloads-folder`: may see the unfinished downloads in the Downloads folder (names and sizes).

### The script

The script defines functions that the app calls. All of them are optional:

| Function | Called |
|---|---|
| `init()` | When the plugin starts: with the app, or when it's turned on or imported again. |
| `on_event(path, body)` | When something posts to the plugin's address (see below). |
| `on_tick()` | Every `set_tick(ms)` milliseconds. |
| `on_settings()` | After a setting changes. |
| `on_action(id)` | When one of its buttons is pressed. |

Keep the plugin's state in `this`, which is a map kept between calls. Read settings with `setting("key")`. Helper
functions you write are called as `this.helper()` so they see the same `this`.

```rust
fn init() {
    this.running = false;
}

fn on_event(path, body) {
    if path == "/start" {
        this.running = true;
    } else if path == "/done" {
        this.running = false;
        if setting("flash") {
            flash(if body.ok { "#4CAF50" } else { "#E53935" });
        }
    }
    this.update();
}

fn on_action(id) {
    flash(setting("color"), "blink");
}

fn update() {
    if this.running {
        activity(setting("color"));
        status("Building…");
    } else {
        clear();
        status("Idle");
    }
}
```

### What a script can do

On the light bar, a plugin's layer covers the current effect. When the layer is cleared the effect comes back. If
several plugins show something, the one that started showing it last is on top.

| Function | What it does |
|---|---|
| `progress(value, color)` | Fills the bar from the left; `value` is 0 to 1. |
| `activity(color)` | A sweep back and forth, for work without a known end. |
| `pulse(color)` | The whole bar breathes, for "needs you". |
| `solid(color)` | The whole bar in one color. |
| `leds(colors)` | Each LED's color from an array, left to right; the bar has 23. Missing ones are dark. |
| `clear()` | Removes this plugin's layer. |
| `flash(color)`, `flash(color, style)` | A short flash over everything; `style` is `"open"`, `"close"` or `"blink"`. |
| `status(text)` | The line on the plugin's card. |
| `notify(title, text)` | A Windows notification (at most one per 5 seconds). |
| `set_tick(ms)` | Calls `on_tick()` every `ms` milliseconds (at least 30); `0` stops it. |
| `now()` | Seconds since 1970, with fractions. |
| `setting(key)`, `settings()` | One setting, or all of them as a map. |
| `plugin_url()` | The plugin's events address. |
| `open_folder(name)` | Opens a folder inside the plugin's own folder in Explorer. |
| `partial_downloads()` | Needs `downloads-folder`. The unfinished downloads: maps with `name`, `size` and `idle` (seconds since the last write). |
| `install_claude_hooks(events)`, `remove_claude_hooks()`, `claude_hooks_installed()` | Need `claude-hooks`. Add or remove hooks that post to this plugin, or count how many there are. |

Colors are `"#RRGGBB"` strings. A call that runs too long is stopped after 2 million operations. An error in `init`
stops the plugin. An error in a later call is shown on the plugin's card, and the plugin keeps running. `eval`
isn't available.

### Sending events

While the app runs it listens on `http://127.0.0.1:47823`, on this PC only. Use a POST with a JSON body:

```
POST http://127.0.0.1:47823/plugins/<id>/<path>
Content-Type: application/json
```

The plugin's `on_event` gets `"/<path>"` (or `""`) and the body as a map. The app answers 200 even when the plugin is
off. It refuses requests from web pages (those with an `Origin` that isn't a browser extension) and bodies over
1 MB. From PowerShell:

```powershell
Invoke-RestMethod -Method Post -Uri http://127.0.0.1:47823/plugins/build-status/done -ContentType application/json -Body '{"ok": true}'
```

In demo mode (`--demo`), plugins run but receive no events. What their permissions allow is only pretended: no
downloads are seen and Claude Code's settings aren't changed.
