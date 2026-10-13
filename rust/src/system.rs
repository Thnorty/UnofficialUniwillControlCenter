//! Small pieces of Windows: the touchpad toggle, the startup task, Control Center detection, the Caps/Num Lock
//! state, and registry reads.

use std::os::windows::process::CommandExt;
use std::process::Command;

use anyhow::{Result, bail};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Security::Authentication::Identity::{EXTENDED_NAME_FORMAT, GetUserNameExW};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, RRF_RT_REG_DWORD, RegCloseKey, RegDeleteValueW, RegGetValueW, RegOpenKeyExW};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CAPITAL,
    VK_CONTROL, VK_F24, VK_LWIN, VK_NUMLOCK,
};
use windows::core::{HSTRING, PWSTR, w};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Reads a DWORD from HKEY_CURRENT_USER.
pub fn read_user_dword(key: &str, value: &str) -> Option<u32> {
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &HSTRING::from(value),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as _),
            Some(&mut size),
        )
    };
    result.is_ok().then_some(data)
}

/// Whether Windows apps use the dark theme.
pub fn dark_mode() -> bool {
    read_user_dword("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize", "AppsUseLightTheme") == Some(0)
}

/// Whether the taskbar and system flyouts use the dark theme (the OSD follows these, like Windows' own).
pub fn system_dark_mode() -> bool {
    read_user_dword("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize", "SystemUsesLightTheme") != Some(1)
}

/// The Windows accent color (DWM's), as RGB.
pub fn accent_color() -> Option<crate::color::Rgb> {
    // AccentColor is ABGR.
    let abgr = read_user_dword("Software\\Microsoft\\Windows\\DWM", "AccentColor")?;
    Some(crate::color::Rgb::new(abgr as u8, (abgr >> 8) as u8, (abgr >> 16) as u8))
}

/// Turns the Precision Touchpad on and off the way Control Center does (GCService5.DeviceSwitchItem.TriggerTouchpad):
/// by pressing Ctrl+Win+F24, which Windows handles as its built-in touchpad toggle.
pub mod touchpad {
    use super::*;

    /// Windows' current touchpad state, or None if it doesn't report one.
    pub fn is_enabled() -> Option<bool> {
        read_user_dword("Software\\Microsoft\\Windows\\CurrentVersion\\PrecisionTouchPad\\Status", "Enabled").map(|v| v != 0)
    }

    pub fn toggle() -> Result<()> {
        let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 },
            },
        };
        let inputs = [
            key(VK_CONTROL, false),
            key(VK_LWIN, false),
            key(VK_F24, false),
            key(VK_F24, true),
            key(VK_LWIN, true),
            key(VK_CONTROL, true),
        ];
        if unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) } as usize != inputs.len() {
            bail!("Couldn't send the touchpad shortcut");
        }
        Ok(())
    }
}

/// Whether Caps Lock and Num Lock are on.
pub fn lock_keys() -> (bool, bool) {
    unsafe { (GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0, GetKeyState(VK_NUMLOCK.0 as i32) & 1 != 0) }
}

/// Control Center's service, which owns the Fn keys and shows its own pop-ups while it runs.
pub fn control_center_running() -> bool {
    process_running("GCUService.exe")
}

fn process_running(name: &str) -> bool {
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return false };
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = false;
        let mut ok = Process32FirstW(snapshot, &mut entry).is_ok();
        while ok {
            let length = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..length]).eq_ignore_ascii_case(name) {
                found = true;
                break;
            }
            ok = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        found
    }
}

/// Starts the app at logon through Task Scheduler with highest privileges. The Run key can't start an app that needs
/// administrator rights without a UAC prompt. The task keeps the earlier C# version's name, so it replaces that one's.
pub mod startup {
    use super::*;

    const TASK_NAME: &str = "MonsterKeyboardRGB";

    pub fn is_enabled() -> bool {
        schtasks(&["/Query", "/TN", TASK_NAME]).is_ok_and(|code| code == 0)
    }

    pub fn enable() -> Result<()> {
        let user = escape(&current_user()?);
        let exe = escape(&std::env::current_exe()?.to_string_lossy());
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Keyboard RGB tray app</Description></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger></Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>--tray</Arguments>
    </Exec>
  </Actions>
</Task>"#
        );
        let file = std::env::temp_dir().join(format!("{TASK_NAME}.task.xml"));
        // UTF-16 with a byte order mark, as the declaration says.
        let mut bytes = vec![0xFF, 0xFE];
        for unit in xml.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(&file, bytes)?;
        let result = schtasks(&["/Create", "/TN", TASK_NAME, "/XML", &file.to_string_lossy(), "/F"]);
        let _ = std::fs::remove_file(&file);
        if result? != 0 {
            bail!("Task Scheduler refused to create the startup task.");
        }
        remove_legacy_run_entry();
        Ok(())
    }

    pub fn disable() -> Result<()> {
        if is_enabled() && schtasks(&["/Delete", "/TN", TASK_NAME, "/F"])? != 0 {
            bail!("Task Scheduler refused to delete the startup task.");
        }
        remove_legacy_run_entry();
        Ok(())
    }

    /// The first versions started from the Run key, which can't launch an elevated app.
    pub fn remove_legacy_run_entry() {
        unsafe {
            let mut key = HKEY::default();
            if RegOpenKeyExW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"), Some(0), KEY_SET_VALUE, &mut key).is_ok() {
                let _ = RegDeleteValueW(key, &HSTRING::from(TASK_NAME));
                let _ = RegCloseKey(key);
            }
        }
    }

    fn schtasks(arguments: &[&str]) -> Result<i32> {
        let output = Command::new("schtasks.exe").args(arguments).creation_flags(CREATE_NO_WINDOW).output()?;
        Ok(output.status.code().unwrap_or(-1))
    }

    fn current_user() -> Result<String> {
        const NAME_SAM_COMPATIBLE: EXTENDED_NAME_FORMAT = EXTENDED_NAME_FORMAT(2); // DOMAIN\user
        let mut buffer = [0u16; 512];
        let mut size = buffer.len() as u32;
        if !unsafe { GetUserNameExW(NAME_SAM_COMPATIBLE, Some(PWSTR(buffer.as_mut_ptr())), &mut size) } {
            bail!("Can't find the current user's name");
        }
        Ok(String::from_utf16_lossy(&buffer[..size as usize]))
    }

    fn escape(text: &str) -> String {
        text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
    }
}
