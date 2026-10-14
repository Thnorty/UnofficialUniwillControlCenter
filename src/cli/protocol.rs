//! What `uucc` and the app send each other over the pipe: one JSON line each way. Both programs compile this file
//! (`uucc` through `#[path]`), so it uses nothing from either crate.

use std::io::{self, BufRead, BufReader, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Changes when a request or reply changes shape, so an old `uucc` and a new app (or the other way round) say so.
pub const VERSION: u32 = 1;
/// The longest message either side reads.
pub const MAX_MESSAGE: usize = 64 * 1024;

/// The app's pipe; the demo (`--demo`) has its own, which `uucc` uses when `UUCC_DEMO=1`.
pub fn pipe_name(demo: bool) -> &'static str {
    if demo { r"\\.\pipe\UnofficialUniwillControlCenter.Demo" } else { r"\\.\pipe\UnofficialUniwillControlCenter" }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Envelope {
    pub version: u32,
    pub request: Request,
}

/// One command. `None` asks for the current value; names travel as typed, since only the app knows its profiles,
/// setups, effects and display modes.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Request {
    Status,
    Mode(Option<String>),
    FanBoost(Option<bool>),
    Profile(Option<String>),
    Lighting { device: Device, changes: LightingChanges },
    Effects,
    Charge(Option<String>),
    Setup(Option<String>),
    Display { on: Option<bool>, mode: Option<String> },
    GpuMode(Option<String>),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Device {
    Keyboard,
    LightBar,
}

/// What a `keyboard` or `lightbar` command changes; nothing given only reads.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default)]
pub struct LightingChanges {
    pub effect: Option<String>,
    /// `#rrggbb`, lowercase.
    pub colors: Option<Vec<String>>,
    /// Percent, 0-100.
    pub brightness: Option<u8>,
    /// 0 (slowest) to 4 (fastest).
    pub speed: Option<u8>,
    pub on: Option<bool>,
    pub linked: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Reply {
    /// The command's result: one of the `…Result` structs below.
    Ok(serde_json::Value),
    /// `code` is `uucc`'s exit code: 1 a bad value, 3 the app couldn't do it.
    Error { code: u8, message: String },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ModeResult {
    pub mode: String,
    pub available: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FanBoostResult {
    pub on: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ProfileResult {
    pub mode: String,
    pub profiles: Vec<String>,
    pub active: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LightingResult {
    pub device: Device,
    pub on: bool,
    /// The effect's ID, such as `starspark`.
    pub effect: String,
    pub effect_name: String,
    pub brightness: u8,
    /// 0-4, for effects that have a speed.
    pub speed: Option<u8>,
    pub colors: Vec<String>,
    /// The light bar's "Same as the keyboard".
    pub linked: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct EffectInfo {
    pub id: String,
    pub name: String,
    pub min_colors: usize,
    pub max_colors: usize,
    pub has_speed: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct EffectsResult {
    pub keyboard: Vec<EffectInfo>,
    pub light_bar: Option<Vec<EffectInfo>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ChargeResult {
    /// `full`, `balanced` or `long-life`.
    pub charge: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SetupResult {
    pub setups: Vec<String>,
    pub applied: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DisplayResult {
    pub on: bool,
    pub mode: String,
    pub modes: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct GpuModeResult {
    /// `hybrid`, `intel` or `nvidia`.
    pub mode: String,
    pub restart_needed: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct FanResult {
    pub rpm: u16,
    /// Percent.
    pub duty: u8,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct StatusResult {
    pub mode: String,
    pub fan_boost: bool,
    pub profile: String,
    /// °C.
    pub cpu_temperature: Option<f32>,
    /// 0-1.
    pub cpu_load: Option<f32>,
    pub gpu_temperature: Option<f32>,
    /// The NVIDIA GPU is asleep (its temperature isn't read, so it isn't woken).
    pub gpu_off: bool,
    pub cpu_fan: Option<FanResult>,
    pub gpu_fan: Option<FanResult>,
    pub battery_percent: Option<u8>,
    pub charging: bool,
    pub on_ac: bool,
    pub charge: String,
    pub keyboard: LightingResult,
    pub light_bar: Option<LightingResult>,
    pub display: DisplayResult,
    pub gpu_mode: Option<String>,
}

/// Which of `names` the user means: the same name in any case, or else the only one it starts.
#[cfg(test)]
pub fn resolve(input: &str, names: &[&str]) -> Result<usize, String> {
    resolve_kind("value", input, names)
}

/// `resolve`, with what the names are (`"profile"`) in the error.
pub fn resolve_kind(kind: &str, input: &str, names: &[&str]) -> Result<usize, String> {
    let wanted = input.trim().to_lowercase();
    let list = names.join(", ");
    if wanted.is_empty() {
        return Err(format!("give a {kind}: one of: {list}"));
    }
    let lower: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    if let Some(i) = lower.iter().position(|n| *n == wanted) {
        return Ok(i);
    }
    let starts: Vec<usize> = (0..names.len()).filter(|&i| lower[i].starts_with(&wanted)).collect();
    match starts.as_slice() {
        [i] => Ok(*i),
        [] => Err(format!("unknown {kind} \"{}\"; one of: {list}", input.trim())),
        _ => Err(format!("\"{}\" matches more than one: {}", input.trim(), starts.iter().map(|&i| names[i]).collect::<Vec<_>>().join(", "))),
    }
}

/// Writes `value` as one JSON line.
pub fn write_message(w: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()
}

/// Reads one JSON line (or up to the end), refusing more than `MAX_MESSAGE` bytes.
pub fn read_message<T: DeserializeOwned>(r: &mut impl Read) -> io::Result<T> {
    let mut line = Vec::new();
    BufReader::new(r.take(MAX_MESSAGE as u64 + 1)).read_until(b'\n', &mut line)?;
    if line.len() > MAX_MESSAGE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too long"));
    }
    if line.is_empty() {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "no message"));
    }
    serde_json::from_slice(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Cursor, ErrorKind};

    #[test]
    fn envelope_json_is_pinned() {
        let envelope = Envelope { version: 1, request: Request::Mode(Some("turbo".into())) };
        assert_eq!(serde_json::to_string(&envelope).unwrap(), r#"{"version":1,"request":{"mode":"turbo"}}"#);
        assert_eq!(serde_json::to_string(&Request::Status).unwrap(), r#""status""#);
        let lighting = Request::Lighting { device: Device::LightBar, changes: LightingChanges { brightness: Some(40), ..Default::default() } };
        let text = serde_json::to_string(&lighting).unwrap();
        assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), lighting);
    }

    #[test]
    fn pipe_names_are_local_pipes() {
        assert_eq!(pipe_name(false), r"\\.\pipe\UnofficialUniwillControlCenter");
        assert_eq!(pipe_name(true), r"\\.\pipe\UnofficialUniwillControlCenter.Demo");
    }

    #[test]
    fn reply_json_is_pinned() {
        assert_eq!(serde_json::to_string(&Reply::Error { code: 3, message: "x".into() }).unwrap(), r#"{"error":{"code":3,"message":"x"}}"#);
        assert_eq!(serde_json::to_string(&Reply::Ok(json!({"on": true}))).unwrap(), r#"{"ok":{"on":true}}"#);
    }

    #[test]
    fn every_request_round_trips() {
        let requests = [
            Request::Status,
            Request::Mode(None),
            Request::Mode(Some("gaming".into())),
            Request::FanBoost(Some(true)),
            Request::Profile(Some("Quiet".into())),
            Request::Lighting {
                device: Device::Keyboard,
                changes: LightingChanges {
                    effect: Some("wave".into()),
                    colors: Some(vec!["#ff0000".into()]),
                    brightness: Some(40),
                    speed: Some(3),
                    on: Some(true),
                    linked: None,
                },
            },
            Request::Effects,
            Request::Charge(Some("long-life".into())),
            Request::Setup(None),
            Request::Display { on: Some(true), mode: Some("Cinema".into()) },
            Request::GpuMode(Some("intel".into())),
        ];
        for request in requests {
            let text = serde_json::to_string(&request).unwrap();
            assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), request, "{text}");
        }
    }

    #[test]
    fn resolve_matches() {
        assert_eq!(resolve("TUR", &["Office", "Gaming", "Turbo"]), Ok(2));
        assert_eq!(resolve("low", &["Vibrant", "Cinema", "Low blue light", "Custom"]), Ok(2));
        assert_eq!(resolve("gece modu – sessiz", &["Gece Modu – Sessiz"]), Ok(0));
        assert_eq!(resolve("  cinema ", &["Vibrant", "Cinema"]), Ok(1));
        let _ = resolve("İ", &["i"]); // doesn't panic
    }

    #[test]
    fn resolve_errors() {
        assert!(resolve("", &["a"]).is_err());
        assert!(resolve("c", &["Cinema", "Custom"]).unwrap_err().contains("Cinema, Custom"));
        assert!(resolve("x", &["a", "b"]).unwrap_err().contains("one of: a, b"));
        assert_eq!(resolve("Quiet", &["Quiet", "Quieter"]), Ok(0));
        assert!(resolve_kind("profile", "x", &["Standard"]).unwrap_err().contains("unknown profile \"x\""));
    }

    #[test]
    fn messages_round_trip_and_are_capped() {
        let mut buffer = Vec::new();
        write_message(&mut buffer, &Reply::Ok(json!({"mode": "Turbo"}))).unwrap();
        assert_eq!(buffer.last(), Some(&b'\n'));
        let reply: Reply = read_message(&mut Cursor::new(buffer)).unwrap();
        assert_eq!(reply, Reply::Ok(json!({"mode": "Turbo"})));
        let long = format!("\"{}\"\n", "a".repeat(70_000));
        let error = read_message::<String>(&mut Cursor::new(long.into_bytes())).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidData);
    }
}
