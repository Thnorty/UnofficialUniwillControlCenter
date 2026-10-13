// Claude Code on the light bar. Claude Code posts each hook event here once "Connect Claude Code" has added its
// hooks. Every session (each Claude Code window) is followed on its own; the bar shows the one that matters
// most: one that needs you, then one at work, then one that just finished.

fn hook_events() {
    ["UserPromptSubmit", "PreToolUse", "PostToolUse", "PostToolUseFailure", "Notification", "Stop", "SessionEnd"]
}

fn init() {
    this.sessions = #{};
    this.connected = claude_hooks_installed() > 0;
    if this.connected {
        // Brings hooks from an earlier version up to date (they were reported as failed while the app was closed).
        try {
            install_claude_hooks(hook_events());
        } catch {
        }
    }
    set_tick(1000);
    this.update();
}

fn on_action(id) {
    if id == "connect" {
        install_claude_hooks(hook_events());
    } else if id == "disconnect" {
        remove_claude_hooks();
        this.sessions = #{};
    }
    this.connected = claude_hooks_installed() > 0;
    this.update();
}

fn on_settings() {
    this.update();
}

fn on_event(path, e) {
    let id = e.session_id;
    if id == () {
        id = "default";
    }
    if !this.sessions.contains(id) {
        this.sessions[id] = #{ state: "idle", total: 0, done: 0, completed: [], seen: now(), stopped: 0.0 };
    }
    let s = this.sessions[id];
    s.seen = now();
    let event = e.hook_event_name;

    if event == "UserPromptSubmit" {
        s.state = "working";
        if s.total > 0 && s.done >= s.total {
            s.total = 0;
            s.done = 0;
            s.completed = [];
        }
    } else if event == "PreToolUse" || event == "PostToolUse" || event == "PostToolUseFailure" {
        // The hooks run in the background, so a tool's event can arrive just after Stop: it doesn't restart work.
        if s.state != "working" && now() - s.stopped < 3.0 {
            this.sessions[id] = s;
            return;
        }
        s.state = "working";
        let input = e.tool_input;
        if e.tool_name == "TodoWrite" && type_of(input) == "map" && type_of(input.todos) == "array" {
            // The whole list each time.
            s.total = input.todos.len();
            s.done = input.todos.filter(|t| t.status == "completed").len();
        } else if event == "PostToolUse" && e.tool_name == "TaskCreate" {
            s.total += 1;
        } else if event == "PostToolUse" && e.tool_name == "TaskUpdate" && type_of(input) == "map" {
            let task = `${input.taskId}`;
            if input.status == "completed" && !s.completed.contains(task) {
                s.completed.push(task);
                s.done += 1;
            } else if input.status == "deleted" && s.total > 0 {
                s.total -= 1;
            }
        }
    } else if event == "Notification" {
        let kind = e.notification_type;
        if kind == "permission_prompt" || kind == "elicitation_dialog" || kind == "elicitation_url_dialog" || kind == "agent_needs_input" {
            s.state = "attention";
            if setting("notify") {
                notify("Claude Code needs you", if e.message == () { "Claude is waiting for you." } else { e.message });
            }
        } else if kind == "idle_prompt" && s.state != "done" {
            s.state = "idle";
        }
    } else if event == "Stop" {
        s.stopped = now();
        let done = setting("done");
        if done == "Flash" {
            flash(setting("done_color"), "open");
            s.state = "idle";
        } else if done == "Nothing" {
            s.state = "idle";
        } else {
            s.state = "done";
        }
    } else if event == "SessionEnd" {
        this.sessions.remove(id);
        this.update();
        return;
    }
    this.sessions[id] = s;
    this.update();
}

fn on_tick() {
    // Forget what's gone quiet: a session that crashed or was closed sends nothing more.
    let t = now();
    let ids = this.sessions.keys();
    for id in ids {
        let s = this.sessions[id];
        let quiet = t - s.seen;
        if (s.state == "working" && quiet > 900.0) || (s.state == "attention" && quiet > 3600.0) || (s.state == "done" && quiet > 1800.0) {
            s.state = "idle";
            this.sessions[id] = s;
        }
        if s.state == "idle" && quiet > 3600.0 {
            this.sessions.remove(id);
        }
    }
    this.update();
}

// What the bar shows, from every session.
fn update() {
    let attention = 0;
    let working = 0;
    let finished = 0;
    let best = ();
    for s in this.sessions.values() {
        if s.state == "attention" {
            attention += 1;
        } else if s.state == "working" {
            working += 1;
            if s.total > 0 && (best == () || s.seen > best.seen) {
                best = s;
            }
        } else if s.state == "done" {
            finished += 1;
        }
    }

    if attention > 0 {
        pulse(setting("attention_color"));
    } else if working > 0 && best != () && setting("show_tasks") {
        progress(best.done.to_float() / best.total.to_float(), setting("working_color"));
    } else if working > 0 {
        activity(setting("working_color"));
    } else if finished > 0 {
        solid(setting("done_color"));
    } else {
        clear();
    }

    if !this.connected {
        status("Not connected: press Connect Claude Code to add its hooks to Claude Code's settings.");
    } else if attention > 0 {
        status("Claude needs you");
    } else if working > 0 && best != () {
        status(`Working: ${best.done} of ${best.total} tasks done`);
    } else if working > 0 {
        status(if working == 1 { "Working" } else { `${working} sessions working` });
    } else {
        status("Connected. Waiting for Claude Code.");
    }
}
