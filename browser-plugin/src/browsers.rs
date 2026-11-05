//! Browser catalogue: install locations, data files, and per-browser shortcuts.

use knobdeck_sdk::{Error, Result, Value};
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
#[cfg(not(target_os = "linux"))]
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

#[cfg(target_os = "macos")]
fn mac_app_support() -> PathBuf {
    home().join("Library").join("Application Support")
}

/// Linux: `$XDG_CONFIG_HOME` (`~/.config`), where Chromium browsers and current Firefox/Zen keep profiles.
#[cfg(target_os = "linux")]
fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".config"))
}

/// Linux: Chromium profile data in `~/.config/<parts>`, or inside the browser's Flatpak.
#[cfg(target_os = "linux")]
fn linux_profile(flatpak_id: &str, parts: &[&str]) -> Vec<PathBuf> {
    vec![join(config_home(), parts), join(home().join(".var/app").join(flatpak_id).join("config"), parts)]
}

/// Linux executables: distro packages on PATH (Debian/Ubuntu `.deb`s and Arch packages put a
/// launcher in /usr/bin), the vendors' /opt installs, and Flatpak / Snap exports.
#[cfg(target_os = "linux")]
fn linux_exes(bins: &[&str], extra: &[PathBuf], flatpak_id: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = bins.iter().flat_map(|b| [PathBuf::from("/usr/bin").join(b), PathBuf::from("/usr/local/bin").join(b), PathBuf::from("/snap/bin").join(b)]).collect();
    v.extend(extra.iter().cloned());
    v.push(home().join(".local/share/flatpak/exports/bin").join(flatpak_id));
    v.push(PathBuf::from("/var/lib/flatpak/exports/bin").join(flatpak_id));
    v
}

/// `.app` bundles in `/Applications` and `~/Applications`, for each bundle name.
#[cfg(target_os = "macos")]
fn app_bundles(names: &[&str]) -> Vec<PathBuf> {
    names
        .iter()
        .flat_map(|n| [PathBuf::from("/Applications").join(n), home().join("Applications").join(n)])
        .collect()
}

/// Shortcuts that differ per browser family and OS.
fn private_shortcut(family: Family) -> &'static str {
    match (family, cfg!(target_os = "macos")) {
        (Family::Chromium, false) => "ctrl shift n",
        (Family::Chromium, true) => "cmd shift n",
        (Family::Firefox, false) => "ctrl shift p",
        (Family::Firefox, true) => "cmd shift p",
    }
}

const HARD_RELOAD: &str = if cfg!(target_os = "macos") { "cmd shift r" } else { "ctrl f5" };

