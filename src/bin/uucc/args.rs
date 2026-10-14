//! The command line, turned into a request.

use crate::protocol::{Device, LightingChanges, Request, resolve_kind};

/// What to do: send `request`, or print help (`help`, the command it's about or empty) or the version.
#[derive(Debug, Default)]
pub struct Command {
    pub request: Option<Request>,
    pub json: bool,
    pub start: bool,
    pub help: Option<String>,
    pub version: bool,
}

/// A command line that doesn't make sense, and why (exit code 1).
#[derive(Debug)]
pub struct Usage(pub String);

pub const SPEEDS: [&str; 5] = ["slowest", "slow", "medium", "fast", "fastest"];
pub const MODES: [&str; 3] = ["office", "gaming", "turbo"];
pub const CHARGES: [&str; 3] = ["full", "balanced", "long-life"];
pub const GPU_MODES: [&str; 3] = ["hybrid", "intel", "nvidia"];

const COMMANDS: [&str; 11] = ["status", "mode", "fan-boost", "profile", "keyboard", "lightbar", "effects", "charge", "setup", "display", "gpu-mode"];
/// Options that take a value, and the commands that have them.
const VALUED: [(&str, &[&str]); 6] = [
    ("effect", &["keyboard", "lightbar"]),
    ("brightness", &["keyboard", "lightbar"]),
    ("speed", &["keyboard", "lightbar"]),
    ("colors", &["keyboard", "lightbar"]),
    ("linked", &["lightbar"]),
    ("mode", &["display"]),
];

pub const USAGE: &str = "\
Unofficial Uniwill Control Center, from a terminal. The app must be running (or use --start).

  uucc status                         Mode, temperatures, fans, battery, lighting and display
  uucc mode [office|gaming|turbo]     The performance mode
  uucc fan-boost [on|off]             Fan Boost
  uucc profile [NAME]                 The mode's profiles, or switch to one
  uucc keyboard [on|off] [OPTIONS]    The keyboard's lighting
  uucc lightbar [on|off] [OPTIONS]    The light bar's lighting
  uucc effects                        The effects each device has
  uucc charge [full|balanced|long-life]   The battery charge mode
  uucc setup [NAME]                   The saved setups, or apply one
  uucc display [on|off] [--mode NAME] The display colors
  uucc gpu-mode [hybrid|intel|nvidia] The GPU mode (takes effect when the laptop restarts)

Without a value, a command shows the current setting. Names can be shortened: uucc mode t
Lighting options: --effect NAME, --brightness 0-100, --speed slowest|slow|medium|fast|fastest (or 1-5),
  --colors HEX,HEX..., and for the light bar --linked on|off (same as the keyboard).
Every command: --json (output for scripts), --start (start the app if it isn't running), --help.
Exit codes: 0 done, 1 bad command or value, 2 the app isn't running, 3 the app couldn't do it.";

/// More about one command, for `uucc help COMMAND`.
pub fn help(command: &str) -> &'static str {
    match command {
        "status" => "uucc status [--json]\n  Everything at once: mode, Fan Boost, profile, CPU and GPU temperature and load, fans, battery,\n  charge mode, lighting, display and GPU mode. Readings are taken fresh, which takes a second or two.",
        "mode" => "uucc mode [office|gaming|turbo]\n  Shows or sets the performance mode. Turbo only where the laptop has it.",
        "fan-boost" => "uucc fan-boost [on|off]\n  Shows or sets Fan Boost (the fans at full speed).",
        "profile" => "uucc profile [NAME]\n  Lists the current mode's profiles, or switches to one.",
        "keyboard" => "uucc keyboard [on|off] [--effect NAME] [--brightness 0-100] [--speed SPEED] [--colors HEX,...]\n  Shows or changes the keyboard's lighting. Off is brightness 0; on brings back the brightness from before.\n  --colors sets the effect's colors: 1 or 4 zone colors for Static, the palette for effects that have one.\n  uucc effects lists the effects and how many colors each takes.",
        "lightbar" => "uucc lightbar [on|off] [--effect NAME] [--brightness 0-100] [--speed SPEED] [--colors HEX,...] [--linked on|off]\n  Shows or changes the light bar's lighting. --linked on makes it follow the keyboard.",
        "effects" => "uucc effects\n  Lists each device's effects: ID, name, and how many colors each takes.",
        "charge" => "uucc charge [full|balanced|long-life]\n  Shows or sets the battery charge mode: full charges to 100%, long-life keeps it around 50-60%.",
        "setup" => "uucc setup [NAME]\n  Lists the setups saved on the Setups page, or applies one.",
        "display" => "uucc display [on|off] [--mode Vibrant|Cinema|\"Low blue light\"|Custom]\n  Shows or changes the display colors. --mode turns them on.",
        "gpu-mode" => "uucc gpu-mode [hybrid|intel|nvidia]\n  Shows or sets the GPU mode (MUX switch). It takes effect when the laptop restarts.",
        _ => USAGE,
    }
}

