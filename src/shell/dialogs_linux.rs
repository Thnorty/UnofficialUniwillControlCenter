//! Opening files and folders, file pickers and questions on Linux. The pickers and questions aren't shown yet.

use std::path::{Path, PathBuf};

/// The window a dialog belongs to.
#[derive(Clone, Copy)]
pub struct Owner;

/// Opens a folder in the file manager (or a file with its program).
pub fn open_path(path: &Path) {
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

/// Asks for a file. `filter` is pairs of a description and patterns, such as ("Plugins", "*.zip;plugin.json").
pub fn pick_file(_owner: Option<Owner>, _title: &str, _filter: &[(&str, &str)]) -> Option<PathBuf> {
    None
}

/// Asks where to save a file, suggesting `name`; asks before replacing one.
pub fn pick_save_file(_owner: Option<Owner>, _title: &str, _filter: &[(&str, &str)], _name: &str) -> Option<PathBuf> {
    None
}

/// Shows an error with an OK button.
pub fn error_box(title: &str, text: &str) {
    eprintln!("{title}: {text}");
}

/// Asks an OK/Cancel question with a warning icon; true for OK. No dialog yet, so the answer is Cancel.
pub fn warning_ok_cancel(title: &str, text: &str, _default_cancel: bool) -> bool {
    eprintln!("{title}: {text}\n(No dialog on Linux yet: not started.)");
    false
}

/// Asks an OK/Cancel question; true for OK.
pub fn confirm(_owner: Option<Owner>, _title: &str, _text: &str) -> bool {
    false
}
