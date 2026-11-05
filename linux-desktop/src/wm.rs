//! Window management on KDE Plasma and GNOME, on Wayland and X11.
//!
//! Wayland gives clients no way to see or move other apps' windows, so this asks the compositor
//! itself: KWin runs a short script (`kwin.rs`), GNOME Shell runs the KnobDeck extension
//! (`gnome.rs`). Both understand the same commands and answer with the same JSON:
//!
//! | command | args | result |
//! |---|---|---|
//! | `outputs` | | `[Output]` |
//! | `list` | | `[Window]`, top of the stack (or most recently used) first |
//! | `active` | | `Window` or null |
//! | `info` | `id` | `Window` |
//! | `minimize` `unminimize` `maximize` `restore` `close` `activate` `lower` | `id?` | null |
//! | `toggle-maximize` `toggle-fullscreen` `toggle-above` `toggle-border` | `id?` | new state (bool) |
//! | `opacity` | `id?`, `value` 0–1 | null |
//! | `move` | `id?`, `x y w h` (frame, global logical px) | the new frame |
//! | `to-output` | `id?`, `index` | null |
//! | `desktop` | `dir`: `next`/`prev`/`new`/`close` | null |
//! | `show-desktop` | | null |
//!
//! A missing `id` means the focused window.

use crate::{desktop, gnome, kwin, Desktop};
use knobdeck_sdk::{json, Error, Result, Value};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn from(v: &Value) -> Rect {
        let n = |k: &str| v[k].as_f64().unwrap_or(0.0);
        Rect { x: n("x"), y: n("y"), w: n("w"), h: n("h") }
    }
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

#[derive(Clone, Debug)]
pub struct Output {
    pub index: usize,
    pub name: String,
    pub frame: Rect,
    /// The area windows may use (panels and docks excluded).
    pub work: Rect,
    pub primary: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Window {
    pub id: String,
    pub title: String,
    /// Window class / Wayland app id (`firefox`, `org.kde.dolphin`).
    pub app: String,
    /// The app's desktop entry id, without `.desktop`, when the compositor knows it.
    pub desktop_file: String,
    pub pid: u32,
    pub frame: Rect,
    pub output: Option<usize>,
    pub minimized: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub above: bool,
    pub no_border: bool,
    pub opacity: f64,
}

impl Window {
    fn from(v: &Value) -> Window {
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        let b = |k: &str| v[k].as_bool().unwrap_or(false);
        Window {
            id: s("id"),
            title: s("title"),
            app: s("app"),
            desktop_file: s("desktopFile").trim_end_matches(".desktop").to_string(),
            pid: v["pid"].as_u64().unwrap_or(0) as u32,
            frame: Rect::from(&v["frame"]),
            output: v["output"].as_i64().filter(|i| *i >= 0).map(|i| i as usize),
            minimized: b("minimized"),
            maximized: b("maximized"),
            fullscreen: b("fullscreen"),
            above: b("above"),
            no_border: b("noBorder"),
            opacity: v["opacity"].as_f64().unwrap_or(1.0),
        }
    }

