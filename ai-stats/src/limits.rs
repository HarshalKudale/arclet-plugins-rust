//! Plan limits (used % of the 5 h / weekly windows) from each provider's own usage endpoint,
//! authenticated with the token its CLI already stored locally. Read-only: tokens are never
//! refreshed or written back (that would rotate the CLI's refresh token and log it out), so
//! an expired token shows "sign in" until Claude Code / Codex next refreshes it.

use crate::usage::{now_ms, parse_iso_ms};
use knobdeck_sdk::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Both endpoints rate-limit; the original Stream Deck plugin polls every 120 s too.
const TTL: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub used: f64,
    /// Unix ms.
    pub resets_at: Option<u64>,
    /// "5h", "7d", "30d", "Fable"…
    pub name: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Provider {
    Claude,
    Codex,
}

static CACHE: Mutex<Option<HashMap<(Provider, String), (Instant, Result<Value, String>)>>> = Mutex::new(None);

/// Drop the cached response so the next read refetches (a press on the tile).
pub fn invalidate(p: Provider, creds: &str) {
    if let Some(m) = CACHE.lock().unwrap().as_mut() {
        m.remove(&(p, creds.to_string()));
    }
}

/// The raw usage JSON, fetched at most once per [`TTL`] per credentials file.
pub fn fetch(p: Provider, creds: &str) -> Result<Value, String> {
    let key = (p, creds.to_string());
    if let Some((at, r)) = CACHE.lock().unwrap().as_ref().and_then(|m| m.get(&key)) {
        if at.elapsed() < TTL {
            return r.clone();
        }
    }
    let r = match p {
        Provider::Claude => claude_fetch(creds),
        Provider::Codex => codex_fetch(creds),
    };
    CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, (Instant::now(), r.clone()));
    r
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build()
}

fn get(req: ureq::Request, who: &str) -> Result<Value, String> {
    match req.call() {
        Ok(r) => r.into_json().map_err(|_| format!("{who}: bad response")),
        Err(ureq::Error::Status(401 | 403, _)) => Err("sign in".into()),
        Err(ureq::Error::Status(429, _)) => Err("rate limited".into()),
        Err(ureq::Error::Status(c, _)) => Err(format!("HTTP {c}")),
        Err(_) => Err("offline".into()),
    }
}

fn read_json(path: &Path) -> Result<Value, String> {
    let s = std::fs::read_to_string(path).map_err(|_| "no login found".to_string())?;
    s.parse().map_err(|_| "bad credentials file".into())
}

fn claude_token(path: &Path) -> Result<String, String> {
    let v = match read_json(path) {
        Ok(v) => v,
        // macOS Claude Code keeps the login in the Keychain, not in a file.
        #[cfg(target_os = "macos")]
        Err(_) => std::process::Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok()?.trim().parse().ok())
            .ok_or("no login found")?,
        #[cfg(not(target_os = "macos"))]
        Err(e) => return Err(e),
    };
    v["claudeAiOauth"]["accessToken"].as_str().map(str::to_string).ok_or_else(|| "no login found".into())
}

fn claude_fetch(creds: &str) -> Result<Value, String> {
    let token = claude_token(Path::new(creds))?;
    get(
        agent()
            .get("https://api.anthropic.com/api/oauth/usage")
            .set("Authorization", &format!("Bearer {token}"))
            .set("anthropic-version", "2023-06-01")
            .set("anthropic-beta", "oauth-2025-04-20"),
        "Claude",
    )
}

fn codex_fetch(creds: &str) -> Result<Value, String> {
    let v = read_json(Path::new(creds))?;
    let t = &v["tokens"];
    let token = t["access_token"].as_str().ok_or("no login found")?;
    let mut req = agent()
        .get("https://chatgpt.com/backend-api/codex/usage")
        .set("Authorization", &format!("Bearer {token}"))
        .set("User-Agent", "codex_cli_rs")
        .set("Accept", "application/json");
    if let Some(id) = t["account_id"].as_str() {
        req = req.set("chatgpt-account-id", id);
    }
    get(req, "Codex")
}

/// `window`: "5h" | "7d" | "fable".
pub fn claude_window(v: &Value, window: &str) -> Option<Window> {
    let (w, name) = match window {
        "fable" => {
            // The model-scoped weekly limit, listed in `limits` (kind weekly_scoped).
            let l = v["limits"].as_array()?.iter().find(|l| {
                let m = &l["scope"]["model"];
                l["kind"] == "weekly_scoped"
                    && [&m["display_name"], &m["id"]].iter().any(|s| s.as_str().is_some_and(|s| s.to_lowercase().contains("fable")))
            })?;
            return Some(Window { used: l["percent"].as_f64()?, resets_at: l["resets_at"].as_str().and_then(parse_iso_ms), name: "Fable".into() });
        }
        "7d" => (&v["seven_day"], "7d"),
        _ => (&v["five_hour"], "5h"),
    };
    Some(Window { used: w["utilization"].as_f64()?, resets_at: w["resets_at"].as_str().and_then(parse_iso_ms), name: name.into() })
}

/// `window`: "primary" (5 h on Plus/Pro) | "secondary" (weekly). Named from the window length,
/// since plans differ (Go has a single 30-day primary window).
pub fn codex_window(v: &Value, window: &str) -> Option<Window> {
    let w = &v["rate_limit"][if window == "secondary" { "secondary_window" } else { "primary_window" }];
    let secs = w["limit_window_seconds"].as_u64().unwrap_or(0);
    let name = if secs >= 86400 { format!("{}d", secs / 86400) } else { format!("{}h", (secs / 3600).max(1)) };
    Some(Window { used: w["used_percent"].as_f64()?, resets_at: w["reset_at"].as_u64().map(|s| s * 1000), name })
}

/// Time until `at` (unix ms): "2d 3h", "1h 48m", "12m".
pub fn left(at: u64) -> String {
    let m = at.saturating_sub(now_ms()) / 60_000;
    let (d, h, m) = (m / 1440, m % 1440 / 60, m % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_windows() {
        let v: Value = r#"{"five_hour":{"utilization":18.0,"resets_at":"1970-01-01T00:00:01.5+00:00"},"seven_day":{"utilization":31.0,"resets_at":null},
            "limits":[{"kind":"weekly_all","percent":31},{"kind":"weekly_scoped","percent":7,"resets_at":"1970-01-01T00:00:02Z","scope":{"model":{"display_name":"Fable"}}}]}"#
            .parse()
            .unwrap();
        assert_eq!(claude_window(&v, "5h"), Some(Window { used: 18.0, resets_at: Some(1500), name: "5h".into() }));
        assert_eq!(claude_window(&v, "7d").unwrap().used, 31.0);
        assert_eq!(claude_window(&v, "fable"), Some(Window { used: 7.0, resets_at: Some(2000), name: "Fable".into() }));
        assert_eq!(claude_window(&"{}".parse().unwrap(), "fable"), None);
    }

    #[test]
    fn codex_windows() {
        let v: Value = r#"{"rate_limit":{"primary_window":{"used_percent":2,"limit_window_seconds":2592000,"reset_at":10},"secondary_window":null}}"#.parse().unwrap();
        assert_eq!(codex_window(&v, "primary"), Some(Window { used: 2.0, resets_at: Some(10_000), name: "30d".into() }));
        assert_eq!(codex_window(&v, "secondary"), None);
    }
}