pub fn parse(args: &[String]) -> Result<Command, Usage> {
    let mut command = Command::default();
    let mut words: Vec<&str> = Vec::new();
    let mut options: Vec<(&str, &str)> = Vec::new();
    let mut help = false;
    let mut colors_list: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        i += 1;
        let Some(option) = arg.strip_prefix("--") else {
            words.push(arg);
            continue;
        };
        let (name, inline) = match option.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (option, None),
        };
        match name {
            "json" => command.json = true,
            "start" => command.start = true,
            "help" => help = true,
            "version" => command.version = true,
            _ if VALUED.iter().any(|(n, _)| *n == name) => {
                let value = match inline {
                    Some(value) => value,
                    None if i < args.len() && !args[i].starts_with("--") => {
                        i += 1;
                        args[i - 1].as_str()
                    }
                    None => return Err(Usage(format!("--{name} needs a value"))),
                };
                if name == "colors" {
                    // PowerShell passes an unquoted list (ff0000,00ff00) as separate arguments.
                    let mut list = value.trim_matches(',').to_string();
                    while i < args.len() && looks_like_color(&args[i]) {
                        list = format!("{list},{}", args[i].trim_matches(','));
                        i += 1;
                    }
                    colors_list = Some(list);
                    options.push((name, ""));
                    continue;
                }
                options.push((name, value));
            }
            _ => return Err(Usage(format!("unknown option --{name}\n\n{USAGE}"))),
        }
    }
    if command.version {
        return Ok(command);
    }
    let (name, values) = match words.split_first() {
        Some((&"help", rest)) => {
            command.help = Some(rest.first().map(|s| s.to_string()).unwrap_or_default());
            return Ok(command);
        }
        Some((name, rest)) => (*name, rest),
        None if help => {
            command.help = Some(String::new());
            return Ok(command);
        }
        None => return Err(Usage(USAGE.into())),
    };
    if !COMMANDS.contains(&name) {
        return Err(Usage(format!("unknown command \"{name}\"\n\n{USAGE}")));
    }
    if help {
        command.help = Some(name.into());
        return Ok(command);
    }
    for (option, _) in &options {
        if !VALUED.iter().any(|(n, commands)| n == option && commands.contains(&name)) {
            return Err(Usage(format!("{name} has no --{option} option\n\n{}", self::help(name))));
        }
    }
    let takes_value = !matches!(name, "status" | "effects");
    if values.len() > usize::from(takes_value) {
        return Err(Usage(format!("too many values for {name}\n\n{}", self::help(name))));
    }
    let value = values.first().copied();
    let option = |wanted: &str| options.iter().rev().find(|(n, _)| *n == wanted).map(|(_, v)| *v);
    let word = |kind: &str, names: &[&str], v: &str| resolve_kind(kind, v, names).map(|i| names[i].to_string()).map_err(Usage);

    command.request = Some(match name {
        "status" => Request::Status,
        "effects" => Request::Effects,
        "mode" => Request::Mode(value.map(|v| word("mode", &MODES, v)).transpose()?),
        "fan-boost" => Request::FanBoost(value.map(switch).transpose()?),
        "profile" => Request::Profile(value.map(String::from)),
        "setup" => Request::Setup(value.map(String::from)),
        "charge" => Request::Charge(value.map(|v| word("charge mode", &CHARGES, v)).transpose()?),
        "gpu-mode" => Request::GpuMode(value.map(|v| word("GPU mode", &GPU_MODES, v)).transpose()?),
        "display" => Request::Display { on: value.map(switch).transpose()?, mode: option("mode").map(String::from) },
        _ => {
            let device = if name == "keyboard" { Device::Keyboard } else { Device::LightBar };
            Request::Lighting {
                device,
                changes: LightingChanges {
                    effect: option("effect").map(String::from),
                    colors: colors_list.as_deref().map(colors).transpose()?,
                    brightness: option("brightness").map(brightness).transpose()?,
                    speed: option("speed").map(speed).transpose()?,
                    on: value.map(switch).transpose()?,
                    linked: option("linked").map(switch).transpose()?,
                },
            }
        }
    });
    Ok(command)
}

