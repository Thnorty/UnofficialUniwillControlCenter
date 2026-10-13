// An example plugin: fills the light bar over a few seconds, again and again. PLUGINS.md lists what a script can do.

// When the plugin starts. `this` keeps the plugin's state between calls.
fn init() {
    this.start = now();
    set_tick(100); // on_tick() ten times a second
}

// The Flash button (its id is in plugin.json).
fn on_action(id) {
    if id == "flash" {
        flash(setting("color"), "blink");
    }
}

// A setting changed: start the fill over.
fn on_settings() {
    this.start = now();
}

fn on_tick() {
    if !setting("show") {
        clear();
        status("Not shown");
        return;
    }
    let done = ((now() - this.start) / setting("seconds").to_float()) % 1.0;
    let color = setting("color");
    switch setting("style") {
        "Activity" => activity(color),
        "Pulse" => pulse(color),
        _ => progress(done, color),
    }
    status(`${setting("label")}: ${(done * 100.0).to_int()}%`);
}
