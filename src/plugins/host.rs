//! Runs the plugins' scripts on a thread of their own. Each enabled plugin gets a Rhai engine with only this app's
//! plugin API: no files, no network, no other programs, and limits on how long a call may run. A plugin keeps its
//! state in `this`, a map that lasts while it's on.
//!
//! The script's functions, all optional: `init()`, `on_event(path, body)` for what's posted to its address,
//! `on_tick()` every `set_tick(ms)`, `on_settings()` after its settings change and `on_action(id)` for its buttons.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rhai::{AST, Array, CallFnOptions, Dynamic, Engine, EvalAltResult, FuncArgs, ImmutableString, Map as ScriptMap, Scope};
use serde_json::{Map, Value};

use super::layers::{Layer, Layers};
use super::manifest::Permission;
use super::{claude, url_for};
use crate::color::Rgb;

/// What the app asks of the plugin thread.
pub enum Command {
    /// `inert`: what its permissions allow is pretended, not done (demo mode).
    Start { id: String, dir: PathBuf, script: String, permissions: Vec<Permission>, inert: bool, settings: Map<String, Value> },
    Stop(String),
    Settings { id: String, values: Map<String, Value> },
    Event { id: String, path: String, body: Value },
    Action { id: String, action: String },
}

/// What the plugin thread tells the app.
pub enum Notice {
    Status { id: String, text: String },
    Log { id: String, text: String, error: bool },
    /// The plugin stopped: its script couldn't start, or it was turned off.
    Stopped { id: String, error: Option<String> },
    Flash { color: Rgb, style: FlashStyle },
    Notify { title: String, text: String },
    /// Whether any plugin shows something on the light bar now.
    Layers(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlashStyle {
    Open,
    Close,
    Blink,
}

type NoticeFn = Arc<dyn Fn(Notice) + Send + Sync>;

/// A handle to the plugin thread; clones share it.
#[derive(Clone)]
pub struct Host {
    commands: Sender<Command>,
}

impl Host {
    pub fn start(layers: Arc<Layers>, notice: impl Fn(Notice) + Send + Sync + 'static) -> Host {
        let (commands, receiver) = mpsc::channel();
        let notice: NoticeFn = Arc::new(notice);
        let _ = std::thread::Builder::new().name("Plugins".into()).spawn(move || run(receiver, layers, notice));
        Host { commands }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

/// What a running plugin's API functions share.
struct Context {
    id: String,
    dir: PathBuf,
    permissions: Vec<Permission>,
    inert: bool,
    layers: Arc<Layers>,
    notice: NoticeFn,
    settings: Mutex<ScriptMap>,
    tick: Mutex<Option<Duration>>,
    last_notify: Mutex<Option<Instant>>,
}

impl Context {
    fn send(&self, notice: Notice) {
        (self.notice)(notice);
    }

    fn show(&self, layer: Layer) {
        if self.layers.set(&self.id, layer) {
            self.send(Notice::Layers(true));
        }
    }

    fn clear(&self) {
        if self.layers.clear(&self.id) {
            self.send(Notice::Layers(false));
        }
    }

    fn require(&self, permission: Permission) -> Result<(), Box<EvalAltResult>> {
        if self.permissions.contains(&permission) {
            Ok(())
        } else {
            Err(format!("This plugin needs the \"{}\" permission in its plugin.json", serde_json::to_value(permission).unwrap().as_str().unwrap_or("")).into())
        }
    }
}

struct Running {
    context: Arc<Context>,
    engine: Engine,
    ast: AST,
    scope: Scope<'static>,
    state: Dynamic,
    next_tick: Option<Instant>,
}

impl Running {
    fn has(&self, name: &str, arity: usize) -> bool {
        self.ast.iter_functions().any(|f| f.name == name && f.params.len() == arity)
    }

    /// Calls a script function if the script has it.
    fn call(&mut self, name: &str, args: impl FuncArgs, arity: usize) -> Result<(), String> {
        if !self.has(name, arity) {
            return Ok(());
        }
        let options = CallFnOptions::new().eval_ast(false).rewind_scope(true).bind_this_ptr(&mut self.state);
        self.engine.call_fn_with_options::<Dynamic>(options, &mut self.scope, &self.ast, name, args).map(|_| ()).map_err(|e| format!("{name}: {e}"))
    }

    fn report(&self, result: Result<(), String>) {
        if let Err(error) = result {
            self.context.send(Notice::Log { id: self.context.id.clone(), text: error, error: true });
        }
    }

    fn schedule(&mut self) {
        let tick = *self.context.tick.lock().unwrap_or_else(|e| e.into_inner());
        self.next_tick = match (tick, self.next_tick) {
            (None, _) => None,
            (Some(_), Some(next)) => Some(next),
            (Some(every), None) => Some(Instant::now() + every),
        };
    }
}

fn run(receiver: Receiver<Command>, layers: Arc<Layers>, notice: NoticeFn) {
    let mut plugins: HashMap<String, Running> = HashMap::new();
    loop {
        let next = plugins.values().filter_map(|p| p.next_tick).min();
        let command = match next {
            Some(at) => receiver.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match command {
            Ok(Command::Start { id, dir, script, permissions, inert, settings }) => {
                if let Some(old) = plugins.remove(&id) {
                    old.context.clear();
                }
                match start(&id, dir, &script, permissions, inert, settings, layers.clone(), notice.clone()) {
                    Ok(plugin) => {
                        plugins.insert(id, plugin);
                    }
                    Err(error) => {
                        layers.clear(&id);
                        notice(Notice::Layers(layers.is_active()));
                        notice(Notice::Stopped { id, error: Some(error) });
                    }
                }
            }
            Ok(Command::Stop(id)) => {
                if let Some(plugin) = plugins.remove(&id) {
                    plugin.context.clear();
                    notice(Notice::Stopped { id, error: None });
                }
            }
            Ok(Command::Settings { id, values }) => {
                if let Some(plugin) = plugins.get_mut(&id) {
                    *plugin.context.settings.lock().unwrap_or_else(|e| e.into_inner()) = to_script_map(&values);
                    let result = plugin.call("on_settings", (), 0);
                    plugin.report(result);
                    plugin.schedule();
                }
            }
            Ok(Command::Event { id, path, body }) => {
                if let Some(plugin) = plugins.get_mut(&id) {
                    let body = rhai::serde::to_dynamic(&body).unwrap_or(Dynamic::UNIT);
                    let result = plugin.call("on_event", (ImmutableString::from(path), body), 2);
                    plugin.report(result);
                    plugin.schedule();
                }
            }
            Ok(Command::Action { id, action }) => {
                if let Some(plugin) = plugins.get_mut(&id) {
                    let result = plugin.call("on_action", (ImmutableString::from(action),), 1);
                    plugin.report(result);
                    plugin.schedule();
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // Ticks that are due.
        let now = Instant::now();
        for plugin in plugins.values_mut() {
            if plugin.next_tick.is_some_and(|at| at <= now) {
                plugin.next_tick = None;
                let result = plugin.call("on_tick", (), 0);
                plugin.report(result);
                plugin.schedule();
            }
        }
    }
    for plugin in plugins.values() {
        plugin.context.clear();
    }
}

#[allow(clippy::too_many_arguments)]
fn start(
    id: &str,
    dir: PathBuf,
    script: &str,
    permissions: Vec<Permission>,
    inert: bool,
    settings: Map<String, Value>,
    layers: Arc<Layers>,
    notice: NoticeFn,
) -> Result<Running, String> {
    let context = Arc::new(Context {
        id: id.to_string(),
        dir,
        permissions,
        inert,
        layers,
        notice,
        settings: Mutex::new(to_script_map(&settings)),
        tick: Mutex::new(None),
        last_notify: Mutex::new(None),
    });
    let engine = engine(&context);
    let ast = engine.compile(script).map_err(|e| format!("The script has an error: {e}"))?;
    let mut scope = Scope::new();
    engine.run_ast_with_scope(&mut scope, &ast).map_err(|e| format!("The script stopped: {e}"))?;
    let mut plugin = Running { context, engine, ast, scope, state: Dynamic::from_map(ScriptMap::new()), next_tick: None };
    plugin.call("init", (), 0).map_err(|e| format!("The script stopped: {e}"))?;
    plugin.schedule();
    Ok(plugin)
}

fn to_script_map(values: &Map<String, Value>) -> ScriptMap {
    rhai::serde::to_dynamic(values).ok().and_then(|d| d.try_cast::<ScriptMap>()).unwrap_or_default()
}

type Fallible<T> = Result<T, Box<EvalAltResult>>;

fn color(text: &str) -> Fallible<Rgb> {
    Rgb::parse_hex(text).ok_or_else(|| format!("\"{text}\" isn't a color like \"#FF8800\"").into())
}

fn number(value: &Dynamic) -> Fallible<f64> {
    value.as_float().or_else(|_| value.as_int().map(|i| i as f64)).map_err(|_| "Expected a number".into())
}

/// The engine for one plugin, with its API and limits.
fn engine(context: &Arc<Context>) -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(2_000_000);
    engine.set_max_call_levels(48);
    engine.set_max_expr_depths(64, 64);
    engine.set_max_string_size(1 << 20);
    engine.set_max_array_size(50_000);
    engine.set_max_map_size(10_000);
    engine.disable_symbol("eval");

    let c = context.clone();
    engine.on_print(move |text| c.send(Notice::Log { id: c.id.clone(), text: text.to_string(), error: false }));
    let c = context.clone();
    engine.on_debug(move |text, _, _| c.send(Notice::Log { id: c.id.clone(), text: text.to_string(), error: false }));

    // The light bar.
    let c = context.clone();
    engine.register_fn("progress", move |value: Dynamic, color_text: &str| -> Fallible<()> {
        let value = number(&value)? as f32;
        c.show(Layer::Progress { value, color: color(color_text)? });
        Ok(())
    });
    let c = context.clone();
    engine.register_fn("activity", move |color_text: &str| -> Fallible<()> {
        c.show(Layer::Activity(color(color_text)?));
        Ok(())
    });
    let c = context.clone();
    engine.register_fn("pulse", move |color_text: &str| -> Fallible<()> {
        c.show(Layer::Pulse(color(color_text)?));
        Ok(())
    });
    let c = context.clone();
    engine.register_fn("solid", move |color_text: &str| -> Fallible<()> {
        c.show(Layer::Solid(color(color_text)?));
        Ok(())
    });
    let c = context.clone();
    engine.register_fn("leds", move |colors: Array| -> Fallible<()> {
        let colors = colors.iter().take(64).map(|v| color(&v.to_string())).collect::<Fallible<Vec<_>>>()?;
        c.show(Layer::Leds(colors));
        Ok(())
    });
    let c = context.clone();
    engine.register_fn("clear", move || c.clear());
    for (name, with_style) in [("flash", false), ("flash", true)] {
        let c = context.clone();
        if with_style {
            engine.register_fn(name, move |color_text: &str, style: &str| -> Fallible<()> {
                let style = match style {
                    "open" => FlashStyle::Open,
                    "close" => FlashStyle::Close,
                    "blink" => FlashStyle::Blink,
                    _ => return Err(format!("\"{style}\" isn't \"open\", \"close\" or \"blink\"").into()),
                };
                c.send(Notice::Flash { color: color(color_text)?, style });
                Ok(())
            });
        } else {
            engine.register_fn(name, move |color_text: &str| -> Fallible<()> {
                c.send(Notice::Flash { color: color(color_text)?, style: FlashStyle::Open });
                Ok(())
            });
        }
    }

    // The app.
    let c = context.clone();
    engine.register_fn("status", move |text: &str| c.send(Notice::Status { id: c.id.clone(), text: text.chars().take(200).collect() }));
    let c = context.clone();
    engine.register_fn("notify", move |title: &str, text: &str| {
        let mut last = c.last_notify.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
            return; // no flood of pop-ups
        }
        *last = Some(Instant::now());
        c.send(Notice::Notify { title: title.chars().take(64).collect(), text: text.chars().take(200).collect() });
    });
    let c = context.clone();
    engine.register_fn("set_tick", move |ms: i64| {
        *c.tick.lock().unwrap_or_else(|e| e.into_inner()) = (ms > 0).then(|| Duration::from_millis(ms.max(30) as u64));
    });
    engine.register_fn("now", || SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()));
    let c = context.clone();
    engine.register_fn("setting", move |key: &str| c.settings.lock().unwrap_or_else(|e| e.into_inner()).get(key).cloned().unwrap_or(Dynamic::UNIT));
    let c = context.clone();
    engine.register_fn("settings", move || c.settings.lock().unwrap_or_else(|e| e.into_inner()).clone());
    let c = context.clone();
    engine.register_fn("plugin_url", move || url_for(&c.id));
    let c = context.clone();
    engine.register_fn("open_folder", move |sub: &str| -> Fallible<()> {
        let relative = Path::new(sub);
        if relative.components().any(|part| !matches!(part, Component::Normal(_))) {
            return Err("Only folders inside the plugin's own folder can be opened".into());
        }
        crate::shell::open_path(&c.dir.join(relative));
        Ok(())
    });

    // With permissions.
    let c = context.clone();
    engine.register_fn("partial_downloads", move || -> Fallible<Array> {
        c.require(Permission::DownloadsFolder)?;
        if c.inert {
            return Ok(Array::new());
        }
        Ok(super::downloads::partial().into_iter().map(Dynamic::from_map).collect())
    });
    let c = context.clone();
    engine.register_fn("install_claude_hooks", move |events: Array| -> Fallible<i64> {
        c.require(Permission::ClaudeHooks)?;
        if c.inert {
            return Err("Demo mode doesn't change Claude Code's settings".into());
        }
        let events: Vec<String> = events.iter().map(|e| e.to_string()).collect();
        claude::install(&url_for(&c.id), &events).map(|n| n as i64).map_err(|e| format!("{e:#}").into())
    });
    let c = context.clone();
    engine.register_fn("remove_claude_hooks", move || -> Fallible<i64> {
        c.require(Permission::ClaudeHooks)?;
        if c.inert {
            return Err("Demo mode doesn't change Claude Code's settings".into());
        }
        claude::remove(&url_for(&c.id)).map(|n| n as i64).map_err(|e| format!("{e:#}").into())
    });
    let c = context.clone();
    engine.register_fn("claude_hooks_installed", move || -> Fallible<i64> {
        c.require(Permission::ClaudeHooks)?;
        Ok(if c.inert { 0 } else { claude::installed(&url_for(&c.id)) as i64 })
    });
    engine
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> (Host, Arc<Layers>, Receiver<String>) {
        let layers = Arc::new(Layers::default());
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let host = Host::start(layers.clone(), move |n| {
            let text = match n {
                Notice::Status { text, .. } => format!("status {text}"),
                Notice::Log { text, error, .. } => format!("log {error} {text}"),
                Notice::Stopped { error, .. } => format!("stopped {error:?}"),
                Notice::Layers(on) => format!("layers {on}"),
                _ => "other".into(),
            };
            let _ = tx.lock().unwrap().send(text);
        });
        (host, layers, rx)
    }

    fn start(host: &Host, script: &str) {
        let settings: Map<String, Value> = serde_json::from_str(r##"{ "color": "#00FF00" }"##).unwrap();
        host.send(Command::Start { id: "t".into(), dir: std::env::temp_dir(), script: script.into(), permissions: vec![], inert: false, settings });
    }

    #[test]
    fn runs_events_with_state_and_limits() {
        let (host, layers, rx) = host();
        start(
            &host,
            r#"
            fn init() { this.count = 0; }
            fn on_event(path, body) {
                this.count += 1;
                progress(body.done / body.total, setting("color"));
                status(`${path} ${this.count}`);
            }
            "#,
        );
        host.send(Command::Event { id: "t".into(), path: "/x".into(), body: serde_json::json!({ "done": 1, "total": 4 }) });
        host.send(Command::Event { id: "t".into(), path: "/x".into(), body: serde_json::json!({ "done": 2, "total": 4 }) });
        let wait = || rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(wait(), "layers true");
        assert_eq!(wait(), "status /x 1");
        assert_eq!(wait(), "status /x 2");
        assert!(layers.is_active());

        // A runaway loop is stopped, and so is a missing permission.
        start(&host, "fn on_event(p, b) { loop {} } fn on_action(a) { install_claude_hooks([]); }");
        assert_eq!(wait(), "layers false"); // restarting cleared the old layer
        host.send(Command::Event { id: "t".into(), path: "".into(), body: Value::Null });
        assert!(wait().starts_with("log true on_event"));
        host.send(Command::Action { id: "t".into(), action: "x".into() });
        assert!(wait().contains("permission"));

        // Inert (demo mode), a permission changes nothing.
        let script = "fn on_action(a) { status(`${claude_hooks_installed()}`); install_claude_hooks([]); }";
        host.send(Command::Start { id: "t".into(), dir: std::env::temp_dir(), script: script.into(), permissions: vec![Permission::ClaudeHooks], inert: true, settings: Map::new() });
        host.send(Command::Action { id: "t".into(), action: "x".into() });
        assert_eq!(wait(), "status 0");
        assert!(wait().contains("Demo mode"));
    }

    /// Runs a bundled plugin with its default settings.
    fn bundled(host: &Host, id: &str, permissions: Vec<Permission>) {
        let manifest = super::super::manifest::Manifest::parse(&std::fs::read_to_string(format!("plugins/{id}/plugin.json")).unwrap()).unwrap();
        let script = std::fs::read_to_string(format!("plugins/{id}/main.rhai")).unwrap();
        host.send(Command::Start { id: id.into(), dir: std::env::temp_dir(), script, permissions, inert: false, settings: manifest.values(None) });
    }

    /// What the plugin sends until it goes quiet, or for a second at most (a ticking plugin never does).
    fn settle(rx: &Receiver<String>) -> Vec<String> {
        let until = Instant::now() + Duration::from_secs(1);
        let mut seen = Vec::new();
        while Instant::now() < until
            && let Ok(text) = rx.recv_timeout(Duration::from_millis(300))
        {
            seen.push(text);
        }
        seen
    }

    #[test]
    fn claude_code_plugin() {
        let (host, layers, rx) = host();
        bundled(&host, "claude-code", vec![Permission::ClaudeHooks]);
        let event = |body: serde_json::Value| host.send(Command::Event { id: "claude-code".into(), path: "".into(), body });
        settle(&rx);
        assert!(layers.top_layer().is_none());

        event(serde_json::json!({ "session_id": "a", "hook_event_name": "UserPromptSubmit" }));
        settle(&rx);
        assert!(matches!(layers.top_layer(), Some((_, Layer::Activity(_)))));

        event(serde_json::json!({ "session_id": "a", "hook_event_name": "PreToolUse", "tool_name": "TodoWrite",
            "tool_input": { "todos": [ { "status": "completed" }, { "status": "in_progress" }, { "status": "pending" }, { "status": "pending" } ] } }));
        let log = settle(&rx);
        assert!(matches!(layers.top_layer(), Some((_, Layer::Progress { value, .. })) if (value - 0.25).abs() < 0.01), "{log:?}");

        event(serde_json::json!({ "session_id": "b", "hook_event_name": "Notification", "notification_type": "permission_prompt", "message": "Allow?" }));
        settle(&rx);
        assert!(matches!(layers.top_layer(), Some((_, Layer::Pulse(_)))));

        event(serde_json::json!({ "session_id": "b", "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": {} }));
        event(serde_json::json!({ "session_id": "b", "hook_event_name": "Stop" }));
        event(serde_json::json!({ "session_id": "a", "hook_event_name": "Stop" }));
        let log = settle(&rx);
        assert!(layers.top_layer().is_none(), "{log:?}");
        assert!(!log.iter().any(|l| l.starts_with("log true")), "{log:?}");
    }

    #[test]
    fn downloads_plugin() {
        let (host, layers, rx) = host();
        bundled(&host, "downloads", vec![Permission::DownloadsFolder]);
        settle(&rx);
        host.send(Command::Event {
            id: "downloads".into(),
            path: "/progress".into(),
            body: serde_json::json!({ "downloads": [ { "id": 1, "name": "a.iso", "received": 300, "total": 1000, "state": "in_progress", "paused": false } ] }),
        });
        let log = settle(&rx);
        assert!(matches!(layers.top_layer(), Some((_, Layer::Progress { value, .. })) if (value - 0.3).abs() < 0.01), "{log:?}");
        assert!(log.iter().any(|l| l == "status a.iso: 30%"), "{log:?}");
        host.send(Command::Event { id: "downloads".into(), path: "/progress".into(), body: serde_json::json!({ "downloads": [] }) });
        let log = settle(&rx);
        assert!(!log.iter().any(|l| l.starts_with("log true")), "{log:?}");
    }

    #[test]
    fn example_plugin() {
        let (host, layers, rx) = host();
        bundled(&host, "example", vec![]);
        let log = settle(&rx);
        assert!(matches!(layers.top_layer(), Some((_, Layer::Progress { .. }))), "{log:?}");
        assert!(log.iter().any(|l| l.starts_with("status Example: ")), "{log:?}");
        host.send(Command::Action { id: "example".into(), action: "flash".into() });
        host.send(Command::Settings { id: "example".into(), values: serde_json::from_str(r##"{ "color": "#00FF00", "seconds": 5, "style": "Pulse", "show": true, "label": "x" }"##).unwrap() });
        let log = settle(&rx);
        assert!(matches!(layers.top_layer(), Some((_, Layer::Pulse(_)))), "{log:?}");
        assert!(!log.iter().any(|l| l.starts_with("log true")), "{log:?}");
    }
}
