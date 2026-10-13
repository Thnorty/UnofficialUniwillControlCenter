//! Where plugins live (`%APPDATA%\TulparControl\plugins\<id>`), the plugins that come with the app, and importing
//! one from a .zip file or a folder's plugin.json.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::manifest::Manifest;

/// A plugin as found on disk.
#[derive(Clone, Debug)]
pub struct Installed {
    pub manifest: Manifest,
    pub dir: PathBuf,
    pub script: String,
    pub builtin: bool,
}

/// The plugins that come with the app: their id and files, from the repository's plugins folder.
const BUILTIN: &[(&str, &[(&str, &[u8])])] = &[
    (
        "claude-code",
        &[
            ("plugin.json", include_bytes!("../../plugins/claude-code/plugin.json")),
            ("main.rhai", include_bytes!("../../plugins/claude-code/main.rhai")),
        ],
    ),
    (
        "downloads",
        &[
            ("plugin.json", include_bytes!("../../plugins/downloads/plugin.json")),
            ("main.rhai", include_bytes!("../../plugins/downloads/main.rhai")),
            ("extension/manifest.json", include_bytes!("../../plugins/downloads/extension/manifest.json")),
            ("extension/background.js", include_bytes!("../../plugins/downloads/extension/background.js")),
        ],
    ),
    (
        "example",
        &[
            ("plugin.json", include_bytes!("../../plugins/example/plugin.json")),
            ("main.rhai", include_bytes!("../../plugins/example/main.rhai")),
        ],
    ),
    (
        "focus-timer",
        &[
            ("plugin.json", include_bytes!("../../plugins/focus-timer/plugin.json")),
            ("main.rhai", include_bytes!("../../plugins/focus-timer/main.rhai")),
        ],
    ),
    (
        "media",
        &[
            ("plugin.json", include_bytes!("../../plugins/media/plugin.json")),
            ("main.rhai", include_bytes!("../../plugins/media/main.rhai")),
        ],
    ),
    (
        "notifications",
        &[
            ("plugin.json", include_bytes!("../../plugins/notifications/plugin.json")),
            ("main.rhai", include_bytes!("../../plugins/notifications/main.rhai")),
        ],
    ),
    (
        "temperature-alert",
        &[
            ("plugin.json", include_bytes!("../../plugins/temperature-alert/plugin.json")),
            ("main.rhai", include_bytes!("../../plugins/temperature-alert/main.rhai")),
        ],
    ),
];

/// Plugins that come with the app but start off: they'd change the light bar unasked, or need choices first.
const OFF_BY_DEFAULT: &[&str] = &["example", "media", "notifications", "temperature-alert"];

const MAX_FILES: usize = 500;
const MAX_BYTES: u64 = 20 << 20;
const MAX_MANIFEST: u64 = 256 << 10;

pub fn is_builtin(id: &str) -> bool {
    BUILTIN.iter().any(|(builtin, _)| *builtin == id)
}

/// Whether a plugin is on before it's ever turned on or off.
pub fn on_by_default(id: &str) -> bool {
    is_builtin(id) && !OFF_BY_DEFAULT.contains(&id)
}

/// The plugins folder. Demo mode uses one in the temp folder, so it leaves the real one alone.
pub fn plugins_dir(demo: bool) -> Option<PathBuf> {
    if demo {
        return Some(std::env::temp_dir().join("TulparControl-demo").join("plugins"));
    }
    std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join("TulparControl").join("plugins"))
}

/// Writes the plugins that come with the app where they're missing or a different version.
pub fn install_builtins(root: &Path) -> Result<()> {
    for (id, files) in BUILTIN {
        let dir = root.join(id);
        let bundled = Manifest::parse(std::str::from_utf8(files[0].1)?)?;
        let current = fs::read_to_string(dir.join("plugin.json")).ok().and_then(|text| Manifest::parse(&text).ok());
        if current.is_some_and(|m| m.version == bundled.version) {
            continue;
        }
        for (path, bytes) in *files {
            let target = dir.join(path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&target, bytes).with_context(|| format!("Couldn't write {}", target.display()))?;
        }
    }
    Ok(())
}

/// Every plugin in the folder, and why the ones that couldn't be read couldn't.
pub fn load_all(root: &Path) -> (Vec<Installed>, Vec<String>) {
    let (mut plugins, mut errors) = (Vec::new(), Vec::new());
    let Ok(entries) = fs::read_dir(root) else { return (plugins, errors) };
    for entry in entries.flatten() {
        let dir = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !dir.is_dir() || name.starts_with('.') {
            continue;
        }
        match load(&dir) {
            Ok(plugin) if plugin.manifest.id == name => plugins.push(plugin),
            Ok(plugin) => errors.push(format!("{name}: its plugin.json says it's \"{}\"", plugin.manifest.id)),
            Err(e) => errors.push(format!("{name}: {e:#}")),
        }
    }
    plugins.sort_by_key(|p| (!p.builtin, p.manifest.name.to_lowercase()));
    (plugins, errors)
}

