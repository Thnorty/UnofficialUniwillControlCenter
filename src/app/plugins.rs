//! The Plugins page and running the plugins: which are on, their settings, importing and removing them, and what
//! they ask of the app (flashes, pop-ups, status lines, the light bar while they show something).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use serde_json::{Map, Value};
use slint::{Model, ModelRc, VecModel};

use super::{App, Kind, post};
use crate::color::Rgb;
use crate::lighting::scenes::{Overlay, OverlayStyle};
use crate::plugins::host::{Command, FlashStyle, Host, Notice};
use crate::plugins::layers::Layers;
use crate::plugins::manifest::{Permission, Setting, SettingKind};
use crate::plugins::store::{self, Installed};
use crate::plugins::{PORT, server, url_for};
use crate::settings::PluginSettings;
use crate::ui::{PluginActionView, PluginSettingView, PluginView};

pub(super) struct Plugins {
    pub layers: Arc<Layers>,
    host: Option<Host>,
    root: Option<PathBuf>,
    installed: Vec<Installed>,
    load_errors: Vec<String>,
    status: HashMap<String, String>,
    errors: HashMap<String, String>,
    running: HashSet<String>,
    expanded: Option<String>,
    server_error: Option<String>,
    /// Plugins show something on the light bar.
    active: bool,
    pub model: Rc<VecModel<PluginView>>,
    /// Each plugin's settings and buttons, kept and updated in place, so a slider being dragged isn't recreated.
    setting_models: HashMap<String, Rc<VecModel<PluginSettingView>>>,
    action_models: HashMap<String, Rc<VecModel<PluginActionView>>>,
}

impl Default for Plugins {
    fn default() -> Self {
        Plugins {
            layers: Arc::new(Layers::default()),
            host: None,
            root: None,
            installed: Vec::new(),
            load_errors: Vec::new(),
            status: HashMap::new(),
            errors: HashMap::new(),
            running: HashSet::new(),
            expanded: None,
            server_error: None,
            active: false,
            model: Rc::new(VecModel::default()),
            setting_models: HashMap::new(),
            action_models: HashMap::new(),
        }
    }
}

/// Updates the rows that changed, leaving the others (and a slider being dragged) alone.
fn update_settings(model: &VecModel<PluginSettingView>, settings: Vec<PluginSettingView>) {
    let same_shape = model.row_count() == settings.len() && model.iter().zip(&settings).all(|(a, b)| a.key == b.key && a.kind == b.kind);
    if !same_shape {
        model.set_vec(settings);
        return;
    }
    for (i, new) in settings.into_iter().enumerate() {
        let Some(old) = model.row_data(i) else { continue };
        let same = old.on == new.on
            && old.number == new.number
            && old.color == new.color
            && old.text == new.text
            && old.choice == new.choice
            && old.label == new.label
            && old.description == new.description;
        if !same {
            // The choice's options stay the same model, so its box keeps its state.
            model.set_row_data(i, PluginSettingView { options: old.options, ..new });
        }
    }
}

fn setting_kind(kind: &SettingKind) -> i32 {
    match kind {
        SettingKind::Toggle => 0,
        SettingKind::Slider { .. } => 1,
        SettingKind::Color => 2,
        SettingKind::Choice { .. } => 3,
        SettingKind::Text => 4,
    }
}

fn setting_view(setting: &Setting, value: &Value) -> PluginSettingView {
    let (minimum, maximum, step, unit, options) = match &setting.kind {
        SettingKind::Slider { min, max, step, unit } => (*min as f32, *max as f32, *step as f32, unit.clone(), Vec::new()),
        SettingKind::Choice { options } => (0.0, 0.0, 1.0, String::new(), options.clone()),
        _ => (0.0, 0.0, 1.0, String::new(), Vec::new()),
    };
    let choice = options.iter().position(|o| Some(o.as_str()) == value.as_str()).unwrap_or(0) as i32;
    PluginSettingView {
        key: setting.key.clone().into(),
        label: setting.label.clone().into(),
        description: setting.description.clone().into(),
        kind: setting_kind(&setting.kind),
        on: value.as_bool().unwrap_or(false),
        number: value.as_f64().unwrap_or(0.0) as f32,
        minimum,
        maximum,
        step,
        unit: unit.into(),
        color: value.as_str().and_then(Rgb::parse_hex).unwrap_or(Rgb::BLACK).to_slint(),
        text: value.as_str().unwrap_or("").into(),
        options: ModelRc::new(VecModel::from(options.into_iter().map(Into::into).collect::<Vec<slint::SharedString>>())),
        choice,
    }
}

