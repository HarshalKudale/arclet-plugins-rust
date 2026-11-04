//! ai-stats — plan limits and local token usage for AI coding tools (Claude Code, Codex).
//!
//! Limits come from each provider's usage endpoint, authenticated with the login its CLI
//! already stored; usage is counted from local transcripts. It never calls a model and never
//! writes to the CLIs' files.

mod limits;
mod usage;

use base64::Engine;
use knobdeck_sdk::input::{self, InputExt};
use knobdeck_sdk::{Action, Plugin, Value, Visual};
use limits::Provider;
use std::path::PathBuf;
use std::time::Duration;

/// Tiles re-read the cache this often, so a changed input shows within seconds; the network
/// is hit only once per `limits::TTL`.
const LIVE: Duration = Duration::from_secs(5);

const CLAUDE_COLOR: &str = "#D97757";
const CODEX_COLOR: &str = "#6C7BFF";

fn svg_uri(svg: String) -> String {
    format!("data:image/svg+xml;base64,{}", base64::engine::general_purpose::STANDARD.encode(svg))
}

/// Ring gauge (win-tools System Stats style) in the provider's colour; red once nearly used up.
fn gauge(pct: f64, color: &str) -> String {
    let p = pct.clamp(0.0, 100.0).round();
    let color = if p >= 90.0 { "#ef4444" } else { color };
    let circ = 2.0 * std::f64::consts::PI * 40.0;
    svg_uri(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><circle cx="50" cy="50" r="40" fill="none" stroke="{color}" stroke-opacity="0.25" stroke-width="10"/><circle cx="50" cy="50" r="40" fill="none" stroke="{color}" stroke-width="10" stroke-linecap="round" stroke-dasharray="{:.1} {circ:.1}" transform="rotate(-90 50 50)"/><text x="50" y="61" text-anchor="middle" font-family="Segoe UI, sans-serif" font-size="30" font-weight="600" fill="#888888">{p:.0}</text></svg>"##,
        circ * p / 100.0
    ))
}

/// The token count on a provider-coloured ring.
fn count_icon(text: &str, color: &str) -> String {
    svg_uri(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><circle cx="50" cy="50" r="40" fill="none" stroke="{color}" stroke-width="10"/><text x="50" y="60" text-anchor="middle" font-family="Segoe UI, sans-serif" font-size="26" font-weight="600" fill="#888888">{text}</text></svg>"##
    ))
}

/// `~/x` → `<home>/x`.
fn expand(path: &str) -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
    match path.trim().strip_prefix("~") {
        Some(rest) => home.join(rest.trim_start_matches(['/', '\\'])),
        None => PathBuf::from(path.trim()),
    }
}

struct Spec {
    provider: Provider,
    name: &'static str,
    color: &'static str,
    creds: &'static str,
    dir: &'static str,
    windows: &'static [(&'static str, &'static str)],
    pick: fn(&Value, &str) -> Option<limits::Window>,
}

const CLAUDE: Spec = Spec {
    provider: Provider::Claude,
    name: "Claude",
    color: CLAUDE_COLOR,
    creds: "~/.claude/.credentials.json",
    dir: "~/.claude",
    windows: &[("5h", "5-hour"), ("7d", "7-day (weekly)"), ("fable", "Fable 7-day")],
    pick: limits::claude_window,
};

const CODEX: Spec = Spec {
    provider: Provider::Codex,
    name: "Codex",
    color: CODEX_COLOR,
    creds: "~/.codex/auth.json",
    dir: "~/.codex",
    windows: &[("primary", "Primary (5-hour)"), ("secondary", "Secondary (weekly)")],
    pick: limits::codex_window,
};

fn creds_of(spec: &Spec, given: String) -> String {
    expand(if given.trim().is_empty() { spec.creds } else { &given }).to_string_lossy().into_owned()
}

fn limits_visual(spec: &Spec, creds: &str, window: &str) -> Visual {
    let window = if window.is_empty() { spec.windows[0].0 } else { window };
    match limits::fetch(spec.provider, creds).map(|v| (spec.pick)(&v, window)) {
        Ok(Some(w)) => {
            let reset = w.resets_at.map(|t| format!(" · {}", limits::left(t))).unwrap_or_default();
            Visual::new().label(format!("{} {} {:.0}%{reset}", spec.name, w.name, w.used)).icon(gauge(w.used, spec.color))
        }
        Ok(None) => Visual::new().label(format!("{} · not on this plan", spec.name)).icon("circle-off"),
        Err(e) => Visual::new().label(format!("{} · {e}", spec.name)).icon("circle-alert"),
    }
}

fn limits_action(id: &str, label: &str, spec: &'static Spec) -> Action {
    Action::button(id, label)
        .icon("gauge")
        .inputs(vec![
            input::text("credentials", "Credentials path").with("default", spec.creds).with("placeholder", spec.creds),
            input::select("window", "Usage type", spec.windows).with("default", spec.windows[0].0),
        ])
        .live(LIVE, move |ic| Ok(limits_visual(spec, &creds_of(spec, ic.input_str("credentials")), &ic.input_str("window"))))
        // Press: refetch now.
        .on_press(move |ctx| {
            let creds = creds_of(spec, ctx.input_str("credentials"));
            limits::invalidate(spec.provider, &creds);
            ctx.set_visual(limits_visual(spec, &creds, &ctx.input_str("window")));
            Ok(())
        })
}

fn usage_visual(spec: &Spec, dir: &str) -> Visual {
    let dir = expand(if dir.trim().is_empty() { spec.dir } else { dir });
    if !dir.is_dir() {
        return Visual::new().label(format!("{} · folder not found", spec.name)).icon("circle-alert");
    }
    let t = usage::compact(usage::tokens(spec.provider, &dir));
    Visual::new().label(format!("{} · {t} tokens · 30d", spec.name)).icon(count_icon(&t, spec.color))
}

fn usage_action(id: &str, label: &str, spec: &'static Spec) -> Action {
    Action::button(id, label)
        .icon("sigma")
        .inputs(vec![input::text("folder", "Data folder").with("default", spec.dir).with("placeholder", spec.dir)])
        .live(Duration::from_secs(15), move |ic| Ok(usage_visual(spec, &ic.input_str("folder"))))
        .on_press(move |ctx| {
            ctx.set_visual(usage_visual(spec, &ctx.input_str("folder")));
            Ok(())
        })
}

fn main() {
    Plugin::new(&format!("ai-stats-{}", std::env::consts::OS), "AI Stats")
        .version(env!("CARGO_PKG_VERSION"))
        .author("KnobDeck")
        .description("Plan limits (5-hour, weekly) and 30-day token usage for Claude Code and Codex, from the logins and transcripts already on this computer.")
        .icon("gauge")
        .platforms(&[std::env::consts::OS])
        .action(limits_action("claude-limits", "Claude Limits", &CLAUDE))
        .action(limits_action("codex-limits", "Codex Limits", &CODEX))
        .action(usage_action("claude-usage", "Claude Usage", &CLAUDE))
        .action(usage_action("codex-usage", "Codex Usage", &CODEX))
        .run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_home() {
        assert!(expand("~/.codex/auth.json").ends_with(".codex/auth.json"));
        assert!(!expand("~/.codex").starts_with("~"));
        assert_eq!(expand(" /abs/x "), PathBuf::from("/abs/x"));
    }
}
