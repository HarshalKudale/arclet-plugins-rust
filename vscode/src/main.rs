//! VS Code plugin for Arclet — drives VS Code through the Arclet VS Code
//! companion extension's local action server.

use arclet_sdk::input::{self, InputExt};
use arclet_sdk::{env_or, json, load_env, Action, Error, Plugin, Result, Value};
use std::time::Duration;

const NAME: &str = "VS Code companion";

fn host() -> String {
    env_or("VSCODE_HOST", "127.0.0.1")
}
fn default_port() -> String {
    env_or("VSCODE_PORT", "32145")
}

fn base_inputs() -> Vec<Value> {
    vec![input::text("port", "Action Port (Optional)").placeholder(&format!("Default {}", default_port()))]
}

/// Port override from the instance inputs, else the configured default.
fn resolve_port(inputs: &Value) -> String {
    match inputs["port"].as_str().map(str::trim) {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => default_port(),
    }
}

/// encodeURIComponent
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn get(path: &str, inputs: &Value, params: &[(&str, &str)]) -> Result<ureq::Response> {
    let port = resolve_port(inputs);
    let mut req = ureq::get(&format!("http://{}:{port}{path}", host())).timeout(Duration::from_secs(10));
    let password = env_or("VSCODE_PASSWORD", "");
    if !password.is_empty() {
        req = req.set("Authorization", &password);
    }
    for (k, v) in params.iter().filter(|(_, v)| !v.is_empty()) {
        req = req.query(k, v);
    }
    match req.call() {
        Ok(r) => Ok(r),
        Err(ureq::Error::Status(401 | 403, _)) => {
            arclet_sdk::mark_setup_required(format!("{NAME} rejected the password"));
            Err(Error::msg(format!("{NAME} rejected the configured password.")))
        }
        Err(ureq::Error::Status(code, r)) => {
            let detail = r.into_json::<Value>().ok().and_then(|b| b["error"].as_str().map(|e| format!(" {e}"))).unwrap_or_default();
            Err(Error::msg(format!("{NAME} returned HTTP {code}.{detail}")))
        }
        Err(e) => Err(Error::msg(format!("{NAME} is not reachable on port {port}. ({e})"))),
    }
}

fn execute_command(command_id: &str, inputs: &Value, params: &[(&str, &str)]) -> Result {
    get(&format!("/api/action/{}", enc(command_id)), inputs, params).map(|_| ())
}

/// `[{ value, label }]` from the companion's `/api/query/<name>`.
fn fetch_options(name: &str, inputs: &Value) -> Result<Vec<Value>> {
    let data: Value = get(&format!("/api/query/{name}"), inputs, &[])?.into_json().unwrap_or(json!({}));
    Ok(data["items"]
        .as_array()
        .map(|items| items.iter().map(|i| json!({"value": i["value"], "label": i["label"]})).collect())
        .unwrap_or_default())
}

fn require(ctx: &arclet_sdk::Ctx, key: &str, what: &str) -> Result<String> {
    match ctx.input_str(key).trim() {
        "" => ctx.fail(format!("{what} is required")),
        v => Ok(v.to_string()),
    }
}

/// A button that runs a companion command with one query-backed select input.
fn picker_button(id: &'static str, label: &str, icon: &str, key: &'static str, input_label: &str, query: &str, what: &'static str, command: &'static str, param: &'static str) -> Action {
    let mut inputs = vec![input::query_select(key, input_label, query, &[])];
    inputs.extend(base_inputs());
    Action::button(id, label).icon(icon).inputs(inputs).on_press(move |ctx| {
        let value = require(ctx, key, what)?;
        execute_command(command, &ctx.inputs, &[(param, &value)])
    })
}

/// A dynamic wheel listing companion options; pressing a child runs `command`.
fn option_wheel(id: &str, label: &str, icon: &'static str, query: &'static str, empty: &'static str, command: &'static str, param: &'static str) -> Action {
    Action::wheel(id, label)
        .icon(icon)
        .inputs(base_inputs())
        .on_open(move |ctx| {
            let items = fetch_options(query, &ctx.inputs)?;
            if items.is_empty() {
                return ctx.fail(empty);
            }
            Ok(items.into_iter().take(16).map(|i| json!({"id": i["value"], "label": i["label"], "icon": icon})).collect())
        })
        .on_child_press(move |ctx| {
            let child = ctx.child_id.clone().unwrap_or_default();
            execute_command(command, &ctx.inputs, &[(param, &child)])
        })
}