impl App {
    /// Puts the bundled plugins in place, starts the enabled ones and listens for their events.
    pub(super) fn start_plugins(&mut self) {
        let root = store::plugins_dir(self.demo);
        if let Some(root) = &root
            && let Err(e) = std::fs::create_dir_all(root).map_err(anyhow::Error::from).and_then(|()| store::install_builtins(root))
        {
            self.plugins.load_errors.push(format!("Couldn't set up the plugins that come with the app: {e:#}"));
        }
        self.plugins.root = root;
        let host = Host::start(self.plugins.layers.clone(), |notice| post(move |app| app.plugin_notice(notice)));
        if !self.demo {
            let events = host.clone();
            let started = server::start(move |id, path, body| events.send(Command::Event { id: id.to_string(), path: path.to_string(), body }));
            if let Err(e) = started {
                self.plugins.server_error = Some(format!("Plugins can't receive events: port {PORT} is taken ({e})."));
            }
        }
        self.plugins.host = Some(host);
        self.reload_plugins();
    }

    /// Reads the plugins folder again and starts or stops plugins to match.
    fn reload_plugins(&mut self) {
        let Some(root) = self.plugins.root.clone() else { return };
        let (installed, errors) = store::load_all(&root);
        let load_errors: Vec<String> = self.plugins.load_errors.iter().filter(|e| e.starts_with("Couldn't set up")).cloned().chain(errors).collect();
        self.plugins.load_errors = load_errors;
        let ids: HashSet<String> = installed.iter().map(|p| p.manifest.id.clone()).collect();
        for gone in self.plugins.running.clone().into_iter().filter(|id| !ids.contains(id)) {
            self.stop_plugin(&gone);
        }
        self.plugins.installed = installed;
        for i in 0..self.plugins.installed.len() {
            let id = self.plugins.installed[i].manifest.id.clone();
            if self.plugin_enabled(&id) && !self.plugins.running.contains(&id) {
                self.start_plugin(&id);
            }
        }
        self.refresh_plugins();
    }

    fn plugin_enabled(&self, id: &str) -> bool {
        self.settings.plugins.get(id).map_or(store::on_by_default(id), |p| p.enabled)
    }

    fn plugin_values(&self, plugin: &Installed) -> Map<String, Value> {
        plugin.manifest.values(self.settings.plugins.get(&plugin.manifest.id).map(|p| &p.values))
    }

    fn start_plugin(&mut self, id: &str) {
        let Some(plugin) = self.plugins.installed.iter().find(|p| p.manifest.id == id) else { return };
        let Some(host) = &self.plugins.host else { return };
        host.send(Command::Start {
            id: id.to_string(),
            dir: plugin.dir.clone(),
            script: plugin.script.clone(),
            permissions: plugin.manifest.permissions.clone(),
            // Demo mode changes nothing outside the app.
            inert: self.demo,
            settings: self.plugin_values(plugin),
        });
        self.plugins.running.insert(id.to_string());
        self.plugins.errors.remove(id);
        self.plugins.status.remove(id);
    }

    fn stop_plugin(&mut self, id: &str) {
        if let Some(host) = &self.plugins.host {
            host.send(Command::Stop(id.to_string()));
        }
        self.plugins.running.remove(id);
        self.plugins.status.remove(id);
    }

    fn plugin_notice(&mut self, notice: Notice) {
        match notice {
            Notice::Status { id, text } => {
                if self.plugins.status.get(&id) != Some(&text) {
                    self.plugins.status.insert(id, text);
                    self.refresh_plugins();
                }
            }
            Notice::Log { id, text, error: true } => {
                self.plugins.errors.insert(id, text);
                self.refresh_plugins();
            }
            Notice::Log { .. } => {}
            Notice::Stopped { id, error } => {
                self.plugins.running.remove(&id);
                self.plugins.status.remove(&id);
                if let Some(error) = error {
                    self.plugins.errors.insert(id, error);
                }
                self.refresh_plugins();
            }
            Notice::Flash { color, style } => {
                let (style, seconds) = match style {
                    FlashStyle::Open => (OverlayStyle::Open, 1.2),
                    FlashStyle::Close => (OverlayStyle::Close, 0.9),
                    FlashStyle::Blink => (OverlayStyle::Blink, 2.0),
                };
                self.flash(Overlay::new(color, style, seconds));
            }
            Notice::Notify { title, text } => {
                if let Some(tray) = &self.tray {
                    tray.notify(&title, &text);
                }
            }
            Notice::Layers(active) => self.plugin_layers_changed(active),
        }
    }