    /// The app's name from its desktop entry, else its class.
    pub fn app_name(&self) -> String {
        crate::apps::find(&self.desktop_file, &self.app).map(|e| e.name).unwrap_or_else(|| self.app.clone())
    }
}

/// Run one bridge command on the running desktop.
pub fn call(cmd: &str, args: Value) -> Result<Value> {
    match desktop() {
        Desktop::Kde => kwin::call(cmd, &args),
        Desktop::Gnome => gnome::call(cmd, &args),
        Desktop::Other => Err(Error::msg("Window actions need GNOME or KDE Plasma")),
    }
}

fn target(id: Option<&str>) -> Value {
    match id {
        Some(id) => json!({ "id": id }),
        None => json!({}),
    }
}

/// Apply a no-result command to a window (`None` = the focused one).
pub fn op(cmd: &str, id: Option<&str>) -> Result {
    call(cmd, target(id)).map(|_| ())
}

/// Apply a toggle command and return the new state.
pub fn toggle(cmd: &str, id: Option<&str>) -> Result<bool> {
    Ok(call(cmd, target(id))?.as_bool().unwrap_or(false))
}

pub fn outputs() -> Result<Vec<Output>> {
    let v = call("outputs", json!({}))?;
    Ok(v.as_array()
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, o)| Output {
                    index: o["index"].as_u64().map_or(i, |n| n as usize),
                    name: o["name"].as_str().unwrap_or("").to_string(),
                    frame: Rect::from(&o["frame"]),
                    work: Rect::from(&o["work"]),
                    primary: o["primary"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Normal windows, top of the stack / most recently used first.
pub fn list() -> Result<Vec<Window>> {
    Ok(call("list", json!({}))?.as_array().map(|a| a.iter().map(Window::from).collect()).unwrap_or_default())
}

/// The focused window.
pub fn active() -> Result<Window> {
    let v = call("active", json!({}))?;
    if v.is_null() {
        return Err(Error::msg("No window has focus"));
    }
    Ok(Window::from(&v))
}

pub fn info(id: Option<&str>) -> Result<Window> {
    match id {
        None => active(),
        Some(_) => Ok(Window::from(&call("info", target(id))?)),
    }
}

/// Move and resize a window's frame (global logical pixels). Leaves maximize/full screen first.
pub fn move_to(id: Option<&str>, r: Rect) -> Result {
    let mut args = target(id);
    args["x"] = json!(r.x.round());
    args["y"] = json!(r.y.round());
    args["w"] = json!(r.w.round().max(1.0));
    args["h"] = json!(r.h.round().max(1.0));
    call("move", args).map(|_| ())
}

pub fn set_opacity(id: Option<&str>, value: f64) -> Result {
    let mut args = target(id);
    args["value"] = json!(value.clamp(0.05, 1.0));
    call("opacity", args).map(|_| ())
}

pub fn to_output(id: Option<&str>, index: usize) -> Result {
    let mut args = target(id);
    args["index"] = json!(index);
    call("to-output", args).map(|_| ())
}

/// `next`, `prev`, `new` or `close`.
pub fn desktop_switch(dir: &str) -> Result {
    call("desktop", json!({ "dir": dir })).map(|_| ())
}

pub fn show_desktop() -> Result {
    call("show-desktop", json!({})).map(|_| ())
}

/// The output a window is on (by its reported index, else by its center).
pub fn output_of(w: &Window, outputs: &[Output]) -> Option<usize> {
    w.output.filter(|i| *i < outputs.len()).or_else(|| outputs.iter().position(|o| o.frame.contains(w.frame.center())))
}

/// Focus the first window whose class or desktop entry matches one of `apps` (case-insensitive;
/// `org.mozilla.firefox` matches `firefox`). Returns whether one was found.
pub fn activate_app(apps: &[&str]) -> Result<bool> {
    let windows = list()?;
    let hit = windows.iter().find(|w| apps.iter().any(|a| app_matches(w, a)));
    match hit {
        Some(w) => op("activate", Some(&w.id)).map(|_| true),
        None => Ok(false),
    }
}

/// Whether a window belongs to `app` (a class, desktop id or executable name).
pub fn app_matches(w: &Window, app: &str) -> bool {
    let app = app.trim().to_ascii_lowercase();
    if app.is_empty() {
        return false;
    }
    let last = |s: &str| s.rsplit('.').next().unwrap_or(s).to_ascii_lowercase();
    [&w.app, &w.desktop_file].iter().any(|c| {
        let c = c.to_ascii_lowercase();
        !c.is_empty() && (c == app || last(&c) == app || last(&c) == last(&app))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_windows() {
        let w = Window::from(&json!({"id": "{a}", "title": "T", "app": "org.mozilla.firefox", "desktopFile": "firefox.desktop", "pid": 42, "frame": {"x": 1, "y": 2, "w": 3, "h": 4}, "output": 1, "maximized": true}));
        assert_eq!(w.desktop_file, "firefox");
        assert_eq!(w.frame, Rect { x: 1.0, y: 2.0, w: 3.0, h: 4.0 });
        assert_eq!(w.output, Some(1));
        assert!(w.maximized && !w.minimized);
        assert!(app_matches(&w, "firefox"));
        assert!(app_matches(&w, "Firefox"));
        assert!(!app_matches(&w, "fire"));
        assert!(!app_matches(&w, ""));
    }

    #[test]
    fn output_by_center() {
        let o = |i, x| Output { index: i, name: String::new(), frame: Rect { x, y: 0.0, w: 100.0, h: 100.0 }, work: Rect::default(), primary: i == 0 };
        let outs = [o(0, 0.0), o(1, 100.0)];
        let w = Window { frame: Rect { x: 120.0, y: 10.0, w: 20.0, h: 20.0 }, ..Default::default() };
        assert_eq!(output_of(&w, &outs), Some(1));
    }

    /// Reads windows and outputs from the running desktop; changes nothing.
    /// `cargo test -p linux-desktop -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_read_only() {
        eprintln!("desktop: {:?}", desktop());
        for o in outputs().unwrap() {
            eprintln!("{o:?}");
        }
        for w in list().unwrap() {
            eprintln!("{:?} | {} | {} | {:?}", w.app, w.title, w.app_name(), w.frame);
        }
        eprintln!("active: {:?}", active().map(|w| w.title));
    }

    /// Acts only on a window titled exactly `knobdeck-wm-test` (create one first; pin kitty's size,
    /// it otherwise reopens at the last window's size:
    /// `kitty -o confirm_os_window_close=0 -o remember_window_size=no -o initial_window_width=640 -o initial_window_height=400 -T knobdeck-wm-test sleep 120 &`).
    /// Needs an unlocked screen: a hidden client never acknowledges resizes. Moves, toggles,
    /// re-reads and closes it.
    #[test]
    #[ignore]
    fn live_window_ops() {
        let w = list().unwrap().into_iter().find(|w| w.title == "knobdeck-wm-test").expect("knobdeck-wm-test window");
        let id = Some(w.id.as_str());
        let outs = outputs().unwrap();
        let work = outs[output_of(&w, &outs).unwrap()].work;
        let target = Rect { x: work.x + 100.0, y: work.y + 80.0, w: 640.0, h: 400.0 };
        move_to(id, target).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let f = info(id).unwrap().frame;
        eprintln!("moved: {f:?}");
        assert!((f.x - target.x).abs() <= 2.0 && (f.y - target.y).abs() <= 2.0 && (f.w - target.w).abs() <= 2.0, "{f:?}");
        assert!(toggle("toggle-maximize", id).unwrap());
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(info(id).unwrap().maximized);
        op("restore", id).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!info(id).unwrap().maximized);
        let above = toggle("toggle-above", id).unwrap();
        assert_eq!(info(id).unwrap().above, above);
        toggle("toggle-above", id).unwrap();
        set_opacity(id, 0.6).unwrap();
        assert!((info(id).unwrap().opacity - 0.6).abs() < 0.01);
        set_opacity(id, 1.0).unwrap();
        op("minimize", id).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(info(id).unwrap().minimized);
        op("activate", id).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!info(id).unwrap().minimized);
        if outs.len() > 1 {
            let to = (output_of(&w, &outs).unwrap() + 1) % outs.len();
            to_output(id, to).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
            assert_eq!(output_of(&info(id).unwrap(), &outs), Some(to));
        }
        assert!(activate_app(&["kitty"]).unwrap());
        op("close", id).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(info(id).is_err(), "window should be gone");
    }
}