pub fn meta(key: &str) -> Meta {
    // `profile` lists the profile folders to look in (one on Windows and macOS).
    let chromium_in = |label, matches, exe: Vec<PathBuf>, profile: Vec<PathBuf>| Meta {
        label,
        family: Family::Chromium,
        matches,
        exe_candidates: exe,
        bookmarks_path: profile.iter().map(|p| p.join("Bookmarks")).find(|p| p.exists()).or_else(|| profile.first().map(|p| p.join("Bookmarks"))),
        history_path: profile.iter().map(|p| p.join("History")).find(|p| p.exists()).or_else(|| profile.first().map(|p| p.join("History"))),
        private_shortcut: private_shortcut(Family::Chromium),
        hard_reload_shortcut: HARD_RELOAD,
    };
    let firefox_in = |label, matches, exe: Vec<PathBuf>, roots: Vec<PathBuf>| {
        let places = roots.iter().find_map(|r| resolve_firefox_places(r));
        Meta {
            label,
            family: Family::Firefox,
            matches,
            exe_candidates: exe,
            bookmarks_path: places.clone(),
            history_path: places,
            private_shortcut: private_shortcut(Family::Firefox),
            hard_reload_shortcut: HARD_RELOAD,
        }
    };
    #[cfg(not(target_os = "linux"))]
    let chromium = |label, matches, exe: Vec<PathBuf>, data: &[&str]| chromium_in(label, matches, exe, local_data(data).into_iter().collect());
    #[cfg(not(target_os = "linux"))]
    let firefox = |label, matches, exe: Vec<PathBuf>, profile_root: &[&str]| firefox_in(label, matches, exe, vec![firefox_root(profile_root)]);
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
    // Linux: distro / vendor / Flatpak / Snap installs; profiles under ~/.config (or the
    // sandbox's home). Window classes double as match terms for the host's app list.
    #[cfg(target_os = "linux")]
    return {
        let h = home();
        match key {
            "msedge" => chromium_in("Microsoft Edge", &["edge", "microsoft edge", "msedge", "microsoft-edge"], linux_exes(&["microsoft-edge-stable", "microsoft-edge"], &["/opt/microsoft/msedge/msedge".into()], "com.microsoft.Edge"), linux_profile("com.microsoft.Edge", &["microsoft-edge", "Default"])),
            "brave" => chromium_in("Brave", &["brave", "brave browser", "brave-browser"], linux_exes(&["brave-browser", "brave-browser-stable", "brave"], &["/opt/brave.com/brave/brave".into(), "/opt/brave-bin/brave".into()], "com.brave.Browser"), linux_profile("com.brave.Browser", &["BraveSoftware", "Brave-Browser", "Default"])),
            "firefox" => firefox_in(
                "Firefox",
                &["firefox", "mozilla firefox", "org.mozilla.firefox"],
                linux_exes(&["firefox"], &["/usr/lib/firefox/firefox".into(), "/opt/firefox/firefox".into()], "org.mozilla.firefox"),
                // Firefox 147+ follows XDG (~/.config/mozilla); older profiles stay in ~/.mozilla.
                vec![config_home().join("mozilla/firefox"), h.join(".mozilla/firefox"), h.join("snap/firefox/common/.mozilla/firefox"), h.join(".var/app/org.mozilla.firefox/config/mozilla/firefox"), h.join(".var/app/org.mozilla.firefox/.mozilla/firefox")],
            ),
            "zen" => firefox_in(
                "Zen",
                &["zen", "zen browser", "zen-browser", "app.zen_browser.zen"],
                linux_exes(&["zen-browser", "zen"], &["/opt/zen-browser-bin/zen-bin".into(), "/opt/zen/zen".into(), h.join(".tarball-installations/zen/zen")], "app.zen_browser.zen"),
                // Zen 1.x keeps profiles in ~/.config/zen; earlier versions used ~/.zen.
                vec![config_home().join("zen"), h.join(".zen"), h.join(".var/app/app.zen_browser.zen/config/zen"), h.join(".var/app/app.zen_browser.zen/.zen")],
            ),
            _ => chromium_in("Chrome", &["chrome", "google chrome", "google-chrome"], linux_exes(&["google-chrome-stable", "google-chrome"], &["/opt/google/chrome/chrome".into()], "com.google.Chrome"), linux_profile("com.google.Chrome", &["google-chrome", "Default"])),
        }
    };
    // macOS: `.app` bundles; profiles under ~/Library/Application Support.
    #[cfg(target_os = "macos")]
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

/// Installed-executable path (a `.exe` on Windows, a `.app` bundle on macOS, a program on Linux)
/// for a browser key or an explicit path / shell:AppsFolder id.
pub fn exe_path(browser: &str) -> Option<String> {
    // Linux app pickers hand over a desktop entry: use the browser's own program when it is
    // found, else the entry itself (launched through `gio launch`).
    if browser.ends_with(".desktop") {
        let key = detect_key(&Value::String(browser.into()))?;
        return meta(key).exe_candidates.into_iter().find(|c| c.exists()).map(|c| c.to_string_lossy().into_owned()).or_else(|| Path::new(browser).exists().then(|| browser.into()));
    }
    if !browser.is_empty() && (Path::new(browser).exists() || browser.to_lowercase().starts_with("shell:appsfolder\\")) {
        return Some(browser.into());
    }
    meta(browser).exe_candidates.into_iter().find(|c| c.exists()).map(|c| c.to_string_lossy().into_owned())
}

/// Window classes / Wayland app ids each browser's windows carry on Linux.
#[cfg(target_os = "linux")]
pub fn window_classes(key: &str) -> &'static [&'static str] {
    match key {
        "msedge" => &["microsoft-edge", "com.microsoft.Edge"],
        "brave" => &["brave-browser", "com.brave.Browser"],
        "firefox" => &["firefox", "org.mozilla.firefox"],
        "zen" => &["zen", "app.zen_browser.zen"],
        _ => &["google-chrome", "com.google.Chrome"],
    }
}

pub fn ensure_executable(browser: &str) -> Result<String> {
    exe_path(browser).ok_or_else(|| {
        let label = if KEYS.contains(&browser) { meta(browser).label } else { detect_key(&Value::String(browser.into())).map_or("Browser", |k| meta(k).label) };
        Error::msg(format!("{label} is not installed"))
    })
}

/// `profiles.ini` lives in `%APPDATA%\\<root>` on Windows and
/// `~/Library/Application Support/<root>` on macOS.
#[cfg(not(target_os = "linux"))]
fn firefox_root(profile_root: &[&str]) -> PathBuf {
    #[cfg(windows)]
    let base = env("APPDATA", "");
    #[cfg(not(windows))]
    let base = mac_app_support();
    join(base, profile_root)
}

