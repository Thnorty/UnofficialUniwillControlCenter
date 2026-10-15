//! Saved setups and app profiles. A setup is the lighting of both devices with the performance mode and Fan Boost,
//! saved under a name and applied in one go. An app profile applies a setup while a program runs; when the program
//! closes, what was on before it started comes back.

use std::rc::Rc;

use slint::{ModelRc, SharedString, VecModel};

use super::{App, Kind, Lighting};
use crate::hardware::system;
use crate::lighting::effects;
use crate::settings::{AppProfile, LightingSettings, Setup};
use crate::shell::{self, osd};
use crate::ui::{AppProfileView, SetupView};

#[derive(Default)]
pub(super) struct Setups {
    pub model: Rc<VecModel<SetupView>>,
    pub names: Rc<VecModel<SharedString>>,
    pub profiles: Rc<VecModel<AppProfileView>>,
    /// The setup applied or saved last, marked in the list.
    active: Option<String>,
    /// The program whose profile is in effect.
    profile: Option<String>,
    /// What was on before that program started.
    before: Option<Setup>,
}

fn effect_name(effects: &[effects::Effect], key: &str) -> &'static str {
    effects.iter().find(|e| e.key == key).map_or("Static", |e| e.name)
}

/// A setup in a few words: "Keyboard Wave · light bar Thinking · Gaming".
fn summary(setup: &Setup) -> String {
    let mut parts = vec![format!("Keyboard {}", effect_name(effects::KEYBOARD, &setup.keyboard.effect))];
    if let Some(light_bar) = &setup.light_bar {
        parts.push(match () {
            _ if !light_bar.on => "light bar off".to_string(),
            _ if light_bar.linked => "light bar same as keyboard".to_string(),
            _ => format!("light bar {}", effect_name(effects::LIGHT_BAR, &light_bar.effect)),
        });
    }
    if let Some(mode) = setup.mode {
        parts.push(if setup.fan_boost { format!("{} with Fan Boost", mode.name()) } else { mode.name().to_string() });
    }
    parts.join(" · ")
}

impl Lighting {
    /// Takes over the choices a setup saved for this device.
    fn load(&mut self, kind: Kind, settings: &LightingSettings) {
        let saved = Lighting::new(kind, settings);
        self.on = saved.on;
        self.effect = saved.effect;
        self.zones = saved.zones;
        self.palettes = saved.palettes;
        self.direction = saved.direction;
        self.percent = saved.percent;
        self.speed = saved.speed;
        self.linked = saved.linked;
    }

    fn saved(&self) -> LightingSettings {
        let mut settings = LightingSettings::default();
        self.store(&mut settings);
        settings
    }
}

impl App {
    pub(super) fn bind_setups(&self) {
        if let Some(ui) = self.ui() {
            ui.set_setups(ModelRc::from(self.setups.model.clone()));
            ui.set_setup_names(ModelRc::from(self.setups.names.clone()));
            ui.set_app_profiles(ModelRc::from(self.setups.profiles.clone()));
        }
        self.refresh_setups();
    }

    /// What's on now, as a setup.
    fn snapshot(&self, name: &str) -> Setup {
        let power = self.power.available(self.demo);
        Setup {
            name: name.to_string(),
            keyboard: self.keyboard.saved(),
            light_bar: self.light_bar.as_ref().map(Lighting::saved),
            mode: power.then_some(self.power.mode),
            fan_boost: power && self.power.fan_boost,
        }
    }

    /// Sends a setup's lighting, mode and Fan Boost.
    fn load_setup(&mut self, setup: &Setup) {
        self.keyboard.load(Kind::Keyboard, &setup.keyboard);
        self.refresh_device(Kind::Keyboard);
        self.queue_send(Kind::Keyboard);
        if let (Some(light_bar), Some(saved)) = (self.light_bar.as_mut(), &setup.light_bar) {
            light_bar.load(Kind::LightBar, saved);
            self.refresh_device(Kind::LightBar);
            self.queue_send(Kind::LightBar);
        }
        if let Some(mode) = setup.mode {
            self.set_mode(mode);
            if self.power.fan_boost != setup.fan_boost {
                self.set_fan_boost(setup.fan_boost);
            }
        }
        self.update_tray_icon();
    }

