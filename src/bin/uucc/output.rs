//! Replies, as text for people.

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::args::SPEEDS;
use crate::protocol::{
    ChargeResult, DisplayResult, EffectInfo, EffectsResult, FanBoostResult, FanResult, GpuModeResult, LightingResult, ModeResult,
    ProfileResult, Request, SetupResult, StatusResult,
};

/// The reply to `request` as text. A reply that doesn't have the expected shape is shown as JSON.
pub fn text(request: &Request, result: &Value) -> String {
    fn as_<T: DeserializeOwned>(value: &Value) -> Option<T> {
        serde_json::from_value(value.clone()).ok()
    }
    let shown = match request {
        Request::Status => as_(result).map(status),
        Request::Mode(_) => as_(result).map(|r: ModeResult| format!("Mode: {} (this laptop has {})", r.mode, r.available.join(", "))),
        Request::FanBoost(_) => as_(result).map(|r: FanBoostResult| format!("Fan Boost: {}", on_off(r.on))),
        Request::Profile(_) => as_(result).map(|r: ProfileResult| {
            let list = r.profiles.iter().map(|p| format!("{} {p}", if *p == r.active { "*" } else { " " })).collect::<Vec<_>>();
            format!("{} mode profiles:\n{}", r.mode, list.join("\n"))
        }),
        Request::Lighting { .. } => as_(result).map(|r: LightingResult| lighting(&r)),
        Request::Effects => as_(result).map(|r: EffectsResult| {
            let mut text = format!("Keyboard effects:\n{}", effect_list(&r.keyboard));
            if let Some(light_bar) = &r.light_bar {
                text += &format!("\n\nLight bar effects:\n{}", effect_list(light_bar));
            }
            text
        }),
        Request::Charge(_) => as_(result).map(|r: ChargeResult| format!("Charge mode: {}", r.charge)),
        Request::Setup(_) => as_(result).map(|r: SetupResult| match r.applied {
            Some(name) => format!("Applied the setup {name}"),
            None if r.setups.is_empty() => "No setups saved yet (save one on the Setups page).".into(),
            None => format!("Setups:\n{}", r.setups.iter().map(|s| format!("  {s}")).collect::<Vec<_>>().join("\n")),
        }),
        Request::Display { .. } => as_(result).map(|r: DisplayResult| display(&r)),
        Request::GpuMode(_) => as_(result).map(|r: GpuModeResult| {
            let restart = if r.restart_needed { "\nRestart the laptop to switch to it." } else { "" };
            format!("GPU mode: {}{restart}", r.mode)
        }),
    };
    shown.unwrap_or_else(|| serde_json::to_string_pretty(result).unwrap_or_default())
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// A reading, or "–" when it couldn't be read.
fn reading<T>(value: Option<T>, show: impl Fn(T) -> String) -> String {
    value.map(show).unwrap_or_else(|| "–".into())
}

fn fan(fan: Option<FanResult>) -> String {
    reading(fan, |f| format!("{} rpm ({}%)", f.rpm, f.duty))
}

fn lighting(r: &LightingResult) -> String {
    let device = match r.device {
        crate::protocol::Device::Keyboard => "Keyboard",
        crate::protocol::Device::LightBar => "Light bar",
    };
    if !r.on {
        return format!("{device}: off");
    }
    if r.linked == Some(true) {
        return format!("{device}: same as the keyboard, brightness {}%", r.brightness);
    }
    let mut parts = vec![r.effect_name.clone(), format!("brightness {}%", r.brightness)];
    if let Some(speed) = r.speed {
        parts.push(format!("speed {}", SPEEDS.get(speed as usize).unwrap_or(&"?")));
    }
    if !r.colors.is_empty() {
        parts.push(format!("colors {}", r.colors.join(" ")));
    }
    format!("{device}: {}", parts.join(", "))
}

fn effect_list(effects: &[EffectInfo]) -> String {
    let width = effects.iter().map(|e| e.id.len()).max().unwrap_or(0);
    effects
        .iter()
        .map(|e| {
            let colors = match (e.min_colors, e.max_colors) {
                (_, 0) => String::new(),
                (min, max) if min == max => format!(", {min} color{}", if min == 1 { "" } else { "s" }),
                (min, max) => format!(", {min}-{max} colors"),
            };
            let speed = if e.has_speed { ", speed" } else { "" };
            format!("  {:width$}  {}{colors}{speed}", e.id, e.name)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn display(r: &DisplayResult) -> String {
    if r.on { format!("Display colors: {}", r.mode) } else { "Display colors: off".into() }
}

fn status(s: StatusResult) -> String {
    let cpu = format!(
        "{}, load {}, fan {}",
        reading(s.cpu_temperature, |t| format!("{t:.0} °C")),
        reading(s.cpu_load, |l| format!("{:.0}%", l * 100.0)),
        fan(s.cpu_fan)
    );
    let gpu_temperature = if s.gpu_off { "off (asleep)".to_string() } else { reading(s.gpu_temperature, |t| format!("{t:.0} °C")) };
    let battery = format!(
        "{}{}{}, charge mode {}",
        reading(s.battery_percent, |p| format!("{p}%")),
        if s.charging { ", charging" } else { "" },
        if s.on_ac { ", on the charger" } else { ", on battery" },
        s.charge
    );
    let mut lines = vec![
        format!("Mode:      {} (profile {}), Fan Boost {}", s.mode, s.profile, on_off(s.fan_boost)),
        format!("CPU:       {cpu}"),
        format!("GPU:       {gpu_temperature}, fan {}", fan(s.gpu_fan)),
        format!("Battery:   {battery}"),
        lighting(&s.keyboard),
    ];
    if let Some(light_bar) = &s.light_bar {
        lines.push(lighting(light_bar));
    }
    lines.push(display(&s.display));
    if let Some(mode) = &s.gpu_mode {
        lines.push(format!("GPU mode:  {mode}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Request, SetupResult};
    use serde_json::json;

    #[test]
    fn mode_shows_the_mode() {
        let shown = text(&Request::Mode(None), &json!({"mode": "Turbo", "available": ["Office", "Gaming", "Turbo"]}));
        assert!(shown.contains("Turbo"), "{shown}");
    }

    #[test]
    fn names_stay_intact() {
        let result = SetupResult { setups: vec!["Gece Modu – Sessiz".into()], applied: None };
        let shown = text(&Request::Setup(None), &serde_json::to_value(result).unwrap());
        assert!(shown.contains("Gece Modu – Sessiz"), "{shown}");
    }

    #[test]
    fn status_shows_missing_and_off() {
        let status = json!({
            "mode": "Gaming", "fan_boost": false, "profile": "Standard",
            "cpu_temperature": 61.4, "cpu_load": null, "gpu_temperature": null, "gpu_off": true,
            "cpu_fan": {"rpm": 3100, "duty": 40}, "gpu_fan": null,
            "battery_percent": 80, "charging": true, "on_ac": true, "charge": "balanced",
            "keyboard": {"device": "keyboard", "on": true, "effect": "static", "effect_name": "Static", "brightness": 60,
                         "speed": null, "colors": ["#ff0000", "#ff0000", "#ff0000", "#ff0000"], "linked": null},
            "light_bar": null,
            "display": {"on": false, "mode": "Vibrant", "modes": ["Vibrant", "Cinema", "Low blue light", "Custom"]},
            "gpu_mode": "hybrid"
        });
        let shown = text(&Request::Status, &status);
        assert!(shown.contains("61 °C"), "{shown}");
        assert!(shown.contains('–'), "{shown}");
        assert!(shown.contains("off"), "{shown}");
        assert!(shown.contains("80%"), "{shown}");
    }
}