    /// Plugins started or stopped showing something: a firmware effect makes way for them, and comes back after.
    fn plugin_layers_changed(&mut self, active: bool) {
        if active == self.plugins.active {
            return;
        }
        self.plugins.active = active;
        if self.light_bar.is_some() {
            self.apply(Kind::LightBar, false, None);
        }
    }

    // ---- The page ---------------------------------------------------------------------------------------------------

    pub(super) fn set_plugin_enabled(&mut self, id: &str, on: bool) {
        let entry = self.settings.plugins.entry(id.to_string()).or_insert_with(|| PluginSettings { enabled: store::on_by_default(id), values: Map::new() });
        entry.enabled = on;
        self.settings_dirty = true;
        if on {
            self.start_plugin(id);
        } else {
            self.stop_plugin(id);
            self.plugins.errors.remove(id);
        }
        self.refresh_plugins();
    }

    pub(super) fn expand_plugin(&mut self, id: &str) {
        self.plugins.expanded = if self.plugins.expanded.as_deref() == Some(id) { None } else { Some(id.to_string()) };
        self.refresh_plugins();
    }

    pub(super) fn plugin_action(&mut self, id: &str, action: &str) {
        if !self.plugins.running.contains(id) {
            self.set_status("Turn the plugin on first.", false, false);
            return;
        }
        if let Some(host) = &self.plugins.host {
            host.send(Command::Action { id: id.to_string(), action: action.to_string() });
        }
    }

    /// Changes one setting; `value` is checked against the plugin's manifest.
    pub(super) fn set_plugin_setting(&mut self, id: &str, key: &str, value: Value) {
        let Some(plugin) = self.plugins.installed.iter().find(|p| p.manifest.id == id) else { return };
        let Some(setting) = plugin.manifest.settings.iter().find(|s| s.key == key) else { return };
        let value = setting.check(Some(&value));
        let entry = self.settings.plugins.entry(id.to_string()).or_insert_with(|| PluginSettings { enabled: store::on_by_default(id), values: Map::new() });
        if entry.values.get(key) == Some(&value) {
            return;
        }
        entry.values.insert(key.to_string(), value);
        self.settings_dirty = true;
        self.send_plugin_settings(id);
        self.refresh_plugins();
    }

    /// A choice setting, by the option's place in its list.
    pub(super) fn set_plugin_choice(&mut self, id: &str, key: &str, choice: usize) {
        let option = self.plugins.installed.iter().find(|p| p.manifest.id == id).and_then(|p| p.manifest.settings.iter().find(|s| s.key == key)).and_then(
            |s| match &s.kind {
                SettingKind::Choice { options } => options.get(choice).cloned(),
                _ => None,
            },
        );
        if let Some(option) = option {
            self.set_plugin_setting(id, key, Value::String(option));
        }
    }

    /// A color setting, from the color picker: `plugin` and `setting` are indexes on the page.
    pub(super) fn set_plugin_color(&mut self, plugin: usize, setting: usize, color: Rgb) {
        let Some(installed) = self.plugins.installed.get(plugin) else { return };
        let Some(key) = installed.manifest.settings.get(setting).map(|s| s.key.clone()) else { return };
        let id = installed.manifest.id.clone();
        self.set_plugin_setting(&id, &key, Value::String(color.hex()));
    }

    pub(super) fn reset_plugin_settings(&mut self, id: &str) {
        if let Some(entry) = self.settings.plugins.get_mut(id) {
            entry.values.clear();
            self.settings_dirty = true;
        }
        self.send_plugin_settings(id);
        self.refresh_plugins();
    }

    fn send_plugin_settings(&self, id: &str) {
        let (Some(host), Some(plugin)) = (&self.plugins.host, self.plugins.installed.iter().find(|p| p.manifest.id == id)) else { return };
        host.send(Command::Settings { id: id.to_string(), values: self.plugin_values(plugin) });
    }

