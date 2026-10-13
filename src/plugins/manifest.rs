//! `plugin.json`: what a plugin is called, the settings the Plugins page shows for it, its buttons and the
//! permissions it needs beyond drawing on the light bar.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::color::Rgb;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// Lowercase letters, digits and dashes; the plugin's folder and its address under /plugins/.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    /// The script, relative to the plugin's folder.
    #[serde(default = "default_script")]
    pub script: String,
    #[serde(default)]
    pub settings: Vec<Setting>,
    /// Buttons on the plugin's card; pressing one calls the script's `on_action(id)`.
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default)]
    pub permissions: Vec<Permission>,
}

fn default_script() -> String {
    "main.rhai".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(flatten)]
    pub kind: SettingKind,
    #[serde(default)]
    pub default: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SettingKind {
    Toggle,
    Slider {
        min: f64,
        max: f64,
        #[serde(default = "one")]
        step: f64,
        #[serde(default)]
        unit: String,
    },
    Color,
    Choice {
        options: Vec<String>,
    },
    Text,
}

fn one() -> f64 {
    1.0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub label: String,
}

/// What a plugin may do besides drawing on the light bar, showing its status and pop-up notifications.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    /// Add and remove its own HTTP hooks in Claude Code's user settings (%USERPROFILE%\.claude\settings.json).
    ClaudeHooks,
    /// See the names and sizes of unfinished downloads in the Downloads folder.
    DownloadsFolder,
}

impl Permission {
    pub fn description(self) -> &'static str {
        match self {
            Permission::ClaudeHooks => "add its hooks to Claude Code's settings",
            Permission::DownloadsFolder => "see unfinished downloads in your Downloads folder",
        }
    }
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Manifest> {
        let manifest: Manifest = serde_json::from_str(text.trim_start_matches('\u{FEFF}'))?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<()> {
        if self.id.is_empty() || self.id.len() > 40 || !self.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            bail!("The plugin's id must be 1-40 lowercase letters, digits or dashes");
        }
        if self.name.trim().is_empty() {
            bail!("The plugin has no name");
        }
        if self.script.contains("..") || self.script.contains(':') || self.script.starts_with(['/', '\\']) {
            bail!("The plugin's script must be inside its folder");
        }
        let mut keys = std::collections::HashSet::new();
        for setting in &self.settings {
            if !keys.insert(&setting.key) {
                bail!("The setting \"{}\" is there twice", setting.key);
            }
            match &setting.kind {
                SettingKind::Slider { min, max, step, .. } if !(min < max && *step > 0.0) => bail!("The slider \"{}\" has no range", setting.key),
                SettingKind::Choice { options } if options.is_empty() => bail!("The choice \"{}\" has no options", setting.key),
                _ => {}
            }
        }
        Ok(())
    }

    /// Each setting's value: the saved one if it's valid, else the default.
    pub fn values(&self, saved: Option<&Map<String, Value>>) -> Map<String, Value> {
        self.settings.iter().map(|s| (s.key.clone(), s.check(saved.and_then(|m| m.get(&s.key))))).collect()
    }
}

impl Setting {
    /// `value` if it suits this setting, else the default (or a sensible one).
    pub fn check(&self, value: Option<&Value>) -> Value {
        let fallback = |v: &Value| self.check_one(v);
        value.and_then(fallback).or_else(|| fallback(&self.default)).unwrap_or_else(|| match &self.kind {
            SettingKind::Toggle => Value::Bool(false),
            SettingKind::Slider { min, .. } => (*min).into(),
            SettingKind::Color => "#FFFFFF".into(),
            SettingKind::Choice { options } => options[0].clone().into(),
            SettingKind::Text => "".into(),
        })
    }

    fn check_one(&self, value: &Value) -> Option<Value> {
        match &self.kind {
            SettingKind::Toggle => value.as_bool().map(Value::Bool),
            SettingKind::Slider { min, max, step, .. } => {
                let v = value.as_f64()?;
                let stepped = min + ((v.clamp(*min, *max) - min) / step).round() * step;
                let v = stepped.clamp(*min, *max);
                Some(if v.fract() == 0.0 && step.fract() == 0.0 { Value::from(v as i64) } else { Value::from(v) })
            }
            SettingKind::Color => Rgb::parse_hex(value.as_str()?).map(|c| c.hex().into()),
            SettingKind::Choice { options } => value.as_str().filter(|v| options.iter().any(|o| o == v)).map(|v| v.into()),
            SettingKind::Text => value.as_str().map(|v| v.chars().take(500).collect::<String>().into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_checks_values() {
        let manifest = Manifest::parse(
            r##"{ "id": "demo", "name": "Demo", "permissions": ["claude-hooks"],
                 "settings": [
                   { "key": "color", "label": "Color", "type": "color", "default": "#ff8800" },
                   { "key": "speed", "label": "Speed", "type": "slider", "min": 0, "max": 10, "default": 4 },
                   { "key": "mode", "label": "Mode", "type": "choice", "options": ["a", "b"], "default": "b" } ] }"##,
        )
        .unwrap();
        assert_eq!(manifest.script, "main.rhai");
        assert_eq!(manifest.permissions, vec![Permission::ClaudeHooks]);
        let saved: Map<String, Value> = serde_json::from_str(r#"{ "speed": 42, "mode": "x" }"#).unwrap();
        let values = manifest.values(Some(&saved));
        assert_eq!(values["color"], "#FF8800");
        assert_eq!(values["speed"], 10);
        assert_eq!(values["mode"], "b");
        assert!(Manifest::parse(r#"{ "id": "Bad Id", "name": "x" }"#).is_err());
    }
}
