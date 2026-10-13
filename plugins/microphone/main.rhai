// The microphone on the light bar: red while Windows has it muted, and if wanted a glow while an app listens.
// Read four times a second, so a mute key or a click shows at once.

fn init() {
    this.said = "";
    set_tick(250);
    this.update();
}

fn on_settings() {
    this.update();
}

fn on_tick() {
    this.update();
}

fn on_action(id) {
    if id == "toggle" {
        let mic = microphone();
        if type_of(mic) == "map" {
            set_microphone_muted(!mic.muted);
        }
        this.update();
    }
}

fn say(text) {
    if text != this.said {
        status(text);
        this.said = text;
    }
}

fn show(color) {
    if setting("style") == "Pulse" {
        pulse(color);
    } else {
        solid(color);
    }
}

fn update() {
    let mic = microphone();
    if type_of(mic) != "map" {
        clear();
        this.say("No microphone.");
        return;
    }
    if mic.muted {
        if setting("when") == "Always" || mic.in_use {
            this.show(setting("muted_color"));
        } else {
            clear();
        }
        this.say(if mic.in_use { "Muted, and an app is trying to listen." } else { "Muted." });
    } else {
        if mic.in_use && setting("show_listening") {
            this.show(setting("listening_color"));
        } else {
            clear();
        }
        this.say(if mic.in_use { "On, and an app is listening." } else { "On." });
    }
}
