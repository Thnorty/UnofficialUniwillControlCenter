//! Opening files and folders, file pickers and questions on Linux. The pickers are the desktop's own, through the
//! XDG desktop portal; questions and errors are shown with kdialog (KDE) or zenity (GNOME and others), whichever the
//! desktop has.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The window a dialog belongs to.
#[derive(Clone, Copy)]
pub struct Owner;

/// Opens a folder in the file manager (or a file with its program).
pub fn open_path(path: &Path) {
    let _ = Command::new("xdg-open").arg(path).spawn();
}

/// The extensions in a pattern list such as "*.zip;plugin.json" (the portal filters by extension).
fn extensions(patterns: &str) -> Vec<String> {
    patterns.split(';').filter_map(|p| p.rsplit_once('.').map(|(_, extension)| extension.to_string())).collect()
}

fn file_dialog(title: &str, filter: &[(&str, &str)]) -> rfd::FileDialog {
    filter.iter().fold(rfd::FileDialog::new().set_title(title), |dialog, (name, patterns)| dialog.add_filter(*name, &extensions(patterns)))
}

/// Asks for a file. `filter` is pairs of a description and patterns, such as ("Plugins", "*.zip;plugin.json").
pub fn pick_file(_owner: Option<Owner>, title: &str, filter: &[(&str, &str)]) -> Option<PathBuf> {
    file_dialog(title, filter).pick_file()
}

/// Asks where to save a file, suggesting `name`; the desktop's dialog asks before replacing one.
pub fn pick_save_file(_owner: Option<Owner>, title: &str, filter: &[(&str, &str)], name: &str) -> Option<PathBuf> {
    file_dialog(title, filter).set_file_name(name).save_file()
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Error,
    Question,
    Warning,
}

/// The dialog programs to try, the desktop's own first.
fn programs() -> [&'static str; 2] {
    let kde = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_ascii_uppercase().contains("KDE"));
    if kde { ["kdialog", "zenity"] } else { ["zenity", "kdialog"] }
}

/// Shows a dialog; Some(true) for OK, Some(false) for Cancel, None when there's no program to show it with.
fn show(kind: Kind, title: &str, text: &str) -> Option<bool> {
    for program in programs() {
        let mut command = Command::new(program);
        if program == "kdialog" {
            command.args(["--title", title]);
            match kind {
                Kind::Error => command.args(["--error", text]),
                Kind::Question => command.args(["--yesno", text, "--yes-label", "OK", "--no-label", "Cancel"]),
                Kind::Warning => command.args(["--warningcontinuecancel", text, "--continue-label", "OK"]),
            };
        } else {
            command.args(["--title", title, "--no-markup", "--text", text]);
            match kind {
                Kind::Error => command.arg("--error"),
                Kind::Question => command.args(["--question", "--ok-label", "OK", "--cancel-label", "Cancel"]),
                Kind::Warning => command.args(["--question", "--icon=dialog-warning", "--ok-label", "OK", "--cancel-label", "Cancel"]),
            };
        }
        if let Ok(status) = command.status() {
            return Some(status.success());
        }
    }
    None
}

const NO_DIALOGS: &str = "(No dialog could be shown: install kdialog or zenity.)";

/// Shows an error with an OK button.
pub fn error_box(title: &str, text: &str) {
    eprintln!("{title}: {text}");
    if show(Kind::Error, title, text).is_none() {
        eprintln!("{NO_DIALOGS}");
    }
}

/// Asks an OK/Cancel question with a warning icon; true for OK. Without a dialog program, the answer is Cancel.
pub fn warning_ok_cancel(title: &str, text: &str, _default_cancel: bool) -> bool {
    show(Kind::Warning, title, text).unwrap_or_else(|| {
        eprintln!("{title}: {text}\n{NO_DIALOGS}");
        false
    })
}

/// Asks an OK/Cancel question; true for OK.
pub fn confirm(_owner: Option<Owner>, title: &str, text: &str) -> bool {
    show(Kind::Question, title, text).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    #[test]
    fn filter_patterns_become_extensions() {
        assert_eq!(super::extensions("*.zip;plugin.json"), ["zip", "json"]);
        assert_eq!(super::extensions("*.json"), ["json"]);
    }
}
