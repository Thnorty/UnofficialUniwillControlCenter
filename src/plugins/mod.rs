//! Plugins: small scripts that show things on the light bar, such as Claude Code at work or a download's progress.
//! A plugin is a folder (or a .zip of one) with a `plugin.json` and a Rhai script; see PLUGINS.md.

pub mod claude;
pub mod downloads;
pub mod host;
pub mod layers;
pub mod manifest;
pub mod readings;
pub mod server;
pub mod store;

/// The port plugins are reached on, at 127.0.0.1.
pub const PORT: u16 = 47823;

/// Where a plugin's events are posted.
pub fn url_for(id: &str) -> String {
    format!("http://127.0.0.1:{PORT}/plugins/{id}")
}
