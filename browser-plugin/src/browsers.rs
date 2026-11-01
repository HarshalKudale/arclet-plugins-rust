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
#[cfg(windows)]
fn install_dirs(parts: &[&str]) -> Vec<PathBuf> {
    vec![
        join(env("ProgramFiles", "C:\\Program Files"), parts),
        join(env("ProgramFiles(x86)", "C:\\Program Files (x86)"), parts),
        join(std::env::var("LOCALAPPDATA").unwrap_or_default().into(), parts),
    ]
}

/// Per-user application data root: `%LOCALAPPDATA%` (Chromium) on Windows,
/// `~/Library/Application Support` on macOS.
fn local_data(parts: &[&str]) -> Option<PathBuf> {
    #[cfg(windows)]
    let base: PathBuf = std::env::var("LOCALAPPDATA").unwrap_or_default().into();
    #[cfg(not(windows))]
    let base = mac_app_support();
    Some(join(base, parts))
}

#[cfg(not(windows))]
fn home() -> PathBuf {
    env("HOME", "/")
}

#[cfg(not(windows))]
fn mac_app_support() -> PathBuf {
    home().join("Library").join("Application Support")
}

/// `.app` bundles in `/Applications` and `~/Applications`, for each bundle name.
#[cfg(not(windows))]
fn app_bundles(names: &[&str]) -> Vec<PathBuf> {
    names
        .iter()
        .flat_map(|n| [PathBuf::from("/Applications").join(n), home().join("Applications").join(n)])
        .collect()
}

/// Shortcuts that differ per browser family and OS.
fn private_shortcut(family: Family) -> &'static str {
    match (family, cfg!(windows)) {
        (Family::Chromium, true) => "ctrl shift n",
        (Family::Chromium, false) => "cmd shift n",
        (Family::Firefox, true) => "ctrl shift p",
        (Family::Firefox, false) => "cmd shift p",
    }
}

const HARD_RELOAD: &str = if cfg!(windows) { "ctrl f5" } else { "cmd shift r" };

pub fn meta(key: &str) -> Meta {
    let chromium = |label, matches, exe: Vec<PathBuf>, data: &[&str]| {
        let mut bookmarks = data.to_vec();
        bookmarks.push("Bookmarks");
        let mut history = data.to_vec();
        history.push("History");
        Meta {
            label,
            family: Family::Chromium,
            matches,
            exe_candidates: exe,
            bookmarks_path: local_data(&bookmarks),
            history_path: local_data(&history),
            private_shortcut: private_shortcut(Family::Chromium),
            hard_reload_shortcut: HARD_RELOAD,
        }
    };
    let firefox = |label, matches, exe: Vec<PathBuf>, profile_root: &[&str]| Meta {
        label,
        family: Family::Firefox,
        matches,
        exe_candidates: exe,
        bookmarks_path: resolve_firefox_places(profile_root),
        history_path: resolve_firefox_places(profile_root),
        private_shortcut: private_shortcut(Family::Firefox),
        hard_reload_shortcut: HARD_RELOAD,
    };
    #[cfg(windows)]
    return match key {
        "msedge" => chromium("Microsoft Edge", &["edge", "microsoft edge", "msedge"], install_dirs(&["Microsoft", "Edge", "Application", "msedge.exe"]), &["Microsoft", "Edge", "User Data", "Default"]),
        "brave" => chromium("Brave", &["brave", "brave browser"], install_dirs(&["BraveSoftware", "Brave-Browser", "Application", "brave.exe"]), &["BraveSoftware", "Brave-Browser", "User Data", "Default"]),
        "firefox" => firefox("Firefox", &["firefox", "mozilla firefox"], install_dirs(&["Mozilla Firefox", "firefox.exe"]), &["Mozilla", "Firefox"]),
        "zen" => {
            let mut exe = install_dirs(&["Zen Browser", "zen.exe"]);
            exe.push(join(std::env::var("LOCALAPPDATA").unwrap_or_default().into(), &["Programs", "Zen Browser", "zen.exe"]));
            firefox("Zen", &["zen", "zen browser"], exe, &["zen"])
        }
        // Unknown keys fall back to Chrome, like the JS plugin.
        _ => chromium("Chrome", &["chrome", "google chrome"], install_dirs(&["Google", "Chrome", "Application", "chrome.exe"]), &["Google", "Chrome", "User Data", "Default"]),
    };
    // macOS: `.app` bundles; profiles under ~/Library/Application Support.
    #[cfg(not(windows))]
    return match key {
        "msedge" => chromium("Microsoft Edge", &["edge", "microsoft edge", "msedge", "com.microsoft.edgemac"], app_bundles(&["Microsoft Edge.app"]), &["Microsoft Edge", "Default"]),
        "brave" => chromium("Brave", &["brave", "brave browser", "com.brave.browser"], app_bundles(&["Brave Browser.app"]), &["BraveSoftware", "Brave-Browser", "Default"]),
        "firefox" => firefox("Firefox", &["firefox", "mozilla firefox", "org.mozilla.firefox"], app_bundles(&["Firefox.app"]), &["Firefox"]),
        "zen" => firefox("Zen", &["zen", "zen browser", "app.zen-browser.zen"], app_bundles(&["Zen.app", "Zen Browser.app"]), &["zen"]),
        _ => chromium("Chrome", &["chrome", "google chrome", "com.google.chrome"], app_bundles(&["Google Chrome.app"]), &["Google", "Chrome", "Default"]),
    };
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

/// Installed-executable path (a `.exe` on Windows, a `.app` bundle on macOS) for a
/// browser key or an explicit path / shell:AppsFolder id.
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

/// `profiles.ini` lives in `%APPDATA%\\<root>` on Windows and
/// `~/Library/Application Support/<root>` on macOS.
fn resolve_firefox_places(profile_root: &[&str]) -> Option<PathBuf> {
    #[cfg(windows)]
    let base = env("APPDATA", "");
    #[cfg(not(windows))]
    let base = mac_app_support();
    let root = join(base, profile_root);
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

#[cfg(test)]
mod tests {
    use super::*;
    use arclet_sdk::json;

    #[test]
    fn detects_installed_app_entries() {
        #[cfg(not(windows))]
        let (chrome, edge) = ("/Applications/Google Chrome.app", "/Applications/Microsoft Edge.app");
        #[cfg(windows)]
        let (chrome, edge) = ("C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe", "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe");
        assert_eq!(detect_key(&json!({"label": "Google Chrome", "value": chrome})), Some("chrome"));
        assert_eq!(detect_key(&json!({"label": "Microsoft Edge", "value": edge})), Some("msedge"));
        assert_eq!(detect_key(&json!({"label": "Firefox", "value": "x"})), Some("firefox"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_locations() {
        let chrome = meta("chrome");
        assert!(chrome.exe_candidates.contains(&PathBuf::from("/Applications/Google Chrome.app")));
        assert!(chrome.history_path.unwrap().ends_with("Library/Application Support/Google/Chrome/Default/History"));
        assert_eq!(meta("firefox").private_shortcut, "cmd shift p");
    }
}
