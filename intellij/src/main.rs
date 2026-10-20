//! IntelliJ plugin for Arclet — runs IDE actions through the Arclet IntelliJ
//! companion plugin's local action service.

use arclet_sdk::input::{self, InputExt};
use arclet_sdk::{env_or, load_env, Action, Error, Plugin, Result, Value};
use std::time::Duration;

fn default_port() -> String {
    env_or("IDEA_PORT", "21420")
}

fn base_inputs() -> Vec<Value> {
    vec![input::text("port", "Action Port (Optional)").placeholder(&format!("Default {}", default_port()))]
}

fn run_config_input() -> Value {
    input::text("runConfig", "Run / Debug Configuration Name (Optional)").placeholder("My App")
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

fn execute_idea_action(action_id: &str, inputs: &Value) -> Result {
    let port = match inputs["port"].as_str().map(str::trim) {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => default_port(),
    };
    let mut req = ureq::get(&format!("http://{}:{port}/api/action/{}", env_or("IDEA_HOST", "127.0.0.1"), enc(action_id)))
        .timeout(Duration::from_secs(10));
    if let Some(name) = inputs["runConfig"].as_str().map(str::trim).filter(|n| !n.is_empty()) {
        req = req.query("name", name);
    }
    let password = env_or("IDEA_PASSWORD", "");
    if !password.is_empty() {
        req = req.set("Authorization", &password);
    }
    match req.call() {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(403 | 500, _)) => Err(Error::msg("IntelliJ companion plugin rejected the configured password.")),
        Err(ureq::Error::Status(code, _)) => Err(Error::msg(format!("IntelliJ companion plugin returned HTTP {code}."))),
        Err(e) => Err(Error::msg(format!("IntelliJ companion plugin is not reachable on port {port}. ({e})"))),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Plain,
    Custom,
    RunConfig,
}

// (id, label, IDE actionId, icon, kind)
const ACTIONS: &[(&str, &str, &str, &str, Kind)] = &[
    ("open-action-browser", "Open Action Browser", "streamdeck.show.action.browser", "🧭", Kind::Plain),
    ("custom-action", "Customized Action", "", "✨", Kind::Custom),
    ("update-project", "Update Project", "Vcs.UpdateProject", "⬇", Kind::Plain),
    ("new-project", "New Project", "NewProject", "🆕", Kind::Plain),
    ("run", "Run", "Run", "▶", Kind::RunConfig),
    ("debug", "Debug", "Debug", "🐞", Kind::RunConfig),
    ("step-over", "Step Over", "StepOver", "⤼", Kind::Plain),
    ("step-into", "Step Into", "StepInto", "↘", Kind::Plain),
    ("step-out", "Step Out", "StepOut", "↗", Kind::Plain),
    ("resume", "Resume", "Resume", "⏵", Kind::Plain),
    ("pause", "Pause", "Pause", "⏸", Kind::Plain),
    ("search-everywhere", "Search Everywhere", "SearchEverywhere", "🔎", Kind::Plain),
    ("stop", "Stop", "Stop", "⏹", Kind::Plain),
    ("project-structure", "Project Structure", "ShowProjectStructureSettings", "🧱", Kind::Plain),
    ("reset-frame", "Reset Frame", "Debugger.PopFrame", "↺", Kind::Plain),
    ("terminal", "Terminal", "ActivateTerminalToolWindow", "⌨", Kind::Plain),
    ("claude-code", "Claude Code", "com.anthropic.code.plugin.actions.OpenClaudeInTerminalAction", "🤖", Kind::Plain),
];

fn main() {
    load_env();
    let mut plugin = Plugin::new("intellij", "IntelliJ IDE")
        .version("1.0.0")
        .author("Arclet")
        .description("Control IntelliJ IDEs through the Arclet IntelliJ companion plugin.")
        .icon("icon:thesvg/intellij-idea")
        .ack_timeout_ms(5000)
        .setup("intellij-setup", Some("setup.html"));

    for &(id, label, idea_action, icon, kind) in ACTIONS {
        let inputs = match kind {
            Kind::Custom => [vec![input::text("actionId", "Action ID").placeholder("SearchEverywhere"), run_config_input()], base_inputs()].concat(),
            Kind::RunConfig => [vec![run_config_input()], base_inputs()].concat(),
            Kind::Plain => base_inputs(),
        };
        plugin = plugin.action(Action::button(id, label).icon(icon).inputs(inputs).on_press(move |ctx| {
            let action_id = if kind == Kind::Custom { ctx.input_str("actionId").trim().to_string() } else { idea_action.to_string() };
            if action_id.is_empty() {
                return ctx.fail("Action ID is required");
            }
            execute_idea_action(&action_id, &ctx.inputs)
        }));
    }
    plugin.run()
}
