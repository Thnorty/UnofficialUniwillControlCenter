// What's playing on the light bar: the song's progress, and the volume for two seconds after it changes. The volume
// is read ten times a second so a change shows at once; the song twice a second.

fn init() {
    this.level = ();      // the last volume, 0-1
    this.muted = false;
    this.volume_until = 0.0;
    this.song = ();
    this.song_read = 0.0;
    this.said = "";
    set_tick(100);
}

fn on_settings() {
    this.volume_until = 0.0;
}

fn say(text) {
    if text != this.said {
        status(text);
        this.said = text;
    }
}

fn clock(seconds) {
    let total = seconds.to_int();
    let rest = total % 60;
    `${total / 60}:${if rest < 10 { "0" } else { "" }}${rest}`
}

fn on_tick() {
    let t = now();
    let v = volume();
    if type_of(v) == "map" {
        if type_of(this.level) != "()" && ((v.level - this.level).abs() > 0.004 || v.muted != this.muted) {
            this.volume_until = t + 2.0;
        }
        this.level = v.level;
        this.muted = v.muted;
    }
    if t - this.song_read >= 0.5 {
        this.song = media();
        this.song_read = t;
    }

    if setting("show_volume") && t < this.volume_until {
        progress(this.level, if this.muted { setting("mute_color") } else { setting("volume_color") });
        this.say(if this.muted { "Muted" } else { `Volume ${(this.level * 100.0).round().to_int()}%` });
        return;
    }

    let s = this.song;
    if type_of(s) != "map" {
        clear();
        this.say("Nothing playing.");
        return;
    }
    if setting("show_song") && s.playing && s.duration > 0.0 {
        progress(s.position / s.duration, setting("song_color"));
    } else {
        clear();
    }
    let name = if s.title == "" { s.app } else if s.artist == "" { s.title } else { `${s.title} – ${s.artist}` };
    let time = if s.duration > 0.0 { ` · ${this.clock(s.position)} / ${this.clock(s.duration)}` } else { "" };
    this.say(`${name}${time}${if s.playing { "" } else { " (paused)" }}`);
}