    fn apply_named(&mut self, name: &str) {
        let Some(setup) = self.settings.setups.iter().find(|s| s.name == name).cloned() else { return };
        self.load_setup(&setup);
        self.setups.active = Some(setup.name.clone());
        self.osd.show_text(osd::SETUP_GLYPH, &setup.name);
        self.refresh_setups();
    }

    pub(super) fn apply_setup(&mut self, index: usize) {
        if let Some(name) = self.settings.setups.get(index).map(|s| s.name.clone()) {
            self.apply_named(&name);
        }
    }

    pub(super) fn save_setup(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.set_status("Give the setup a name first.", true, false);
            return;
        }
        let setup = self.snapshot(name);
        match self.settings.setups.iter_mut().find(|s| s.name.eq_ignore_ascii_case(name)) {
            Some(existing) => *existing = Setup { name: existing.name.clone(), ..setup },
            None => self.settings.setups.push(setup),
        }
        self.setups.active = Some(name.to_string());
        self.settings_dirty = true;
        self.set_status(&format!("Saved \"{name}\"."), false, false);
        self.refresh_setups();
    }

    /// Saves what's on now into an existing setup.
    pub(super) fn update_setup(&mut self, index: usize) {
        let Some(name) = self.settings.setups.get(index).map(|s| s.name.clone()) else { return };
        self.settings.setups[index] = self.snapshot(&name);
        self.setups.active = Some(name.clone());
        self.settings_dirty = true;
        self.set_status(&format!("Updated \"{name}\" to what's on now."), false, false);
        self.refresh_setups();
    }

    pub(super) fn delete_setup(&mut self, index: usize) {
        if index >= self.settings.setups.len() {
            return;
        }
        let removed = self.settings.setups.remove(index);
        if self.setups.active.as_deref() == Some(removed.name.as_str()) {
            self.setups.active = None;
        }
        self.settings_dirty = true;
        self.refresh_setups();
    }

    pub(super) fn add_app_profile(&mut self) {
        let Some(first) = self.settings.setups.first().map(|s| s.name.clone()) else {
            self.set_status("Save a setup first: an app profile switches to one of them.", true, false);
            return;
        };
        let owner = self.window_owner();
        let Some(path) = shell::pick_file(owner, "Choose an app", &[("Programs (*.exe)", "*.exe")]) else { return };
        let Some(program) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { return };
        if !self.settings.app_profiles.iter().any(|p| p.program.eq_ignore_ascii_case(&program)) {
            self.settings.app_profiles.push(AppProfile { program, setup: first });
            self.settings_dirty = true;
        }
        self.refresh_setups();
        self.check_app_profiles();
    }

    pub(super) fn remove_app_profile(&mut self, index: usize) {
        if index < self.settings.app_profiles.len() {
            self.settings.app_profiles.remove(index);
            self.settings_dirty = true;
            self.refresh_setups();
            self.check_app_profiles();
        }
    }

    pub(super) fn set_app_profile_setup(&mut self, index: usize, setup: usize) {
        let Some(name) = self.settings.setups.get(setup).map(|s| s.name.clone()) else { return };
        let Some(profile) = self.settings.app_profiles.get_mut(index) else { return };
        profile.setup = name.clone();
        self.settings_dirty = true;
        if self.setups.profile.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(&profile.program)) {
            self.apply_named(&name); // its app is running: show the new choice now
        }
        self.refresh_setups();
    }

    pub(super) fn set_app_profiles_on(&mut self, on: bool) {
        self.settings.app_profiles_enabled = on;
        self.settings_dirty = true;
        self.check_app_profiles();
        self.refresh_setups();
    }

    /// The profile whose program runs, the first in the list if several do.
    fn running_profile(&self) -> Option<AppProfile> {
        if !self.settings.app_profiles_enabled || self.settings.app_profiles.is_empty() {
            return None;
        }
        let running = system::running_programs();
        self.settings
            .app_profiles
            .iter()
            .find(|p| running.contains(&p.program.to_lowercase()) && self.settings.setups.iter().any(|s| s.name == p.setup))
            .cloned()
    }

    /// Called every two seconds: switches to a profile's setup when its program starts, and back when it closes.
    pub(super) fn check_app_profiles(&mut self) {
        // Linux doesn't watch running apps yet.
        if cfg!(target_os = "linux") {
            return;
        }
        let found = self.running_profile();
        let current = self.setups.profile.clone();
        match (current, found) {
            (None, Some(profile)) => {
                self.setups.before = Some(self.snapshot(""));
                self.setups.profile = Some(profile.program.clone());
                self.apply_named(&profile.setup);
            }
            (Some(program), Some(profile)) if !program.eq_ignore_ascii_case(&profile.program) => {
                self.setups.profile = Some(profile.program.clone());
                self.apply_named(&profile.setup);
            }
            (Some(_), None) => {
                self.setups.profile = None;
                if let Some(before) = self.setups.before.take() {
                    self.load_setup(&before);
                }
                self.setups.active = None;
                self.osd.show_text(osd::SETUP_GLYPH, "Back to your setup");
                self.refresh_setups();
            }
            _ => {}
        }
    }

    /// On exit with a profile's app still running, what was on before is what's kept.
    pub(super) fn leave_app_profile(&mut self) {
        let Some(before) = self.setups.before.take() else { return };
        self.setups.profile = None;
        self.keyboard.load(Kind::Keyboard, &before.keyboard);
        self.keyboard.pending = true;
        if let (Some(light_bar), Some(saved)) = (self.light_bar.as_mut(), &before.light_bar) {
            light_bar.load(Kind::LightBar, saved);
            light_bar.pending = true;
        }
        if let Some(mode) = before.mode {
            self.power.mode = mode;
            self.power.fan_boost = before.fan_boost;
        }
    }

    pub(super) fn refresh_setups(&self) {
        let setups = &self.settings.setups;
        let rows: Vec<SetupView> = setups
            .iter()
            .map(|s| SetupView { name: s.name.clone().into(), summary: summary(s).into(), active: self.setups.active.as_deref() == Some(s.name.as_str()) })
            .collect();
        self.setups.model.set_vec(rows);
        self.setups.names.set_vec(setups.iter().map(|s| SharedString::from(s.name.as_str())).collect::<Vec<_>>());
        // Linux doesn't watch running apps, so its page lists none (a backup from Windows may bring some).
        let profiles: Vec<AppProfileView> = self
            .settings
            .app_profiles
            .iter()
            .filter(|_| cfg!(not(target_os = "linux")))
            .map(|p| AppProfileView {
                program: p.program.clone().into(),
                setup: setups.iter().position(|s| s.name == p.setup).map_or(-1, |i| i as i32),
                running: self.setups.profile.as_deref().is_some_and(|r| r.eq_ignore_ascii_case(&p.program)),
            })
            .collect();
        self.setups.profiles.set_vec(profiles);
        if let Some(ui) = self.ui() {
            ui.set_app_profiles_on(self.settings.app_profiles_enabled);
            let status = match &self.setups.profile {
                _ if !self.settings.app_profiles_enabled => "Off: the apps below don't change anything.".to_string(),
                Some(program) => format!("{program} is running, so its setup is on. What was on before comes back when it closes."),
                None if self.settings.app_profiles.is_empty() => "Add an app below and pick the setup it should use.".to_string(),
                None => "None of the apps below is running.".to_string(),
            };
            ui.set_app_profile_status(status.into());
        }
    }

    /// The tray's Setups submenu: (index, name, on now).
    pub(super) fn setup_menu(&self) -> Vec<(usize, String, bool)> {
        self.settings.setups.iter().enumerate().map(|(i, s)| (i, s.name.clone(), self.setups.active.as_deref() == Some(s.name.as_str()))).collect()
    }
}
