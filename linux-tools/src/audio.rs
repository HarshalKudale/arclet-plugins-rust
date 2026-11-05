//! Audio through the sound server's own client, `pactl` (PulseAudio, or PipeWire's pulse server,
//! which Debian, Ubuntu, Fedora and Arch desktops all run). Devices are sinks (output) and
//! sources (input); per-app volume works on the app's playback streams (sink inputs).

use knobdeck_sdk::{Error, Result, Value};
use std::process::Command;

/// The pseudo device meaning "whatever the default is now".
pub const DEFAULT_DEVICE: &str = "__default__";

fn pactl(args: &[&str]) -> Result<String> {
    let out = Command::new("pactl").args(args).output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Error::msg("pactl is missing: install pulseaudio-utils (Debian/Ubuntu) or libpulse (Arch)"),
        _ => Error::msg(format!("pactl: {e}")),
    })?;
    if !out.status.success() {
        return Err(Error::msg(format!("pactl {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn pactl_json(what: &str) -> Result<Vec<Value>> {
    let text = pactl(&["-f", "json", "list", what])?;
    // pactl ≥ 16 prints a JSON array; older versions ignore -f and print text.
    serde_json::from_str::<Value>(&text).ok().and_then(|v| v.as_array().cloned()).ok_or_else(|| Error::msg("pactl is too old for JSON output (needs PulseAudio 16 or PipeWire 0.3.40+)"))
}

/// Average channel volume in percent, from pactl's `{"front-left": {"value_percent": "40%"}, …}`.
fn volume_percent(v: &Value) -> i64 {
    let Some(map) = v.as_object() else { return 0 };
    let vals: Vec<f64> = map.values().filter_map(|c| c["value_percent"].as_str()?.trim_end_matches('%').trim().parse().ok()).collect();
    if vals.is_empty() {
        0
    } else {
        (vals.iter().sum::<f64>() / vals.len() as f64).round() as i64
    }
}

pub struct Device {
    pub id: String,
    pub name: String,
}

fn kind(input: bool) -> (&'static str, &'static str) {
    if input {
        ("sources", "source")
    } else {
        ("sinks", "sink")
    }
}

/// Playback or recording devices (monitors of outputs left out).
pub fn devices(input: bool) -> Result<Vec<Device>> {
    Ok(pactl_json(kind(input).0)?
        .into_iter()
        .filter(|d| !(input && (d["monitor_of_sink"].as_str().is_some_and(|m| !m.is_empty() && m != "n/a") || d["properties"]["device.class"] == "monitor")))
        .map(|d| Device { id: d["name"].as_str().unwrap_or("").to_string(), name: d["description"].as_str().unwrap_or_else(|| d["name"].as_str().unwrap_or("")).to_string() })
        .filter(|d| !d.id.is_empty())
        .collect())
}

pub fn default_device(input: bool) -> Option<String> {
    pactl(&[if input { "get-default-source" } else { "get-default-sink" }]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Make a device the default. PipeWire and PulseAudio move streams that follow the default.
pub fn set_default(input: bool, id: &str) -> Result {
    pactl(&[if input { "set-default-source" } else { "set-default-sink" }, id]).map(|_| ())
}

fn device_arg(input: bool, id: &str) -> String {
    if id.is_empty() || id == DEFAULT_DEVICE {
        (if input { "@DEFAULT_SOURCE@" } else { "@DEFAULT_SINK@" }).into()
    } else {
        id.into()
    }
}

pub fn device_muted(input: bool, id: &str) -> Result<bool> {
    let k = kind(input).1;
    Ok(pactl(&[&format!("get-{k}-mute"), &device_arg(input, id)])?.contains("yes"))
}

pub fn set_device_mute(input: bool, id: &str, mute: bool) -> Result {
    let k = kind(input).1;
    pactl(&[&format!("set-{k}-mute"), &device_arg(input, id), if mute { "1" } else { "0" }]).map(|_| ())
}

pub fn toggle_device_mute(input: bool, id: &str) -> Result<bool> {
    let muted = !device_muted(input, id)?;
    set_device_mute(input, id, muted)?;
    Ok(muted)
}

/// `Volume: front-left: 26214 /  40% / -23.88 dB,   front-right: …` → 40.
fn parse_volume_line(text: &str) -> Option<i64> {
    let vals: Vec<f64> = text.split('/').filter_map(|p| p.trim().strip_suffix('%')?.trim().parse().ok()).collect();
    (!vals.is_empty()).then(|| (vals.iter().sum::<f64>() / vals.len() as f64).round() as i64)
}

pub fn device_volume(input: bool, id: &str) -> Result<i64> {
    let k = kind(input).1;
    parse_volume_line(&pactl(&[&format!("get-{k}-volume"), &device_arg(input, id)])?).ok_or_else(|| Error::msg("Could not read the volume"))
}

pub fn set_device_volume(input: bool, id: &str, pct: f64) -> Result {
    let k = kind(input).1;
    pactl(&[&format!("set-{k}-volume"), &device_arg(input, id), &format!("{}%", pct.round().clamp(0.0, 100.0) as i64)]).map(|_| ())
}

pub fn adjust_device_volume(input: bool, id: &str, delta: f64) -> Result<i64> {
    let target = (device_volume(input, id)? as f64 + delta).clamp(0.0, 100.0);
    set_device_volume(input, id, target)?;
    Ok(target as i64)
}

// ── Per-app streams ──

pub struct Stream {
    index: u64,
    /// The process binary (`firefox`, `spotify`), else the app's name.
    pub app: String,
    pub label: String,
    pid: u32,
    muted: bool,
    volume: i64,
}

pub fn streams() -> Result<Vec<Stream>> {
    Ok(pactl_json("sink-inputs")?
        .into_iter()
        .filter_map(|s| {
            let p = &s["properties"];
            let name = p["application.name"].as_str().unwrap_or("").to_string();
            let binary = p["application.process.binary"].as_str().unwrap_or("").to_string();
            let app = if binary.is_empty() { name.clone() } else { binary };
            if app.is_empty() {
                return None;
            }
            Some(Stream {
                index: s["index"].as_u64()?,
                label: if name.is_empty() { app.clone() } else { name },
                app,
                pid: p["application.process.id"].as_str().and_then(|v| v.parse().ok()).unwrap_or(0),
                muted: s["mute"].as_bool().unwrap_or(false),
                volume: volume_percent(&s["volume"]),
            })
        })
        .collect())
}

/// Apps currently playing, one entry per app: (value, label).
pub fn stream_apps() -> Result<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = vec![];
    for s in streams()? {
        if !out.iter().any(|(a, _)| a.eq_ignore_ascii_case(&s.app)) {
            out.push((s.app, s.label));
        }
    }
    Ok(out)
}

/// What a per-app action targets: an app name, or a window's process and class.
pub enum Target {
    App(String),
    Window { pid: u32, class: String },
}

fn parent_pid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // pid (comm) state ppid …; comm may hold spaces and parens, so split after the last ')'.
    stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

/// Whether `pid` is `root` or one of its descendants (browsers play from a child process).
fn descends_from(mut pid: u32, root: u32) -> bool {
    for _ in 0..16 {
        if pid == root {
            return true;
        }
        match parent_pid(pid) {
            Some(p) if p > 1 => pid = p,
            _ => return false,
        }
    }
    false
}

fn matches(s: &Stream, t: &Target) -> bool {
    match t {
        Target::App(a) => s.app.eq_ignore_ascii_case(a) || s.label.eq_ignore_ascii_case(a),
        Target::Window { pid, class } => {
            let class = class.rsplit('.').next().unwrap_or(class).to_ascii_lowercase();
            (*pid > 0 && s.pid > 0 && descends_from(s.pid, *pid)) || (!class.is_empty() && (s.app.to_ascii_lowercase() == class || s.label.to_ascii_lowercase() == class))
        }
    }
}

fn target_streams(t: &Target) -> Result<Vec<Stream>> {
    Ok(streams()?.into_iter().filter(|s| matches(s, t)).collect())
}

/// Set every matching stream's volume; returns how many matched.
pub fn set_app_volume(t: &Target, pct: f64) -> Result<usize> {
    let list = target_streams(t)?;
    for s in &list {
        pactl(&["set-sink-input-volume", &s.index.to_string(), &format!("{}%", pct.round().clamp(0.0, 100.0) as i64)])?;
    }
    Ok(list.len())
}

pub fn app_volume(t: &Target) -> Result<Option<i64>> {
    Ok(target_streams(t)?.first().map(|s| s.volume))
}

pub fn adjust_app_volume(t: &Target, delta: f64) -> Result<Option<i64>> {
    let Some(current) = app_volume(t)? else { return Ok(None) };
    let target = (current as f64 + delta).clamp(0.0, 100.0);
    set_app_volume(t, target)?;
    Ok(Some(target as i64))
}

pub fn app_mute(t: &Target) -> Result<Option<bool>> {
    Ok(target_streams(t)?.first().map(|s| s.muted))
}

pub fn toggle_app_mute(t: &Target) -> Result<Option<bool>> {
    let list = target_streams(t)?;
    let Some(first) = list.first() else { return Ok(None) };
    let mute = !first.muted;
    for s in &list {
        pactl(&["set-sink-input-mute", &s.index.to_string(), if mute { "1" } else { "0" }])?;
    }
    Ok(Some(mute))
}

#[cfg(test)]
mod tests {
    use super::*;
    use knobdeck_sdk::json;

    #[test]
    fn volume_parsing() {
        assert_eq!(parse_volume_line("Volume: front-left: 26214 /  40% / -23.88 dB,   front-right: 32768 /  50% / -18.06 dB\n"), Some(45));
        assert_eq!(parse_volume_line("Volume: mono: 65536 / 100% / 0.00 dB"), Some(100));
        assert_eq!(parse_volume_line("nothing"), None);
        assert_eq!(volume_percent(&json!({"front-left": {"value_percent": "40%"}, "front-right": {"value_percent": "60%"}})), 50);
    }

    #[test]
    fn own_process_descends_from_itself_and_parent() {
        let me = std::process::id();
        assert!(descends_from(me, me));
        assert!(descends_from(me, parent_pid(me).unwrap()));
        assert!(!descends_from(me, u32::MAX));
    }

    /// Reads devices and streams; changes nothing.
    #[test]
    fn reads_devices_and_streams() {
        if Command::new("pactl").arg("info").output().is_err() {
            return;
        }
        for input in [false, true] {
            let list = devices(input).unwrap();
            eprintln!("input={input} default={:?} devices={:?}", default_device(input), list.iter().map(|d| &d.name).collect::<Vec<_>>());
            if !list.is_empty() {
                eprintln!("volume={:?} muted={:?}", device_volume(input, DEFAULT_DEVICE), device_muted(input, DEFAULT_DEVICE));
            }
        }
        eprintln!("apps={:?}", stream_apps().unwrap());
    }
}