// (id, label, commandId, icon) — the category column of the JS table is not emitted to actions.json.
const SIMPLE: &[(&str, &str, &str, &str)] = &[
    ("command-palette", "Command Palette", "vscode.openCommandPalette", "🧭"),
    ("quick-open", "Quick Open", "vscode.quickOpen", "🔎"),
    ("explorer", "Explorer", "vscode.explorer", "📁"),
    ("search", "Search", "vscode.search", "🔍"),
    ("source-control", "Source Control", "vscode.sourceControl", "🌿"),
    ("extensions", "Extensions", "vscode.extensions", "🧩"),
    ("terminal", "Toggle Terminal", "vscode.toggleTerminal", "⌨"),
    ("problems", "Problems", "vscode.problems", "⚠"),
    ("save-all", "Save All", "vscode.saveAll", "💾"),
    ("format-document", "Format Document", "vscode.formatDocument", "🪄"),
    ("toggle-sidebar", "Toggle Sidebar", "vscode.toggleSidebar", "📚"),
    ("toggle-panel", "Toggle Panel", "vscode.togglePanel", "🧱"),
    ("zen-mode", "Zen Mode", "vscode.zenMode", "🧘"),
    ("split-editor", "Split Editor", "vscode.splitEditor", "🪟"),
    ("new-terminal", "New Terminal", "vscode.newTerminal", "➕"),
    ("stop-debugging", "Stop Debugging", "vscode.stopDebugging", "⏹"),
];

fn main() {
    load_env();
    let mut plugin = Plugin::new("vscode", "VS Code")
        .version("1.0.0")
        .author("Arclet")
        .description("Control Visual Studio Code through the Arclet VS Code companion extension.")
        .icon("icon:thesvg/visual-studio-code")
        .ack_timeout_ms(5000)
        .setup("vscode-setup", Some("setup.html"));

    for &(id, label, command, icon) in SIMPLE {
        plugin = plugin.action(
            Action::button(id, label).icon(icon).inputs(base_inputs()).on_press(move |ctx| execute_command(command, &ctx.inputs, &[])),
        );
    }

    plugin
        .action(picker_button("run-task", "Run Task", "▶", "taskName", "Task", "vscode-get-tasks", "Task", "vscode.runTask", "name"))
        .action(picker_button("start-debugging", "Start Debugging", "🐞", "configName", "Launch Configuration", "vscode-get-launch-configs", "Launch configuration", "vscode.startDebugging", "name"))
        .action(picker_button("focus-terminal", "Focus Terminal", "📟", "terminalName", "Terminal", "vscode-get-terminals", "Terminal", "vscode.focusTerminal", "name"))
        .action(picker_button("open-editor", "Open Editor", "📄", "editorUri", "Open Editor", "vscode-get-open-editors", "Editor", "vscode.openEditor", "uri"))
        .action({
            let mut inputs = vec![input::text("commandId", "Command ID").placeholder("workbench.action.tasks.runTask")];
            inputs.extend(base_inputs());
            Action::button("custom-command", "Custom Command").icon("✨").inputs(inputs).on_press(|ctx| {
                let command = require(ctx, "commandId", "Command ID")?;
                execute_command(&command, &ctx.inputs, &[])
            })
        })
        .action(option_wheel("task-wheel", "Task Wheel", "▶", "tasks", "No VS Code tasks are available.", "vscode.runTask", "name"))
        .action(option_wheel("debug-wheel", "Debug Wheel", "🐞", "launch-configs", "No VS Code launch configurations are available.", "vscode.startDebugging", "name"))
        .action(option_wheel("open-editor-wheel", "Open Editor Wheel", "📄", "open-editors", "No open editors are available.", "vscode.openEditor", "uri"))
        .query("vscode-get-tasks", |ctx| Ok(Value::Array(fetch_options("tasks", &ctx.inputs)?)))
        .query("vscode-get-launch-configs", |ctx| Ok(Value::Array(fetch_options("launch-configs", &ctx.inputs)?)))
        .query("vscode-get-terminals", |ctx| Ok(Value::Array(fetch_options("terminals", &ctx.inputs)?)))
        .query("vscode-get-open-editors", |ctx| Ok(Value::Array(fetch_options("open-editors", &ctx.inputs)?)))
        .run()
}
