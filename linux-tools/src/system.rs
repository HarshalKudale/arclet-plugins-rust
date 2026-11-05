//! Desktop and system actions through freedesktop / KDE / GNOME D-Bus services, never keystrokes.

use knobdeck_sdk::{json, Error, Result, Value};
use linux_desktop::{bus, desktop, spawn_detached, which, wm, Desktop};
use zbus::zvariant::OwnedValue;

/// Run one of KWin's / Plasma's own global actions by name through KGlobalAccel. This is the
/// action itself (what its shortcut would trigger), so it works whatever key, if any, the user
/// bound to it.
pub fn kde_action(component: &str, name: &str) -> Result {
    let conn = bus::session()?;
    bus::call0(&conn, "org.kde.kglobalaccel", &format!("/component/{component}"), "org.kde.kglobalaccel.Component", "invokeShortcut", &(name,))
}

/// A Shell action through the KnobDeck GNOME extension.
fn gnome(cmd: &str) -> Result {
    linux_desktop::gnome::call(cmd, &json!({})).map(|_| ())
}

fn launch_first(programs: &[&'static str], what: &str) -> Result {
    match which(programs) {
        Some(p) => spawn_detached(p, &[]),
        None => Err(Error::msg(format!("No {what} app found"))),
    }
}

pub fn overview() -> Result {
    match desktop() {
        Desktop::Kde => kde_action("kwin", "Overview"),
        Desktop::Gnome => gnome("overview"),
        Desktop::Other => Err(linux_desktop::unsupported()),
    }
}

/// Every window of the current desktop side by side (GNOME's overview does this).
pub fn present_windows() -> Result {
    match desktop() {
        Desktop::Kde => kde_action("kwin", "Expose"),
        Desktop::Gnome => gnome("overview"),
        Desktop::Other => Err(linux_desktop::unsupported()),
    }
}

/// The app launcher: Plasma's application menu, GNOME's app grid.
pub fn app_launcher() -> Result {
    match desktop() {
        Desktop::Kde => kde_action("plasmashell", "activate application launcher"),
        Desktop::Gnome => gnome("app-grid"),
        Desktop::Other => Err(linux_desktop::unsupported()),
    }
}

/// KRunner on Plasma, the overview's search on GNOME.
pub fn search() -> Result {
    match desktop() {
        Desktop::Kde => bus::call0(&bus::session()?, "org.kde.krunner", "/App", "org.kde.krunner.App", "display", &()),
        Desktop::Gnome => gnome("search"),
        Desktop::Other => Err(linux_desktop::unsupported()),
    }
}

/// Region screenshot: Spectacle on Plasma, the Shell's screenshot UI on GNOME.
pub fn screenshot() -> Result {
    match desktop() {
        Desktop::Kde => spawn_detached(which(&["spectacle"]).ok_or_else(|| Error::msg("Spectacle is not installed"))?, &["--region"]),
        Desktop::Gnome => gnome("screenshot"),
        Desktop::Other => Err(linux_desktop::unsupported()),
    }
}

/// Plasma's emoji picker, GNOME Characters.
pub fn emoji_picker() -> Result {
    launch_first(&["plasma-emojier", "gnome-characters"], "emoji picker")
}

pub fn settings() -> Result {
    match desktop() {
        Desktop::Kde => launch_first(&["systemsettings", "systemsettings5"], "settings"),
        Desktop::Gnome => launch_first(&["gnome-control-center"], "settings"),
        Desktop::Other => Err(linux_desktop::unsupported()),
    }
}

pub fn system_monitor() -> Result {
    match desktop() {
        Desktop::Kde => launch_first(&["plasma-systemmonitor", "ksysguard"], "system monitor"),
        _ => launch_first(&["gnome-system-monitor", "resources", "plasma-systemmonitor"], "system monitor"),
    }
}

/// The home folder in the desktop's file manager (`org.freedesktop.FileManager1`).
pub fn file_manager() -> Result {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let conn = bus::session()?;
    let uri = format!("file://{home}");
    bus::call0(&conn, "org.freedesktop.FileManager1", "/org/freedesktop/FileManager1", "org.freedesktop.FileManager1", "ShowFolders", &(vec![uri.as_str()], ""))
        .or_else(|_| spawn_detached("xdg-open", &[&home]))
}

/// Lock through logind (the session's own lock request), else the screensaver interface.
pub fn lock_screen() -> Result {
    let logind = bus::system().and_then(|c| bus::call0(&c, "org.freedesktop.login1", "/org/freedesktop/login1/session/auto", "org.freedesktop.login1.Session", "Lock", &()));
    if logind.is_ok() {
        return Ok(());
    }
    bus::call0(&bus::session()?, "org.freedesktop.ScreenSaver", "/org/freedesktop/ScreenSaver", "org.freedesktop.ScreenSaver", "Lock", &())
}

/// Turn the displays off: PowerDevil's own action on Plasma; GNOME blanks through its
/// screensaver (which also locks once the lock delay passes, as GNOME does when idle).
pub fn sleep_display() -> Result {
    match desktop() {
        Desktop::Kde => kde_action("org_kde_powerdevil", "Turn Off Screen"),
        Desktop::Gnome => bus::call0(&bus::session()?, "org.gnome.ScreenSaver", "/org/gnome/ScreenSaver", "org.gnome.ScreenSaver", "SetActive", &(true,)),
        Desktop::Other => Err(linux_desktop::unsupported()),
    }
}

pub fn desktop_switch(dir: &str) -> Result {
    wm::desktop_switch(dir)
}

pub fn show_desktop() -> Result {
    wm::show_desktop()
}

// ── Power profiles (power-profiles-daemon, used by GNOME and Plasma) ──

/// power-profiles-daemon's current name, then its older one.
const PPD: [(&str, &str); 2] = [("org.freedesktop.UPower.PowerProfiles", "/org/freedesktop/UPower/PowerProfiles"), ("net.hadess.PowerProfiles", "/net/hadess/PowerProfiles")];

fn ppd() -> Result<(&'static str, &'static str)> {
    PPD.into_iter().find(|(n, _)| bus::system_has_owner(n)).ok_or_else(|| Error::msg("Power profiles need power-profiles-daemon (or tuned-ppd)"))
}

pub fn profile_label(id: &str) -> String {
    match id {
        "power-saver" => "Power Saver".into(),
        "balanced" => "Balanced".into(),
        "performance" => "Performance".into(),
        other => other.into(),
    }
}

/// The available profiles (`power-saver`, `balanced`, `performance`).
pub fn power_profiles() -> Result<Vec<String>> {
    let (name, path) = ppd()?;
    let conn = bus::system()?;
    let list: Vec<std::collections::HashMap<String, OwnedValue>> = bus::get(&conn, name, path, name, "Profiles")?;
    Ok(list.into_iter().filter_map(|p| String::try_from(p.get("Profile")?.try_clone().ok()?).ok()).collect())
}

pub fn active_profile() -> Result<String> {
    let (name, path) = ppd()?;
    bus::get::<String>(&bus::system()?, name, path, name, "ActiveProfile")
}

pub fn set_profile(id: &str) -> Result {
    if id.is_empty() {
        return Err(Error::msg("Choose a power profile first"));
    }
    let (name, path) = ppd()?;
    bus::set(&bus::system()?, name, path, name, "ActiveProfile", id)
}

// ── Wi-Fi (NetworkManager) and Bluetooth (BlueZ) ──

const NM: &str = "org.freedesktop.NetworkManager";

/// The first Bluetooth adapter's object path.
fn bluetooth_adapter() -> Result<String> {
    let conn = bus::system()?;
    let objects: std::collections::HashMap<zbus::zvariant::OwnedObjectPath, std::collections::HashMap<String, std::collections::HashMap<String, OwnedValue>>> =
        bus::call(&conn, "org.bluez", "/", "org.freedesktop.DBus.ObjectManager", "GetManagedObjects", &()).map_err(|_| Error::msg("Bluetooth service (BlueZ) is not running"))?;
    let mut adapters: Vec<String> = objects.into_iter().filter(|(_, ifaces)| ifaces.contains_key("org.bluez.Adapter1")).map(|(p, _)| p.to_string()).collect();
    adapters.sort();
    adapters.into_iter().next().ok_or_else(|| Error::msg("No Bluetooth adapter found"))
}

pub fn radio_on(kind: &str) -> Result<bool> {
    let conn = bus::system()?;
    if kind == "bluetooth" {
        let path = bluetooth_adapter()?;
        bus::get::<bool>(&conn, "org.bluez", &path, "org.bluez.Adapter1", "Powered")
    } else {
        bus::get::<bool>(&conn, NM, "/org/freedesktop/NetworkManager", NM, "WirelessEnabled").map_err(|_| Error::msg("Wi-Fi needs NetworkManager"))
    }
}

/// Switch a radio; returns the new state.
pub fn set_radio(kind: &str, on: bool) -> Result<bool> {
    let conn = bus::system()?;
    if kind == "bluetooth" {
        let path = bluetooth_adapter()?;
        bus::set(&conn, "org.bluez", &path, "org.bluez.Adapter1", "Powered", on).map_err(|e| Error::msg(format!("Bluetooth: {} (is it blocked in rfkill?)", e.message())))?;
    } else {
        bus::set(&conn, NM, "/org/freedesktop/NetworkManager", NM, "WirelessEnabled", on)?;
    }
    Ok(on)
}

/// Round-trip time to `host` in ms through the system `ping` (which has the network capability
/// raw ICMP needs), or `None` on timeout.
pub fn ping(host: &str, timeout_ms: u32) -> Result<Option<u32>> {
    let host = host.trim();
    if host.is_empty() {
        return Err(Error::msg("Enter a host to ping"));
    }
    if host.starts_with('-') {
        return Err(Error::msg(format!("Unknown host \"{host}\"")));
    }
    let secs = timeout_ms.div_ceil(1000).max(1).to_string();
    let out = std::process::Command::new("ping").args(["-c", "1", "-n", "-W", &secs, host]).output().map_err(|e| Error::msg(format!("ping: {e}")))?;
    let text = String::from_utf8_lossy(&out.stdout);
    if let Some(ms) = parse_ping(&text) {
        return Ok(Some(ms));
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if err.contains("Name or service not known") || err.contains("unknown host") || err.contains("Temporary failure in name resolution") {
        return Err(Error::msg(format!("Unknown host \"{host}\"")));
    }
    Ok(None)
}

/// `64 bytes from 1.1.1.1: icmp_seq=1 ttl=57 time=12.3 ms` → 12.
fn parse_ping(text: &str) -> Option<u32> {
    let t = text.split("time=").nth(1)?;
    t.split_whitespace().next()?.parse::<f64>().ok().map(|ms| ms.round() as u32)
}

pub fn options(list: Vec<(String, String)>) -> Value {
    Value::Array(list.into_iter().map(|(v, l)| json!({"value": v, "label": l})).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_parsing() {
        assert_eq!(parse_ping("64 bytes from 1.1.1.1: icmp_seq=1 ttl=57 time=12.3 ms\n"), Some(12));
        assert_eq!(parse_ping("no reply"), None);
    }

    #[test]
    fn ping_loopback_and_bad_input() {
        if linux_desktop::which(&["ping"]).is_none() {
            return;
        }
        assert!(ping("127.0.0.1", 1000).unwrap().is_some());
        assert!(ping("", 1000).is_err());
        assert!(ping("-f", 1000).is_err());
    }

    /// Reads only; switches nothing.
    #[test]
    fn reads_profiles_and_radios() {
        eprintln!("profiles: {:?} active: {:?}", power_profiles().map_err(|e| e.message().to_string()), active_profile().map_err(|e| e.message().to_string()));
        for kind in ["wifi", "bluetooth"] {
            eprintln!("{kind}: {:?}", radio_on(kind).map_err(|e| e.message().to_string()));
        }
    }
}