/// The `places.sqlite` of the profile in use under a profiles root: of every profile
/// `profiles.ini` lists and every profile folder in the root, the one written to most recently.
/// `profiles.ini` alone isn't enough: Firefox's and Zen's selectable profiles ("Profile Groups")
/// are not listed there, and `Default=` names the profile a fresh start opens, not the one the
/// user is in. Ties go to the install default, then `Default=1`, then the listing order.
fn resolve_firefox_places(root: &Path) -> Option<PathBuf> {
    let ini = std::fs::read_to_string(root.join("profiles.ini")).unwrap_or_default();
    let mut sections: Vec<Vec<&str>> = vec![];
    for line in ini.lines() {
        let line = line.trim();
        if line.starts_with('[') || sections.is_empty() {
            sections.push(vec![]);
        }
        sections.last_mut().unwrap().push(line);
    }
    let get = |section: &[&str], k: &str| section.iter().find_map(|l| l.strip_prefix(k)).map(str::trim).map(String::from);
    let header = |s: &[&str], prefix: &str| s.first().is_some_and(|h| h.starts_with(prefix));
    let mut dirs: Vec<PathBuf> = vec![];
    // `[Install<hash>]` names the profile an installation opens by default.
    for section in sections.iter().filter(|s| header(s, "[Install")) {
        if let Some(raw) = get(section, "Default=") {
            dirs.push(root.join(raw)); // join keeps an absolute path as is
        }
    }
    let profiles: Vec<&Vec<&str>> = sections.iter().filter(|s| header(s, "[Profile")).collect();
    let marked = |s: &&&Vec<&str>| s.join("\n").to_lowercase().contains("default=1");
    for section in profiles.iter().filter(marked).chain(profiles.iter().filter(|s| !marked(s))) {
        if let Some(raw) = get(section, "Path=") {
            let relative = get(section, "IsRelative=").map_or(true, |v| v != "0");
            dirs.push(if relative { root.join(raw) } else { PathBuf::from(raw) });
        }
    }
    let mut unlisted: Vec<PathBuf> = std::fs::read_dir(root).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    unlisted.sort();
    dirs.extend(unlisted);
    let written = |dir: &PathBuf| -> Option<std::time::SystemTime> {
        let base = dir.join("places.sqlite");
        let t = std::fs::metadata(&base).ok()?.modified().ok()?;
        let wal = std::fs::metadata(dir.join("places.sqlite-wal")).and_then(|m| m.modified()).ok();
        Some(wal.map_or(t, |w| w.max(t)))
    };
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for dir in dirs {
        if let Some(t) = written(&dir) {
            if best.as_ref().map_or(true, |(bt, _)| t > *bt) {
                best = Some((t, dir.join("places.sqlite")));
            }
        }
    }
    best.map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use knobdeck_sdk::json;

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

    #[test]
    fn firefox_profile_is_the_one_in_use() {
        let root = std::env::temp_dir().join(format!("kd-ff-{}", std::process::id()));
        let touch = |p: &str| {
            std::fs::create_dir_all(root.join(p)).unwrap();
            std::fs::write(root.join(p).join("places.sqlite"), "").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(30));
        };
        let ini = "[General]\nStartWithLastProfile=1\n\n[Profile1]\nName=old\nIsRelative=1\nPath=old.default\nDefault=1\n\n[Profile0]\nName=Default (release)\nIsRelative=1\nPath=a9.Default (release)\n\n[Install54D0F503E9AABA74]\nDefault=a9.Default (release)\nLocked=1\n";
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("profiles.ini"), ini).unwrap();
        touch("old.default");
        touch("a9.Default (release)");
        assert_eq!(resolve_firefox_places(&root), Some(root.join("a9.Default (release)/places.sqlite")));
        // A selectable profile missing from profiles.ini, used since: it wins.
        touch("5k.Default (release)-2");
        assert_eq!(resolve_firefox_places(&root), Some(root.join("5k.Default (release)-2/places.sqlite")));
        // Folders without a places database are ignored.
        std::fs::create_dir_all(root.join("Profile Groups")).unwrap();
        assert_eq!(resolve_firefox_places(&root), Some(root.join("5k.Default (release)-2/places.sqlite")));
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_locations() {
        let chrome = meta("chrome");
        assert!(chrome.exe_candidates.contains(&PathBuf::from("/usr/bin/google-chrome-stable")));
        assert!(chrome.history_path.unwrap().ends_with("google-chrome/Default/History"));
        assert_eq!(meta("firefox").private_shortcut, "ctrl shift p");
        assert_eq!(meta("chrome").hard_reload_shortcut, "ctrl f5");
        assert_eq!(detect_key(&json!({"label": "Zen Browser", "value": "/usr/share/applications/zen.desktop"})), Some("zen"));
        assert_eq!(detect_key(&json!({"label": "Brave", "value": "/usr/share/applications/brave-browser.desktop"})), Some("brave"));
        assert_eq!(detect_key(&json!({"label": "Microsoft Edge", "value": "/usr/share/applications/microsoft-edge.desktop"})), Some("msedge"));
        eprintln!("zen: exe={:?} places={:?}", exe_path("/usr/share/applications/zen.desktop"), meta("zen").history_path);
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
