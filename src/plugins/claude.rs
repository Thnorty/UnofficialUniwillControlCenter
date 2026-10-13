//! Claude Code's HTTP hooks for a plugin (the `claude-hooks` permission): entries in the user's Claude Code settings
//! that POST each hook event to the plugin's address. Only entries with that address are added or removed; the
//! rest of the file is kept as it is, in its order, and a copy is kept the first time.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

/// Events that take HTTP hooks (SessionStart doesn't).
pub const EVENTS: &[&str] =
    &["UserPromptSubmit", "PreToolUse", "PostToolUse", "PostToolUseFailure", "Notification", "Stop", "SubagentStop", "SessionEnd", "PreCompact"];

/// Events whose hooks have a matcher; "*" matches everything.
const WITH_MATCHER: &[&str] = &["PreToolUse", "PostToolUse", "PostToolUseFailure", "Notification", "PreCompact", "SessionEnd"];

fn settings_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("settings.json"));
    }
    std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".claude").join("settings.json"))
}

fn read() -> Result<(PathBuf, Map<String, Value>)> {
    let path = settings_path().context("Couldn't find the user folder")?;
    let settings = match fs::read_to_string(&path) {
        Ok(text) if text.trim().is_empty() => Map::new(),
        Ok(text) => match serde_json::from_str::<Value>(text.trim_start_matches('\u{FEFF}')) {
            Ok(Value::Object(map)) => map,
            _ => bail!("Claude Code's settings.json isn't valid JSON, so it wasn't changed"),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(e) => return Err(e).context("Couldn't read Claude Code's settings.json"),
    };
    Ok((path, settings))
}

fn write(path: &PathBuf, settings: &Map<String, Value>) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let backup = path.with_extension("json.tulpar-backup");
    if path.exists() && !backup.exists() {
        fs::copy(path, &backup).context("Couldn't keep a copy of Claude Code's settings.json")?;
    }
    let temp = path.with_extension("json.tulpar-new");
    fs::write(&temp, serde_json::to_string_pretty(settings)? + "\n")?;
    fs::rename(&temp, path).context("Couldn't write Claude Code's settings.json")
}

fn is_ours(hook: &Value, url: &str) -> bool {
    hook.get("type").and_then(Value::as_str) == Some("http") && hook.get("url").and_then(Value::as_str) == Some(url)
}

/// Adds a hook posting to `url` for each of `events` that doesn't have one yet. Returns how many were added.
pub fn install(url: &str, events: &[String]) -> Result<usize> {
    let (path, mut settings) = read()?;
    let added = add_hooks(&mut settings, url, events)?;
    if added > 0 {
        write(&path, &settings)?;
    }
    Ok(added)
}

/// Removes every hook posting to `url`. Returns how many were removed.
pub fn remove(url: &str) -> Result<usize> {
    let (path, mut settings) = read()?;
    let removed = remove_hooks(&mut settings, url);
    if removed > 0 {
        write(&path, &settings)?;
    }
    Ok(removed)
}

fn add_hooks(settings: &mut Map<String, Value>, url: &str, events: &[String]) -> Result<usize> {
    if let Some(event) = events.iter().find(|e| !EVENTS.contains(&e.as_str())) {
        bail!("Claude Code can't send {event} to an HTTP hook");
    }
    let hooks = settings.entry("hooks").or_insert_with(|| json!({}));
    let Some(hooks) = hooks.as_object_mut() else { bail!("The \"hooks\" in Claude Code's settings.json aren't a list of events") };
    let mut added = 0;
    for event in events {
        let groups = hooks.entry(event.clone()).or_insert_with(|| json!([]));
        let Some(groups) = groups.as_array_mut() else { continue };
        let present = groups.iter().any(|g| g.get("hooks").and_then(Value::as_array).is_some_and(|h| h.iter().any(|h| is_ours(h, url))));
        if present {
            continue;
        }
        let mut group = json!({ "hooks": [ { "type": "http", "url": url, "timeout": 2 } ] });
        if WITH_MATCHER.contains(&event.as_str()) {
            group["matcher"] = json!("*");
        }
        groups.push(group);
        added += 1;
    }
    Ok(added)
}

fn remove_hooks(settings: &mut Map<String, Value>, url: &str) -> usize {
    let mut removed = 0;
    if let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) {
        for groups in hooks.values_mut() {
            let Some(groups) = groups.as_array_mut() else { continue };
            for group in groups.iter_mut() {
                if let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                    let before = list.len();
                    list.retain(|h| !is_ours(h, url));
                    removed += before - list.len();
                }
            }
            groups.retain(|g| g.get("hooks").and_then(Value::as_array).is_none_or(|h| !h.is_empty()));
        }
        hooks.retain(|_, groups| groups.as_array().is_none_or(|g| !g.is_empty()));
        if hooks.is_empty() {
            settings.remove("hooks");
        }
    }
    removed
}

/// How many events have a hook posting to `url`.
pub fn installed(url: &str) -> usize {
    let Ok((_, settings)) = read() else { return 0 };
    let Some(hooks) = settings.get("hooks").and_then(Value::as_object) else { return 0 };
    hooks
        .values()
        .filter(|groups| {
            groups.as_array().is_some_and(|g| g.iter().any(|g| g.get("hooks").and_then(Value::as_array).is_some_and(|h| h.iter().any(|h| is_ours(h, url)))))
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "http://127.0.0.1:47823/plugins/claude-code";

    #[test]
    fn adds_and_removes_only_its_own() {
        let text = r#"{ "model": "opus", "hooks": { "Stop": [ { "hooks": [ { "type": "command", "command": "beep" } ] } ] }, "zeta": 1 }"#;
        let Value::Object(mut settings) = serde_json::from_str(text).unwrap() else { panic!() };
        let events = vec!["Stop".to_string(), "PreToolUse".to_string()];
        assert_eq!(add_hooks(&mut settings, URL, &events).unwrap(), 2);
        assert_eq!(add_hooks(&mut settings, URL, &events).unwrap(), 0); // already there
        assert_eq!(settings["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(settings["hooks"]["PreToolUse"][0]["matcher"], "*");
        assert!(add_hooks(&mut settings, URL, &["SessionStart".to_string()]).is_err());
        assert_eq!(remove_hooks(&mut settings, URL), 2);
        let after = serde_json::to_string(&settings).unwrap();
        // Back as it was, keys in their order.
        assert_eq!(after, r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"beep"}]}]},"zeta":1}"#);
    }
}
