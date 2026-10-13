// Flashes the bar for new notifications. Once a second it looks at the notification center; a notification it
// hasn't seen before, from a chosen app, gets a flash. The ones there when the plugin starts don't.

fn init() {
    this.seen = #{};
    this.first = true;
    this.last = "";
    set_tick(1000);
    this.check();
}

fn on_tick() {
    this.check();
}

fn on_settings() {
    this.check();
}

// The color for an app's notifications, or () if it isn't one of the chosen apps.
fn color_for(app) {
    let apps = setting("apps");
    apps.trim();
    if apps == "" {
        return setting("color");
    }
    let name = app.to_lower();
    for item in apps.split(",") {
        let parts = item.split("=");
        let wanted = parts[0];
        wanted.trim();
        if wanted != "" && name.contains(wanted.to_lower()) {
            if parts.len() > 1 {
                let color = parts[1];
                color.trim();
                return color;
            }
            return setting("color");
        }
    }
    ()
}

fn check() {
    let list = notifications();
    if type_of(list) != "array" {
        status("Windows doesn't let apps read notifications: turn on Notification access in Settings › Privacy & security › Notifications.");
        return;
    }
    let seen = #{};
    for n in list {
        let key = `${n.id}`;
        seen[key] = true;
        if !this.first && !(key in this.seen) {
            let color = this.color_for(n.app);
            if type_of(color) != "()" {
                flash(color, setting("style").to_lower());
                this.last = n.app;
            }
        }
    }
    this.seen = seen;
    this.first = false;
    let apps = setting("apps");
    apps.trim();
    let watching = if apps == "" { "Flashes for every app" } else { `Flashes for ${apps}` };
    status(if this.last == "" { `${watching}.` } else { `${watching}. The last one: ${this.last}.` });
}