    pub(super) fn import_plugin(&mut self) {
        let Some(root) = self.plugins.root.clone() else { return };
        let owner = self.window_hwnd();
        let Some(path) = crate::shell::pick_file(owner, "Import a plugin", &[("Plugins (*.zip, plugin.json)", "*.zip;plugin.json")]) else { return };
        let before: HashMap<String, Vec<Permission>> = self.plugins.installed.iter().map(|p| (p.manifest.id.clone(), p.manifest.permissions.clone())).collect();
        match store::import(&root, &path) {
            Ok(manifest) => {
                let id = manifest.id.clone();
                let was_running = self.plugins.running.contains(&id);
                if was_running {
                    self.stop_plugin(&id);
                }
                // A plugin that asks for permissions it didn't have waits to be turned on, so they're seen first.
                let had = before.get(&id).cloned().unwrap_or_default();
                let new_permissions = manifest.permissions.iter().any(|p| !had.contains(p));
                let entry = self.settings.plugins.entry(id.clone()).or_insert_with(|| PluginSettings { enabled: true, values: Map::new() });
                if new_permissions {
                    entry.enabled = false;
                }
                self.settings_dirty = true;
                self.plugins.expanded = Some(id.clone());
                self.reload_plugins();
                let message = if self.plugin_enabled(&id) {
                    format!("Imported {} {}.", manifest.name, manifest.version)
                } else {
                    format!("Imported {}. Check what it may do, then turn it on.", manifest.name)
                };
                self.set_status(&message, false, false);
            }
            Err(e) => self.set_status(&format!("Couldn't import the plugin: {e:#}"), true, false),
        }
    }

    pub(super) fn remove_plugin(&mut self, id: &str) {
        let Some(root) = self.plugins.root.clone() else { return };
        self.stop_plugin(id);
        match store::remove(&root, id) {
            Ok(()) => {
                self.settings.plugins.remove(id);
                self.settings_dirty = true;
                self.reload_plugins();
            }
            Err(e) => self.set_status(&format!("{e:#}"), true, false),
        }
    }

    pub(super) fn open_plugins_folder(&self) {
        if let Some(root) = &self.plugins.root {
            crate::shell::open_path(root);
        }
    }

    pub(super) fn refresh_plugins(&mut self) {
        let Some(ui) = self.ui() else { return };
        let mut views = Vec::new();
        for p in &self.plugins.installed {
            let m = &p.manifest;
            let enabled = self.plugin_enabled(&m.id);
            let values = self.plugin_values(p);
            let settings: Vec<PluginSettingView> = m.settings.iter().map(|s| setting_view(s, &values[&s.key])).collect();
            let settings_model = self.plugins.setting_models.entry(m.id.clone()).or_default();
            update_settings(settings_model, settings);
            let actions: Vec<PluginActionView> = m.actions.iter().map(|a| PluginActionView { id: a.id.clone().into(), label: a.label.clone().into() }).collect();
            let actions_model = self.plugins.action_models.entry(m.id.clone()).or_default();
            if actions_model.iter().collect::<Vec<_>>() != actions {
                actions_model.set_vec(actions);
            }
            let permissions: Vec<&str> = m.permissions.iter().map(|p| p.description()).collect();
            let plugins = &self.plugins;
            let status = match plugins.status.get(&m.id) {
                _ if !enabled => "Off".to_string(),
                Some(status) => status.clone(),
                None if plugins.running.contains(&m.id) => "On".to_string(),
                None => "Stopped".to_string(),
            };
            views.push(PluginView {
                id: m.id.clone().into(),
                name: m.name.clone().into(),
                version: m.version.clone().into(),
                author: m.author.clone().into(),
                description: m.description.clone().into(),
                status: status.into(),
                error: if enabled { plugins.errors.get(&m.id).cloned().unwrap_or_default().into() } else { "".into() },
                enabled,
                builtin: p.builtin,
                expanded: plugins.expanded.as_deref() == Some(m.id.as_str()),
                permissions: if permissions.is_empty() { "".into() } else { format!("It may {}.", permissions.join(" and ")).into() },
                url: url_for(&m.id).into(),
                settings: ModelRc::from(plugins.setting_models[&m.id].clone()),
                actions: ModelRc::from(plugins.action_models[&m.id].clone()),
            });
        }
        let plugins = &self.plugins;
        let model = &plugins.model;
        let same_plugins = model.row_count() == views.len() && model.iter().zip(&views).all(|(a, b)| a.id == b.id);
        if same_plugins {
            for (i, view) in views.into_iter().enumerate() {
                if model.row_data(i).as_ref() != Some(&view) {
                    model.set_row_data(i, view);
                }
            }
        } else {
            model.set_vec(views);
        }
        let mut notes: Vec<String> = Vec::new();
        if self.demo {
            notes.push("Demo mode: plugins receive no events, and what their permissions allow is only pretended.".into());
        } else if let Some(error) = &plugins.server_error {
            notes.push(error.clone());
        }
        notes.extend(plugins.load_errors.iter().map(|e| format!("Not loaded: {e}")));
        ui.set_plugins_note(notes.join("\n").into());
    }
}
