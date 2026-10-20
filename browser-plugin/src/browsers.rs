//! Browser catalogue: install locations, data files, and per-browser shortcuts.

use arclet_sdk::{Error, Result, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Chromium,
    Firefox,
}

#[derive(Clone)]
pub struct Meta {
    pub label: &'static str,
    pub family: Family,
    pub matches: &'static [&'static str],
    pub exe_candidates: Vec<PathBuf>,
    pub bookmarks_path: Option<PathBuf>,
    pub history_path: Option<PathBuf>,
    pub private_shortcut: &'static str,
    pub hard_reload_shortcut: &'static str,
}

/// Detection order matters (first match wins).
pub const KEYS: [&str; 5] = ["chrome", "msedge", "brave", "firefox", "zen"];

fn env(key: &str, default: &str) -> PathBuf {
    PathBuf::from(std::env::var(key).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.into()))
}

fn join(base: PathBuf, parts: &[&str]) -> PathBuf {
    parts.iter().fold(base, |p, s| p.join(s))
}

/// `[%ProgramFiles%, %ProgramFiles(x86)%, %LOCALAPPDATA%] / parts`
fn install_dirs(parts: &[&str]) -> Vec<PathBuf> {
    vec![
        join(env("ProgramFiles", "C:\\Program Files"), parts),
        join(env("ProgramFiles(x86)", "C:\\Program Files (x86)"), parts),
        join(std::env::var("LOCALAPPDATA").unwrap_or_default().into(), parts),
    ]
}

fn local_data(parts: &[&str]) -> Option<PathBuf> {
    Some(join(std::env::var("LOCALAPPDATA").unwrap_or_default().into(), parts))
}

pub fn meta(key: &str) -> Meta {
    let chromium = |label, matches, exe: &[&str], data: &[&str]| {
        let mut bookmarks = data.to_vec();
        bookmarks.push("Bookmarks");
        let mut history = data.to_vec();
        history.push("History");
        Meta {

            label,
            family: Family::Chromium,
            matches,
            exe_candidates: install_dirs(exe),
            bookmarks_path: local_data(&bookmarks),
            history_path: local_data(&history),
            private_shortcut: "ctrl shift n",
            hard_reload_shortcut: "ctrl f5",
        }
    };
    let firefox = |label, matches, exe: Vec<PathBuf>, profile_root: &str| Meta {

        label,
        family: Family::Firefox,
        matches,
        exe_candidates: exe,
        bookmarks_path: resolve_firefox_places(profile_root),
        history_path: resolve_firefox_places(profile_root),
        private_shortcut: "ctrl shift p",
        hard_reload_shortcut: "ctrl f5",
    };
    match key {
        "msedge" => chromium("Microsoft Edge", &["edge", "microsoft edge", "msedge"], &["Microsoft", "Edge", "Application", "msedge.exe"], &["Microsoft", "Edge", "User Data", "Default"]),
        "brave" => chromium("Brave", &["brave", "brave browser"], &["BraveSoftware", "Brave-Browser", "Application", "brave.exe"], &["BraveSoftware", "Brave-Browser", "User Data", "Default"]),
        "firefox" => firefox("Firefox", &["firefox", "mozilla firefox"], install_dirs(&["Mozilla Firefox", "firefox.exe"]), "Mozilla\\Firefox"),
        "zen" => {
            let mut exe = install_dirs(&["Zen Browser", "zen.exe"]);
            exe.push(join(std::env::var("LOCALAPPDATA").unwrap_or_default().into(), &["Programs", "Zen Browser", "zen.exe"]));
            firefox("Zen", &["zen", "zen browser"], exe, "zen")
        }
        // Unknown keys fall back to Chrome, like the JS plugin.
        _ => chromium("Chrome", &["chrome", "google chrome"], &["Google", "Chrome", "Application", "chrome.exe"], &["Google", "Chrome", "User Data", "Default"]),
    }
}

/// Selected combobox value: `{ value, label }` or a bare string.
pub fn selection(input: &Value) -> (String, String) {
    match input {
        Value::Object(o) => {
            let value = o.get("value").and_then(Value::as_str).unwrap_or("").to_string();
            let label = o.get("label").and_then(Value::as_str).map(String::from).unwrap_or_else(|| value.clone());
            (label, value)
        }
        Value::String(s) => (s.clone(), s.clone()),
        _ => (String::new(), String::new()),
    }
}

pub fn detect_key(input: &Value) -> Option<&'static str> {
    let (label, value) = selection(input);
    let haystack = format!("{label} {value}").to_lowercase();
    KEYS.into_iter().find(|&key| {
        let m = meta(key);
        m.exe_candidates.iter().any(|c| c.to_string_lossy().to_lowercase() == value.to_lowercase())
            || m.matches.iter().any(|term| haystack.contains(term))
    })
}

/// Installed-executable path for a browser key or an explicit exe path / shell:AppsFolder id.
pub fn exe_path(browser: &str) -> Option<String> {
    if !browser.is_empty() && (Path::new(browser).exists() || browser.to_lowercase().starts_with("shell:appsfolder\\")) {
        return Some(browser.into());
    }
    meta(browser).exe_candidates.into_iter().find(|c| c.exists()).map(|c| c.to_string_lossy().into_owned())
}

pub fn ensure_executable(browser: &str) -> Result<String> {
    exe_path(browser).ok_or_else(|| {
        let label = if KEYS.contains(&browser) { meta(browser).label } else { detect_key(&Value::String(browser.into())).map_or("Browser", |k| meta(k).label) };
        Error::msg(format!("{label} is not installed"))
    })
}

fn resolve_firefox_places(profile_root: &str) -> Option<PathBuf> {
    let appdata = env("APPDATA", "");
    let root = profile_root.split('\\').fold(appdata, |p, s| p.join(s));
    let ini = std::fs::read_to_string(root.join("profiles.ini")).ok()?;
    let mut sections: Vec<Vec<&str>> = vec![];
    for line in ini.lines() {
        if line.starts_with('[') || sections.is_empty() {
            sections.push(vec![]);
        }
        sections.last_mut().unwrap().push(line);
    }
    for section in sections {
        let lower = section.join("\n").to_lowercase();
        if !(lower.contains("default=1") || lower.contains("default=true")) {
            continue;
        }
        let get = |k: &str| section.iter().find_map(|l| l.strip_prefix(k)).map(str::trim);
        let Some(raw) = get("Path=") else { continue };
        let relative = get("IsRelative=").map_or(true, |v| v != "0");
        let dir = if relative { root.join(raw) } else { PathBuf::from(raw) };
        let places = dir.join("places.sqlite");
        if places.exists() {
            return Some(places);
        }
    }
    None
}
