//! Unfinished downloads in the Downloads folder (the `downloads-folder` permission): browsers write a download to a
//! file with its own extension until it's done.

use std::path::PathBuf;
use std::time::SystemTime;

use rhai::{Dynamic, Map};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Downloads, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

/// Chrome and Edge, Firefox, old Edge, Safari, Opera.
const PARTIAL: &[&str] = &["crdownload", "part", "partial", "download", "opdownload"];

fn folder() -> Option<PathBuf> {
    unsafe {
        if let Ok(path) = SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None) {
            let text = path.to_string().ok();
            CoTaskMemFree(Some(path.0 as _));
            if let Some(text) = text {
                return Some(PathBuf::from(text));
            }
        }
    }
    std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("Downloads"))
}

/// Each unfinished download: `name` (without the partial extension), `size` in bytes and `idle`, the seconds since
/// it last grew.
pub fn partial() -> Vec<Map> {
    let Some(entries) = folder().and_then(|f| std::fs::read_dir(f).ok()) else { return Vec::new() };
    let now = SystemTime::now();
    let mut found = Vec::new();
    for entry in entries.flatten().take(5000) {
        let path = entry.path();
        let Some(extension) = path.extension().and_then(|e| e.to_str()) else { continue };
        if !PARTIAL.iter().any(|p| extension.eq_ignore_ascii_case(p)) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else { continue };
        if !metadata.is_file() {
            continue;
        }
        let idle = metadata.modified().ok().and_then(|m| now.duration_since(m).ok()).map_or(0.0, |d| d.as_secs_f64());
        let mut item = Map::new();
        item.insert("name".into(), Dynamic::from(path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()));
        item.insert("size".into(), Dynamic::from(metadata.len() as i64));
        item.insert("idle".into(), Dynamic::from(idle));
        found.push(item);
    }
    found
}
