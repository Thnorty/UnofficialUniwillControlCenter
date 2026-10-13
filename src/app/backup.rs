//! Backing up all settings to a file and restoring them. Restoring saves the file's settings over the current ones
//! as the app exits, and starts it again, so everything is set up from them as at any start.

use crate::settings::Settings;
use crate::shell;

use super::App;

const FILTER: &[(&str, &str)] = &[("Tulpar Control settings (*.json)", "*.json")];

impl App {
    pub(super) fn export_settings(&mut self) {
        self.store_settings();
        let owner = self.window_hwnd();
        let Some(path) = shell::pick_save_file(owner, "Back up settings", FILTER, "Tulpar Control settings.json") else { return };
        let result = self.settings.to_backup().and_then(|text| Ok(std::fs::write(&path, text)?));
        match result {
            Ok(()) => self.set_status(&format!("Settings saved to {}.", path.display()), false, false),
            Err(e) => self.set_status(&format!("Couldn't save the settings: {e:#}"), true, false),
        }
    }

    pub(super) fn import_settings(&mut self) {
        let owner = self.window_hwnd();
        let Some(path) = shell::pick_file(owner, "Restore settings", FILTER) else { return };
        let restored = match std::fs::read_to_string(&path).map_err(anyhow::Error::from).and_then(|text| Settings::from_backup(&text)) {
            Ok(settings) => settings,
            Err(e) => {
                self.set_status(&format!("Couldn't restore the settings: {e:#}"), true, false);
                return;
            }
        };
        if self.demo {
            self.set_status("The backup can be read. Demo mode doesn't change any settings.", false, false);
            return;
        }
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let question = format!("Replace all of Tulpar Control's settings with the ones in \"{name}\"?\n\nThe app restarts to use them.");
        if !shell::confirm(owner, "Restore settings", &question) {
            return;
        }
        self.replace_settings = Some(restored);
        self.exit();
    }
}
