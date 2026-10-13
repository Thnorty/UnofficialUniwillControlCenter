// A Pomodoro-style timer. A focus round fills the bar and a break empties it; after every few rounds the break is a
// long one.

fn init() {
    this.phase = "idle"; // "focus", "break" or "idle"
    this.round = 1;      // the focus round, from 1
    this.total = 1.0;    // the running part's length in seconds
    this.ends = 0.0;     // when it ends
    this.left = 0.0;     // seconds left while paused
    this.paused = false;
    this.waiting = false; // a part ended, and the next one waits for Start
    set_tick(1000);
    this.update();
}

fn on_action(id) {
    switch id {
        "start" => this.start(),
        "pause" => this.pause(),
        "skip" => {
            if this.phase != "idle" {
                this.next(false);
            }
        }
        "stop" => {
            this.phase = "idle";
            this.round = 1;
            this.paused = false;
            this.waiting = false;
        }
    }
    this.update();
}

fn on_settings() {
    this.update();
}

fn on_tick() {
    if this.phase != "idle" && !this.paused && !this.waiting && now() >= this.ends {
        this.next(true);
    }
    this.update();
}

fn long_break() {
    this.phase == "break" && this.round % setting("rounds") == 0
}

fn minutes() {
    if this.phase == "focus" {
        setting("focus").to_float()
    } else if this.long_break() {
        setting("long_break").to_float()
    } else {
        setting("break").to_float()
    }
}

fn color() {
    if this.phase == "focus" { setting("focus_color") } else { setting("break_color") }
}

fn begin() {
    this.total = this.minutes() * 60.0;
    this.ends = now() + this.total;
}

fn start() {
    if this.phase == "idle" {
        this.phase = "focus";
        this.round = 1;
        this.begin();
    } else if this.paused {
        this.ends = now() + this.left;
        this.paused = false;
    } else if this.waiting {
        this.waiting = false;
        this.begin();
    }
}

fn pause() {
    if this.phase != "idle" && !this.paused && !this.waiting {
        this.left = this.ends - now();
        if this.left < 0.0 {
            this.left = 0.0;
        }
        this.paused = true;
    }
}

// To the next part; `ended` when its time ran out rather than Skip.
fn next(ended) {
    if this.phase == "focus" {
        this.phase = "break";
    } else {
        this.phase = "focus";
        this.round += 1;
    }
    this.paused = false;
    this.waiting = false;
    if ended {
        flash(this.color(), "blink");
        if setting("notify") {
            let text = if this.phase == "focus" {
                `Round ${this.round}: focus for ${setting("focus")} minutes.`
            } else if this.long_break() {
                `Time for a long break: ${setting("long_break")} minutes.`
            } else {
                `Time for a break: ${setting("break")} minutes.`
            };
            notify("Focus timer", text);
        }
        if !setting("auto") {
            this.waiting = true;
            return;
        }
    }
    this.begin();
}

fn clock(seconds) {
    let total = seconds.ceiling().to_int();
    let rest = total % 60;
    `${total / 60}:${if rest < 10 { "0" } else { "" }}${rest}`
}

fn update() {
    if this.phase == "idle" {
        clear();
        status("Press Start to begin.");
        return;
    }
    let focus = this.phase == "focus";
    let name = if focus { `Focus ${this.round}` } else if this.long_break() { "Long break" } else { "Break" };
    if this.waiting {
        pulse(this.color());
        status(`${name} is next: press Start.`);
        return;
    }
    let left = if this.paused { this.left } else { this.ends - now() };
    if left < 0.0 {
        left = 0.0;
    }
    let done = 1.0 - left / this.total;
    progress(if focus { done } else { 1.0 - done }, this.color());
    status(`${name}: ${this.clock(left)} left${if this.paused { ", paused" } else { "" }}`);
}
