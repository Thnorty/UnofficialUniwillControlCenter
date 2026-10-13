// Downloads on the light bar. The browser extension posts the downloads in progress to /progress about twice a
// second, and /done when one finishes. Without it, the Downloads folder shows that something is downloading, but
// not how far along it is.

fn init() {
    this.browser = [];
    this.browser_at = 0.0;
    this.folder_count = 0;
    set_tick(1000);
    this.update();
}

fn on_action(id) {
    if id == "extension" {
        open_folder("extension");
    }
}

fn on_settings() {
    this.update();
}

fn on_event(path, e) {
    if path == "/progress" && type_of(e) == "map" && type_of(e.downloads) == "array" {
        this.browser = e.downloads;
        this.browser_at = now();
    } else if path == "/done" && setting("done_flash") {
        flash(setting("done_color"), "open");
    }
    this.update();
}

fn on_tick() {
    this.update();
}

fn update() {
    // From the extension, while it keeps reporting.
    let active = [];
    if now() - this.browser_at < 5.0 {
        for d in this.browser {
            if d.state == "in_progress" && !d.paused {
                active.push(d);
            }
        }
    }
    if active.len() > 0 {
        let received = 0;
        let total = 0;
        let known = true;
        for d in active {
            received += d.received;
            if d.total > 0 {
                total += d.total;
            } else {
                known = false;
            }
        }
        if known && total > 0 {
            let percent = (received * 100) / total;
            progress(received.to_float() / total.to_float(), setting("color"));
            status(if active.len() == 1 { `${active[0].name}: ${percent}%` } else { `${active.len()} downloads: ${percent}%` });
        } else {
            activity(setting("color"));
            status(`${active.len()} downloading`);
        }
        this.folder_count = 0;
        return;
    }

    // From the Downloads folder: unfinished downloads that are still growing.
    if setting("watch_folder") {
        let growing = partial_downloads().filter(|f| f.idle < 10.0);
        if growing.len() > 0 {
            activity(setting("color"));
            status(if growing.len() == 1 { `Downloading ${growing[0].name}` } else { `${growing.len()} downloads` });
            this.folder_count = growing.len();
            return;
        }
        if this.folder_count > 0 && now() - this.browser_at >= 5.0 && setting("done_flash") {
            flash(setting("done_color"), "open"); // gone from the folder: finished (or cancelled)
        }
    }
    this.folder_count = 0;
    clear();
    status(if now() - this.browser_at < 5.0 { "The browser extension is connected. No downloads." } else { "No downloads." });
}
