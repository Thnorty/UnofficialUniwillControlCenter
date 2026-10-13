// The bar pulses while the CPU or the GPU is over its limit, until it's 5 °C below the limit again, so a reading
// going back and forth across the limit doesn't make it flicker.

fn init() {
    this.hot = false;
    set_tick(2000);
    this.update();
}

fn on_settings() {
    this.update();
}

fn on_tick() {
    this.update();
}

fn update() {
    let readings = [["CPU", cpu_temperature(), setting("cpu_limit").to_float()]];
    if setting("watch_gpu") {
        readings.push(["GPU", gpu_temperature(), setting("gpu_limit").to_float()]);
    }
    let margin = if this.hot { 5.0 } else { 0.0 };
    let shown = [];
    let over = [];
    for r in readings {
        if type_of(r[1]) == "()" {
            continue; // no reading: the GPU is off, or this PC can't say
        }
        let text = `${r[0]} ${r[1].round().to_int()} °C`;
        shown.push(text);
        if r[1] >= r[2] - margin {
            over.push(text);
        }
    }
    let hot = over.len() > 0;
    if hot && !this.hot && setting("notify") {
        notify("Too hot", `${over[0]}${if over.len() > 1 { ` and ${over[1]}` } else { "" }}: over the limit.`);
    }
    this.hot = hot;
    if hot {
        pulse(setting("color"));
    } else {
        clear();
    }
    if shown.len() == 0 {
        status("No temperature reading.");
    } else {
        let text = shown[0];
        if shown.len() > 1 {
            text += ` · ${shown[1]}`;
        }
        status(if hot { `${text}: over the limit.` } else { `${text}. The bar pulses from ${setting("cpu_limit")} °C.` });
    }
}