fn switch(value: &str) -> Result<bool, Usage> {
    match value.to_lowercase().as_str() {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(Usage(format!("\"{value}\": say on or off"))),
    }
}

fn brightness(value: &str) -> Result<u8, Usage> {
    value.trim_end_matches('%').parse::<u8>().ok().filter(|b| *b <= 100).ok_or_else(|| Usage(format!("brightness \"{value}\": give 0-100")))
}

fn speed(value: &str) -> Result<u8, Usage> {
    if let Ok(n) = value.parse::<u8>() {
        return if (1..=5).contains(&n) { Ok(n - 1) } else { Err(Usage(format!("speed {n}: give 1-5 or {}", SPEEDS.join(", ")))) };
    }
    resolve_kind("speed", value, &SPEEDS).map(|i| i as u8).map_err(Usage)
}

/// `ff8800`, `#ff8800` or `ff8800,` (one of a list PowerShell split up).
fn looks_like_color(arg: &str) -> bool {
    let hex = arg.trim_matches(',').trim_start_matches('#');
    hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit())
}

fn colors(value: &str) -> Result<Vec<String>, Usage> {
    value
        .split(',')
        .map(|c| {
            let hex = c.trim().trim_start_matches('#');
            if hex.len() == 6 && hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
                Ok(format!("#{}", hex.to_lowercase()))
            } else {
                Err(Usage(format!("color \"{}\": give #rrggbb, such as #ff8800", c.trim())))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Device, LightingChanges, Request};

    fn parse_line(line: &[&str]) -> Result<Command, Usage> {
        parse(&line.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    fn request(line: &[&str]) -> Request {
        parse_line(line).unwrap_or_else(|e| panic!("{line:?}: {}", e.0)).request.expect("a request")
    }

    fn usage(line: &[&str]) -> String {
        match parse_line(line) {
            Ok(_) => panic!("{line:?} parsed"),
            Err(Usage(message)) => message,
        }
    }

    fn keyboard(changes: LightingChanges) -> Request {
        Request::Lighting { device: Device::Keyboard, changes }
    }

    #[test]
    fn modes() {
        assert_eq!(request(&["mode"]), Request::Mode(None));
        assert_eq!(request(&["mode", "t"]), Request::Mode(Some("turbo".into())));
        assert_eq!(request(&["mode", "Gaming"]), Request::Mode(Some("gaming".into())));
        assert!(usage(&["mode", "x"]).contains("office, gaming, turbo"));
        assert!(usage(&["mode", "office", "gaming"]).contains("mode"));
    }

    #[test]
    fn switches() {
        assert_eq!(request(&["fan-boost", "on"]), Request::FanBoost(Some(true)));
        assert_eq!(request(&["fan-boost", "OFF"]), Request::FanBoost(Some(false)));
        assert_eq!(request(&["fan-boost"]), Request::FanBoost(None));
        assert!(usage(&["fan-boost", "maybe"]).contains("on or off"));
    }

    #[test]
    fn lighting_options() {
        assert_eq!(
            request(&["keyboard", "--effect", "wave", "--speed", "fast", "--brightness", "40"]),
            keyboard(LightingChanges { effect: Some("wave".into()), speed: Some(3), brightness: Some(40), ..Default::default() })
        );
        assert_eq!(request(&["keyboard", "--speed", "1"]), keyboard(LightingChanges { speed: Some(0), ..Default::default() }));
        assert_eq!(request(&["keyboard", "--speed=fastest"]), keyboard(LightingChanges { speed: Some(4), ..Default::default() }));
        assert!(usage(&["keyboard", "--speed", "6"]).contains("speed"));
        assert!(usage(&["keyboard", "--brightness", "101"]).contains("0-100"));
        assert_eq!(
            request(&["keyboard", "--colors", "ff0000,#00FF00"]),
            keyboard(LightingChanges { colors: Some(vec!["#ff0000".into(), "#00ff00".into()]), ..Default::default() })
        );
        assert!(usage(&["keyboard", "--colors", "red"]).contains("red"));
        // PowerShell passes an unquoted ff0000,00ff00 as two arguments.
        assert_eq!(
            request(&["keyboard", "--colors", "ff0000", "00ff00", "on"]),
            keyboard(LightingChanges { colors: Some(vec!["#ff0000".into(), "#00ff00".into()]), on: Some(true), ..Default::default() })
        );
        assert!(usage(&["keyboard", "--linked", "on"]).contains("--linked"));
        assert_eq!(
            request(&["lightbar", "off", "--linked=on"]),
            Request::Lighting { device: Device::LightBar, changes: LightingChanges { on: Some(false), linked: Some(true), ..Default::default() } }
        );
        assert_eq!(request(&["keyboard"]), keyboard(LightingChanges::default()));
        assert!(usage(&["keyboard", "--effect"]).contains("--effect"));
        assert!(usage(&["mode", "--effect", "wave"]).contains("--effect"));
    }

    #[test]
    fn values_with_words() {
        assert_eq!(request(&["charge", "long"]), Request::Charge(Some("long-life".into())));
        assert_eq!(request(&["gpu-mode", "i"]), Request::GpuMode(Some("intel".into())));
        assert_eq!(request(&["display", "--mode", "low blue"]), Request::Display { on: None, mode: Some("low blue".into()) });
        assert_eq!(request(&["display", "on"]), Request::Display { on: Some(true), mode: None });
        assert_eq!(request(&["setup", "Gece Modu – Sessiz"]), Request::Setup(Some("Gece Modu – Sessiz".into())));
        assert_eq!(request(&["profile", "Quiet"]), Request::Profile(Some("Quiet".into())));
        assert_eq!(request(&["effects"]), Request::Effects);
        assert_eq!(request(&["status"]), Request::Status);
    }

    #[test]
    fn options_anywhere() {
        let command = parse_line(&["--json", "status"]).unwrap();
        assert!(command.json && command.request == Some(Request::Status));
        let command = parse_line(&["status", "--json", "--start"]).unwrap();
        assert!(command.json && command.start);
    }

    #[test]
    fn help_and_mistakes() {
        assert!(usage(&[]).contains("uucc mode"));
        assert!(usage(&["frobnicate"]).contains("uucc mode"));
        assert!(usage(&["status", "--frob"]).contains("--frob"));
        assert_eq!(parse_line(&["help", "mode"]).unwrap().help, Some("mode".into()));
        assert_eq!(parse_line(&["mode", "--help"]).unwrap().help, Some("mode".into()));
        assert_eq!(parse_line(&["--help"]).unwrap().help, Some(String::new()));
        assert!(parse_line(&["--version"]).unwrap().version);
    }
}
