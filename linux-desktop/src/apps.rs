//! XDG desktop entries: an app's display name and icon (as a data URI for wheel slices).

use base64::Engine;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub icon: String,
}

/// `$XDG_DATA_HOME` then `$XDG_DATA_DIRS` (Flatpak exports included when the session lists them).
pub fn data_dirs() -> Vec<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut dirs = vec![std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".local/share"))];
    let sys = std::env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(sys.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    for extra in [home.join(".local/share/flatpak/exports/share"), PathBuf::from("/var/lib/flatpak/exports/share")] {
        if !dirs.contains(&extra) {
            dirs.push(extra);
        }
    }
    dirs
}

fn parse(path: &Path) -> Option<(Entry, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let (mut name, mut icon, mut wm_class, mut in_main) = (String::new(), String::new(), String::new(), false);
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        if let Some(v) = line.strip_prefix("Name=") {
            name = v.trim().into();
        } else if let Some(v) = line.strip_prefix("Icon=") {
            icon = v.trim().into();
        } else if let Some(v) = line.strip_prefix("StartupWMClass=") {
            wm_class = v.trim().into();
        }
    }
    (!name.is_empty()).then(|| (Entry { path: path.to_path_buf(), name, icon }, wm_class))
}

/// The desktop entry for a window: by its desktop file id, else by `StartupWMClass` or file name
/// matching its class (`firefox` → `firefox.desktop`, `org.mozilla.firefox.desktop`).
pub fn find(desktop_id: &str, class: &str) -> Option<Entry> {
    let dirs: Vec<PathBuf> = data_dirs().into_iter().map(|d| d.join("applications")).filter(|d| d.is_dir()).collect();
    for id in [desktop_id, class].into_iter().filter(|s| !s.is_empty()) {
        let id = id.trim_end_matches(".desktop");
        for d in &dirs {
            if let Some((e, _)) = parse(&d.join(format!("{id}.desktop"))) {
                return Some(e);
            }
        }
    }
    let class = class.to_ascii_lowercase();
    if class.is_empty() {
        return None;
    }
    for d in &dirs {
        for f in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = f.path();
            if p.extension().is_none_or(|e| e != "desktop") {
                continue;
            }
            let stem = p.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let last = stem.rsplit('.').next().unwrap_or(&stem).to_string();
            if let Some((e, wm)) = parse(&p) {
                if wm.eq_ignore_ascii_case(&class) || last == class {
                    return Some(e);
                }
            }
        }
    }
    None
}

/// An icon name or path as a `data:` URI (PNG or SVG up to 256 KB), for wheel slices.
pub fn icon_data_uri(icon: &str) -> Option<String> {
    let path = icon_file(icon)?;
    let bytes = std::fs::read(&path).ok().filter(|b| b.len() <= 256 * 1024)?;
    let mime = if path.extension().is_some_and(|e| e == "svg") { "image/svg+xml" } else { "image/png" };
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

fn icon_file(icon: &str) -> Option<PathBuf> {
    if icon.is_empty() {
        return None;
    }
    let p = Path::new(icon);
    if p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    // hicolor holds every app's own icon; the other themes cover desktop apps that only ship there.
    const SUBDIRS: [&str; 12] = [
        "hicolor/scalable/apps",
        "hicolor/256x256/apps",
        "hicolor/128x128/apps",
        "hicolor/96x96/apps",
        "hicolor/64x64/apps",
        "hicolor/48x48/apps",
        "hicolor/32x32/apps",
        "breeze/apps/48",
        "breeze-dark/apps/48",
        "Adwaita/scalable/apps",
        "Papirus/48x48/apps",
        "Papirus/64x64/apps",
    ];
    let dirs = data_dirs();
    for d in &dirs {
        for sub in SUBDIRS {
            for ext in ["svg", "png"] {
                let f = d.join("icons").join(sub).join(format!("{icon}.{ext}"));
                if f.is_file() {
                    return Some(f);
                }
            }
        }
    }
    dirs.iter().flat_map(|d| ["svg", "png"].map(|ext| d.join("pixmaps").join(format!("{icon}.{ext}")))).chain(["svg", "png"].map(|ext| PathBuf::from(format!("/usr/share/pixmaps/{icon}.{ext}")))).find(|f| f.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_main_section_only() {
        let dir = std::env::temp_dir().join(format!("kd-apps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.desktop");
        std::fs::write(&f, "[Desktop Entry]\nName=Example\nIcon=example\nStartupWMClass=Ex\n[Desktop Action new]\nName=New Window\n").unwrap();
        let (e, wm) = parse(&f).unwrap();
        assert_eq!((e.name.as_str(), e.icon.as_str(), wm.as_str()), ("Example", "example", "Ex"));
        std::fs::remove_dir_all(dir).ok();
    }
}