fn load(dir: &Path) -> Result<Installed> {
    let manifest = Manifest::parse(&fs::read_to_string(dir.join("plugin.json")).context("No plugin.json")?)?;
    let script = fs::read_to_string(dir.join(&manifest.script)).with_context(|| format!("No {}", manifest.script))?;
    let builtin = is_builtin(&manifest.id);
    Ok(Installed { manifest, dir: dir.to_path_buf(), script, builtin })
}

/// Installs a plugin from a .zip file, or from the folder of a plugin.json, replacing an older copy. Returns its
/// manifest.
pub fn import(root: &Path, path: &Path) -> Result<Manifest> {
    let is_manifest = path.file_name().is_some_and(|n| n.eq_ignore_ascii_case("plugin.json"));
    let staging = root.join(".import");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    let result = if is_manifest { copy_folder(path.parent().context("No folder")?, &staging) } else { unzip(path, &staging) };
    let manifest = result.and_then(|()| load(&staging)).map(|p| p.manifest);
    let manifest = match manifest {
        Ok(m) if is_builtin(&m.id) => Err(anyhow::anyhow!("\"{}\" is the id of a plugin that comes with the app", m.id)),
        other => other,
    };
    let manifest = match manifest {
        Ok(m) => m,
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
    };
    let target = root.join(&manifest.id);
    if target.exists() {
        fs::remove_dir_all(&target).context("Couldn't replace the installed copy")?;
    }
    fs::rename(&staging, &target)?;
    Ok(manifest)
}

/// Unzips the plugin into `to`. The plugin.json may be at the top or in one folder.
fn unzip(path: &Path, to: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(fs::File::open(path)?).context("Not a .zip file")?;
    if archive.len() > MAX_FILES {
        bail!("The .zip file has too many files");
    }
    let names: Vec<String> = archive.file_names().map(|n| n.map(|n| n.into_owned())).collect::<Result<_, _>>().context("The .zip file is damaged")?;
    let prefix = names
        .iter()
        .filter(|n| *n == "plugin.json" || (n.ends_with("/plugin.json") && n.matches('/').count() == 1))
        .min_by_key(|n| n.len())
        .map(|n| n.trim_end_matches("plugin.json").to_string())
        .context("The .zip file has no plugin.json")?;
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let Some(relative) = file.enclosed_name() else { bail!("The .zip file has a file outside its folder") };
        let Ok(relative) = relative.strip_prefix(&prefix) else { continue };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = to.join(relative);
        if file.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        total += file.size();
        if total > MAX_BYTES || (relative == Path::new("plugin.json") && file.size() > MAX_MANIFEST) {
            bail!("The plugin is too big");
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut bytes = Vec::new();
        (&mut file).take(MAX_BYTES).read_to_end(&mut bytes)?;
        fs::write(&target, bytes)?;
    }
    Ok(())
}

fn copy_folder(from: &Path, to: &Path) -> Result<()> {
    let mut count = 0usize;
    let mut total = 0u64;
    let mut stack = vec![(from.to_path_buf(), to.to_path_buf())];
    while let Some((source, target)) = stack.pop() {
        fs::create_dir_all(&target)?;
        for entry in fs::read_dir(&source)?.flatten() {
            let kind = entry.file_type()?;
            count += 1;
            if count > MAX_FILES {
                bail!("The plugin's folder has too many files");
            }
            if kind.is_dir() {
                stack.push((entry.path(), target.join(entry.file_name())));
            } else if kind.is_file() {
                total += entry.metadata()?.len();
                if total > MAX_BYTES {
                    bail!("The plugin is too big");
                }
                fs::copy(entry.path(), target.join(entry.file_name()))?;
            }
        }
    }
    Ok(())
}

/// Uninstalls a plugin that didn't come with the app.
pub fn remove(root: &Path, id: &str) -> Result<()> {
    if is_builtin(id) {
        bail!("Plugins that come with the app can be turned off but not removed");
    }
    fs::remove_dir_all(root.join(id)).context("Couldn't remove the plugin's folder")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_parse() {
        for (id, files) in BUILTIN {
            let manifest = Manifest::parse(std::str::from_utf8(files[0].1).unwrap()).unwrap();
            assert_eq!(&manifest.id, id);
            assert!(files.iter().any(|(path, _)| *path == manifest.script));
        }
    }
}
