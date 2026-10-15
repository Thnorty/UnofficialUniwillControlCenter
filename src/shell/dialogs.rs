//! Opening files and folders, file pickers and questions, with Windows' own dialogs.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{IDOK, MB_ICONQUESTION, MB_OKCANCEL, MessageBoxW, SW_SHOWNORMAL};
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

/// Opens a folder in File Explorer (or a file with its program).
pub fn open_path(path: &Path) {
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(path.as_os_str()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Asks for a file. `filter` is pairs of a description and patterns, such as ("Plugins", "*.zip;plugin.json").
pub fn pick_file(owner: Option<HWND>, title: &str, filter: &[(&str, &str)]) -> Option<PathBuf> {
    file_dialog(owner, title, filter, "", false)
}

/// Asks where to save a file, suggesting `name`; asks before replacing one.
pub fn pick_save_file(owner: Option<HWND>, title: &str, filter: &[(&str, &str)], name: &str) -> Option<PathBuf> {
    file_dialog(owner, title, filter, name, true)
}

fn file_dialog(owner: Option<HWND>, title: &str, filter: &[(&str, &str)], name: &str, save: bool) -> Option<PathBuf> {
    let mut filter_text: Vec<u16> = Vec::new();
    for (name, patterns) in filter {
        filter_text.extend(name.encode_utf16());
        filter_text.push(0);
        filter_text.extend(patterns.encode_utf16());
        filter_text.push(0);
    }
    filter_text.push(0);
    let title = HSTRING::from(title);
    let mut file = vec![0u16; 1024];
    for (slot, c) in file.iter_mut().zip(name.encode_utf16().take(1000)) {
        *slot = c;
    }
    let extension = HSTRING::from("json");
    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner.unwrap_or_default(),
        lpstrFilter: PCWSTR(filter_text.as_ptr()),
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        lpstrDefExt: if save { PCWSTR(extension.as_ptr()) } else { PCWSTR::null() },
        Flags: if save { OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR } else { OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR },
        ..Default::default()
    };
    let chosen = unsafe { if save { GetSaveFileNameW(&mut dialog) } else { GetOpenFileNameW(&mut dialog) } };
    if !chosen.as_bool() {
        return None;
    }
    let length = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    Some(PathBuf::from(String::from_utf16_lossy(&file[..length])))
}

/// Asks an OK/Cancel question; true for OK.
pub fn confirm(owner: Option<HWND>, title: &str, text: &str) -> bool {
    unsafe { MessageBoxW(owner, &HSTRING::from(text), &HSTRING::from(title), MB_OKCANCEL | MB_ICONQUESTION) == IDOK }
}

/// Shows an error with an OK button.
pub fn error_box(title: &str, text: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK};
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(title), MB_OK | MB_ICONERROR);
    }
}

/// Asks an OK/Cancel question with a warning icon, with Cancel as the default if `default_cancel`; true for OK.
pub fn warning_ok_cancel(title: &str, text: &str, default_cancel: bool) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{MB_DEFBUTTON2, MB_ICONWARNING};
    let style = MB_OKCANCEL | MB_ICONWARNING | if default_cancel { MB_DEFBUTTON2 } else { Default::default() };
    unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(title), style) == IDOK }
}

/// The window a dialog belongs to.
pub type Owner = HWND;
