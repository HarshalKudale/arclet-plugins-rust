//! mac-tools — window management and system tools for Arclet on macOS; the
//! macOS counterpart of win-tools.
//!
//! Current Window (minimise / fill / restore / close / native full screen),
//! Position (halves, quarters, centre, custom, next display), window-picker
//! wheels ("Dynamic …"), a window switcher with app icons, Spaces / Mission
//! Control / system shortcuts, live system stats, audio device switching and
//! microphone mute, and a clipboard history.
//!
//! Window control goes through the Accessibility API (Arclet's permission).
//! What macOS doesn't allow for other apps' windows (always on top, opacity,
//! click-through, borderless, send to back, per-app volume) is left out
//! rather than faked.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("mac-tools only runs on macOS");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
mod audio;
#[cfg(target_os = "macos")]
mod ax;
#[cfg(target_os = "macos")]
mod clipboard;
#[cfg(target_os = "macos")]
mod icon;
#[cfg(target_os = "macos")]
mod screens;
#[cfg(target_os = "macos")]
mod stats;
#[cfg(target_os = "macos")]
mod system;
#[cfg(target_os = "macos")]
mod windows;

#[cfg(target_os = "macos")]
fn main() {
    app::run()
}

#[cfg(target_os = "macos")]
mod app {
    use crate::ax::{self, Elem, Rect};
    use crate::screens::{self, Display};
    use crate::system::{self, Dock, Hotkey};
    use crate::{audio, clipboard, icon, stats, windows};
    use arclet_sdk::input::{self, InputExt};
    use arclet_sdk::{json, Action, Ctx, Error, Plugin, Result, Value, Visual};
    use std::collections::HashMap;
    use std::process::Command;
    use std::sync::Mutex;

    fn parse_int(text: &str) -> Option<i64> {
        let t = text.trim();
        let end = t.char_indices().take_while(|&(i, c)| c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))).last().map(|(i, c)| i + c.len_utf8())?;
        t[..end].parse().ok()
    }

    fn input_int(inputs: &Value, key: &str) -> Option<i64> {
        match &inputs[key] {
            Value::String(s) => parse_int(s),
            Value::Number(n) => n.as_i64(),
            _ => None,
        }
    }

    fn str_input(inputs: &Value, key: &str) -> String {
        inputs[key].as_str().unwrap_or("").to_string()
    }

    // ── Displays ─────────────────────────────────────────────────────────────

    fn monitor_input() -> Value {
        input::query_select("monitor", "Display", "get-monitors", &[]).with("default", "current")
    }

    /// The display chosen by the `monitor` input ("current" = the one showing the window).
    fn resolve_display(frame: &Rect, choice: &str) -> Result<Display> {
        let list = screens::list();
        if list.is_empty() {
            return Err(Error::msg("No displays found"));
        }
        let i = match choice.parse::<usize>() {
            Ok(i) if i < list.len() => i,
            Ok(_) => return Err(Error::msg("That display is no longer connected")),
            Err(_) => screens::containing(&list, frame),
        };
        Ok(list[i].clone())
    }

    fn display_label(d: &Display, i: usize) -> String {
        format!("Display {}{} — {}×{} @ ({}, {})", i + 1, if d.primary { " (Main)" } else { "" }, d.frame.w, d.frame.h, d.frame.x, d.frame.y)
    }

    // ── Window operations ────────────────────────────────────────────────────

    #[derive(Clone, Copy)]
    struct Region {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }

    fn reg(x: f64, y: f64, w: f64, h: f64) -> Region {
        Region { x, y, w, h }
    }

    fn region_rect(work: &Rect, r: Region) -> Rect {
        Rect { x: (work.x + work.w * r.x).round(), y: (work.y + work.h * r.y).round(), w: (work.w * r.w).round(), h: (work.h * r.h).round() }
    }

    /// Frames before "Fill" so Restore / the toggle can put windows back.
    static SAVED: Mutex<Vec<(Elem, Rect)>> = Mutex::new(Vec::new());

    fn save_frame(w: &Elem, r: Rect) {
        let mut s = SAVED.lock().unwrap();
        s.retain(|(e, _)| e != w);
        s.push((w.clone(), r));
        if s.len() > 64 {
            s.remove(0);
        }
    }

    fn take_saved(w: &Elem) -> Option<Rect> {
        let mut s = SAVED.lock().unwrap();
        let i = s.iter().position(|(e, _)| e == w)?;
        Some(s.remove(i).1)
    }

    fn work_area(w: &Elem) -> Result<Rect> {
        Ok(resolve_display(&w.frame()?, "current")?.work)
    }

    /// Pinned to the work area's top-left and covering it, give or take the
    /// resize increment of grid-sized apps (Terminal rows and columns).
    fn covers(f: &Rect, work: &Rect) -> bool {
        const SLACK: f64 = 40.0;
        (f.x - work.x).abs() <= 4.0 && (f.y - work.y).abs() <= 4.0 && f.w >= work.w - SLACK && f.h >= work.h - SLACK && f.w <= work.w + 4.0 && f.h <= work.h + 4.0
    }

    fn is_filled(w: &Elem) -> bool {
        match (w.frame(), work_area(w)) {
            (Ok(f), Ok(work)) => covers(&f, &work),
            _ => false,
        }
    }

    fn minimize(w: &Elem) -> Result {
        w.set_minimized(true)
    }

    /// "Maximize" on macOS = fill the display's visible area (menu bar and Dock stay).
    fn fill(w: &Elem) -> Result {
        let cur = w.frame()?;
        let work = work_area(w)?;
        if !covers(&cur, &work) {
            save_frame(w, cur);
        }
        w.set_frame(work)
    }

    fn restore(w: &Elem) -> Result {
        if w.is_minimized() {
            return w.set_minimized(false);
        }
        if w.is_fullscreen() {
            return w.set_fullscreen(false);
        }
        match take_saved(w) {
            Some(r) => w.set_frame(r),
            None => Ok(()),
        }
    }

    fn toggle_fill(w: &Elem) -> Result<&'static str> {
        if is_filled(w) {
            let work = work_area(w)?;
            // Never filled by us: shrink to a centred 70% so the toggle still does something.
            let back = take_saved(w).unwrap_or(Rect { x: work.x + work.w * 0.15, y: work.y + work.h * 0.15, w: work.w * 0.7, h: work.h * 0.7 });
            w.set_frame(back)?;
            Ok("normal")
        } else {
            fill(w)?;
            Ok("maximized")
        }
    }

    fn toggle_fullscreen(w: &Elem) -> Result<&'static str> {
        let on = !w.is_fullscreen();
        w.set_fullscreen(on)?;
        Ok(if on { "fullscreen" } else { "windowed" })
    }

    fn center_in(w: &Elem, work: Rect) -> Result {
        let f = w.frame()?;
        let (cw, ch) = (f.w.min(work.w), f.h.min(work.h));
        w.set_frame(Rect { x: (work.x + (work.w - cw) / 2.0).round(), y: (work.y + (work.h - ch) / 2.0).round(), w: cw, h: ch })
    }

    /// Same relative place and size on the next display (wraps around).
    fn move_to_next_display(w: &Elem) -> Result {
        let list = screens::list();
        if list.len() < 2 {
            return Err(Error::msg("Only one display is connected"));
        }
        let f = w.frame()?;
        let i = screens::containing(&list, &f);
        let (from, to) = (list[i].work, list[(i + 1) % list.len()].work);
        let (sx, sy) = (to.w / from.w, to.h / from.h);
        let r = Rect { x: to.x + (f.x - from.x) * sx, y: to.y + (f.y - from.y) * sy, w: (f.w * sx).min(to.w), h: (f.h * sy).min(to.h) };
        w.set_frame(Rect { x: r.x.round(), y: r.y.round(), w: r.w.round(), h: r.h.round() })
    }

    fn hide_app(w: &Elem) -> Result {
        let pid = w.pid().ok_or_else(|| Error::msg("Window's app is gone"))?;
        Elem::app(pid).set_bool("AXHidden", true)
    }

    fn snap(w: &Elem, inputs: &Value, region: Region) -> Result {
        let d = resolve_display(&w.frame()?, &str_input(inputs, "monitor"))?;
        w.set_frame(region_rect(&d.work, region))
    }

    /// Custom-position inputs: X/Y from the display's visible top-left; blank keeps the current value.
    fn apply_custom_position(w: &Elem, inputs: &Value) -> Result {
        let cur = w.frame()?;
        let d = resolve_display(&cur, &str_input(inputs, "monitor"))?;
        let rel_x = input_int(inputs, "x").map_or(cur.x - d.work.x, |v| v as f64);
        let rel_y = input_int(inputs, "y").map_or(cur.y - d.work.y, |v| v as f64);
        let width = input_int(inputs, "w").filter(|v| *v > 0).map_or(cur.w, |v| v as f64);
        let height = input_int(inputs, "h").filter(|v| *v > 0).map_or(cur.h, |v| v as f64);
        w.set_frame(Rect { x: d.work.x + rel_x, y: d.work.y + rel_y, w: width, h: height })
    }

    fn custom_position_inputs() -> Vec<Value> {
        let keep = |key: &str, label: &str| input::text(key, label).placeholder("(keep current)").with("queryLabel", "Get current");
        vec![
            monitor_input(),
            keep("x", "X (from display left)").with("query", json!({"id": "get-window-x", "args": ["monitor"]})),
            keep("y", "Y (from below the menu bar)").with("query", json!({"id": "get-window-y", "args": ["monitor"]})),
            keep("w", "Width").with("query", "get-window-w"),
            keep("h", "Height").with("query", "get-window-h"),
        ]
    }

    // ── Action builders ──────────────────────────────────────────────────────

    /// A button that runs `op` on the focused window.
    fn window_button(id: &str, label: &str, icon: &str, op: fn(&Elem) -> Result) -> Action {
        Action::button(id, label).icon(icon).on_press(move |_| op(&ax::focused_window()?))
    }

    /// A volatile toggle whose state follows the focused window.
    fn window_toggle(id: &str, label: &str, icon: &str, states: [(&'static str, &'static str, &'static str); 2], get: fn(&Elem) -> bool, flip: fn(&Elem) -> Result<&'static str>) -> Action {
        let mut a = Action::toggle(id, label).icon(icon).volatile();
        for (v, l, i) in states {
            a = a.state(v, l, i);
        }
        let (off, on) = (states[0].0, states[1].0);
        a.initial_with(move |_| Some(ax::focused_window().map_or(off, |w| if get(&w) { on } else { off }).to_string())).on_press(move |ctx| {
            ctx.fan_out(flip(&ax::focused_window()?)?);
            Ok(())
        })
    }

    fn snap_button(id: &str, label: &str, icon: &str, region: Region) -> Action {
        Action::button(id, label).icon(icon).inputs(vec![monitor_input()]).on_press(move |ctx| snap(&ax::focused_window()?, &ctx.inputs, region))
    }

    static PICKERS: Mutex<Option<HashMap<String, HashMap<String, Elem>>>> = Mutex::new(None);

    /// Slices for every open window (max 16) with its app's icon; records child id → window under `wheel_id`.
    fn window_slices(wheel_id: &str) -> Result<Vec<Value>> {
        ax::ensure_trusted()?;
        let list = windows::list_open_windows();
        if list.is_empty() {
            return Err(Error::msg("No open windows found"));
        }
        let mut map = HashMap::new();
        let slices = list
            .into_iter()
            .take(16)
            .enumerate()
            .map(|(i, w)| {
                let child = format!("w{i}");
                let icon = w.app.bundle.as_deref().and_then(icon::app_icon).unwrap_or_else(|| "app-window".into());
                let label = if w.title.is_empty() { w.app.name.clone() } else { format!("{} — {}", w.title, w.app.name) };
                let label = if w.elem.is_minimized() { format!("{label} (minimized)") } else { label };
                map.insert(child.clone(), w.elem);
                json!({"id": child, "label": label, "icon": icon})
            })
            .collect();
        PICKERS.lock().unwrap().get_or_insert_with(HashMap::new).insert(wheel_id.to_string(), map);
        Ok(slices)
    }

    fn picked_window(wheel_id: &str, ctx: &Ctx) -> Option<Elem> {
        PICKERS.lock().unwrap().as_ref()?.get(wheel_id)?.get(ctx.child_id.as_deref()?).cloned()
    }

    /// A wheel listing every open window; the chosen one gets `on_select` applied.
    fn picker_wheel(id: &'static str, label: &str, icon: &str, inputs: Vec<Value>, on_select: fn(&Ctx, &Elem) -> Result) -> Action {
        Action::wheel(id, label)
            .icon(icon)
            .inputs(inputs)
            .on_open(move |ctx| window_slices(id).or_else(|e| ctx.fail(e.message().to_string())))
            .on_child_press(move |ctx| {
                let Some(w) = picked_window(id, ctx) else {
                    ctx.set_error("That window is no longer available");
                    return Ok(());
                };
                on_select(ctx, &w)
            })
    }

    fn snap_picker(id: &'static str, label: &str, icon: &str, region: Region) -> Action {
        Action::wheel(id, label)
            .icon(icon)
            .inputs(vec![monitor_input()])
            .on_open(move |ctx| window_slices(id).or_else(|e| ctx.fail(e.message().to_string())))
            .on_child_press(move |ctx| match picked_window(id, ctx) {
                Some(w) => snap(&w, &ctx.inputs, region),
                None => {
                    ctx.set_error("That window is no longer available");
                    Ok(())
                }
            })
    }

    /// A button that presses a shortcut through the host. Last resort only:
    /// use it when no OS call does the job (see CLAUDE.md).
    fn keys_button(id: &str, label: &str, icon: &str, keys: &'static str) -> Action {
        Action::button(id, label).icon(icon).on_press(move |_| arclet_sdk::send_keys(keys))
    }

    /// `open -b <bundle id>`: launches the app or brings it forward.
    fn open_bundle(bundle_id: &str) -> Result {
        let out = Command::new("/usr/bin/open").args(["-b", bundle_id]).output()?;
        if !out.status.success() {
            return Err(Error::msg(String::from_utf8_lossy(&out.stderr).trim().to_string()));
        }
        Ok(())
    }

    fn open_button(id: &str, label: &str, icon: &str, bundle_id: &'static str) -> Action {
        Action::button(id, label).icon(icon).on_press(move |_| open_bundle(bundle_id))
    }

    // ── Audio devices ────────────────────────────────────────────────────────

    static DEVICE_IDS: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

    fn device_wheel(id: &'static str, label: &str, icon: &'static str, input: bool) -> Action {
        Action::wheel(id, label)
            .icon(icon)
            .on_open(move |ctx| {
                let list = audio::devices(input);
                if list.is_empty() {
                    return ctx.fail(if input { "No microphones found" } else { "No output devices found" });
                }
                let current = audio::default_device(input);
                let mut map = DEVICE_IDS.lock().unwrap();
                let map = map.get_or_insert_with(HashMap::new);
                map.retain(|k, _| !k.starts_with(id));
                Ok(list
                    .into_iter()
                    .take(16)
                    .enumerate()
                    .map(|(i, d)| {
                        let child = format!("{id}-{i}");
                        map.insert(child.clone(), d.id);
                        let icon = if Some(d.id) == current { "circle-check" } else { icon };
                        json!({"id": child, "label": d.name, "icon": icon})
                    })
                    .collect())
            })
            .on_child_press(move |ctx| {
                let dev = DEVICE_IDS.lock().unwrap().as_ref().and_then(|m| m.get(ctx.child_id.as_deref().unwrap_or("")).copied());
                match dev {
                    Some(d) => audio::set_default(input, d),
                    None => ctx.fail("That device is no longer available"),
                }
            })
    }

    static CLIPBOARD_IDS: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

    pub fn run() -> ! {
        let mut p = Plugin::new("mac-tools", "Mac Tools")
            .version("1.0.0")
            .author("Arclet")
            .description("macOS window management (snap, fill, full screen, displays), window switcher, Spaces and system shortcuts, live stats, audio devices, mic mute and clipboard history.")
            .icon("app-window")
            .platforms(&["macos"])
            // Several toggles derive their state from the focused window.
            .subscribe(&["focus_changed"])
            .on_ready(|_| {
                clipboard::start();
                Ok(())
            })
            // ── Queries ──
            .query("get-monitors", |ctx| {
                ctx.log("query get-monitors");
                let mut opts = vec![json!({"value": "current", "label": "Current Display (where the window is)"})];
                for (i, d) in screens::list().iter().enumerate() {
                    opts.push(json!({"value": i.to_string(), "label": display_label(d, i)}));
                }
                Ok(Value::Array(opts))
            })
            .query("get-window-x", |ctx| {
                let f = ax::focused_window()?.frame()?;
                Ok(json!((f.x - resolve_display(&f, &str_input(&ctx.inputs, "monitor"))?.work.x).round().to_string()))
            })
            .query("get-window-y", |ctx| {
                let f = ax::focused_window()?.frame()?;
                Ok(json!((f.y - resolve_display(&f, &str_input(&ctx.inputs, "monitor"))?.work.y).round().to_string()))
            })
            .query("get-window-w", |_| Ok(json!(ax::focused_window()?.frame()?.w.round().to_string())))
            .query("get-window-h", |_| Ok(json!(ax::focused_window()?.frame()?.h.round().to_string())))
            // ── Current Window ──
            .action(window_button("minimize", "Minimize", "minimize.svg", minimize))
            .action(window_button("maximize", "Fill Screen", "maximize.svg", fill))
            .action(window_button("restore", "Restore", "restore.svg", restore))
            .action(window_button("close-window", "Close Window", "close.svg", |w| w.close()))
            .action(window_button("bring-to-front", "Bring to Front", "front.svg", |w| w.bring_to_front()))
            .action(window_button("hide-app", "Hide App", "eye-off", hide_app))
            .action(window_toggle("toggle-max-restore", "Fill / Restore", "maxrestore.svg", [("normal", "Windowed", "minimize-2"), ("maximized", "Filled", "maximize-2")], is_filled, toggle_fill))
            .action(window_toggle("toggle-fullscreen", "Full Screen", "fullscreen.svg", [("windowed", "Windowed", "minimize-2"), ("fullscreen", "Full Screen", "maximize")], Elem::is_fullscreen, toggle_fullscreen))
            // ── Switch Window ──
            .action(
                Action::wheel("switch-window-wheel", "Switch to Window")
                    .icon("switch-app.svg")
                    .on_open(|ctx| window_slices("switch-window-wheel").or_else(|e| ctx.fail(e.message().to_string())))
                    .on_child_press(|ctx| match picked_window("switch-window-wheel", ctx) {
                        Some(w) => w.bring_to_front(),
                        None => {
                            ctx.set_error("That window is no longer available");
                            Ok(())
                        }
                    }),
            )
            // ── Position ──
            .action(window_button("move-next-monitor", "Move to Next Display", "monitor.svg", move_to_next_display))
            .action(center_button("center-window", "Center Window"))
            .action(snap_button("move-left-half", "Move Left Half", "snap-left.svg", reg(0.0, 0.0, 0.5, 1.0)))
            .action(snap_button("move-right-half", "Move Right Half", "snap-right.svg", reg(0.5, 0.0, 0.5, 1.0)))
            .action(snap_button("move-top-half", "Move Top Half", "snap-top.svg", reg(0.0, 0.0, 1.0, 0.5)))
            .action(snap_button("move-bottom-half", "Move Bottom Half", "snap-bottom.svg", reg(0.0, 0.5, 1.0, 0.5)))
            .action(snap_button("move-top-left", "Move Top Left", "snap-topleft.svg", reg(0.0, 0.0, 0.5, 0.5)))
            .action(snap_button("move-top-right", "Move Top Right", "snap-topright.svg", reg(0.5, 0.0, 0.5, 0.5)))
            .action(snap_button("move-bottom-left", "Move Bottom Left", "snap-bottomleft.svg", reg(0.0, 0.5, 0.5, 0.5)))
            .action(snap_button("move-bottom-right", "Move Bottom Right", "snap-bottomright.svg", reg(0.5, 0.5, 0.5, 0.5)))
            .action(snap_button("move-left-third", "Move Left Third", "snap-left.svg", reg(0.0, 0.0, 1.0 / 3.0, 1.0)))
            .action(snap_button("move-center-third", "Move Center Third", "center.svg", reg(1.0 / 3.0, 0.0, 1.0 / 3.0, 1.0)))
            .action(snap_button("move-right-third", "Move Right Third", "snap-right.svg", reg(2.0 / 3.0, 0.0, 1.0 / 3.0, 1.0)))
            .action(snap_button("fill-display", "Fill Display", "fullscreen.svg", reg(0.0, 0.0, 1.0, 1.0)))
            .action(Action::button("custom-position", "Custom Position").icon("move.svg").inputs(custom_position_inputs()).on_press(|ctx| apply_custom_position(&ax::focused_window()?, &ctx.inputs)));

        // ── Dynamic Window Actions: window-picker wheels ──
        p = p
            .action(picker_wheel("dynamic-minimize-wheel", "Dynamic Minimize", "minimize.svg", vec![], |_, w| minimize(w)))
            .action(picker_wheel("dynamic-maximize-wheel", "Dynamic Fill Screen", "maximize.svg", vec![], |_, w| fill(w)))
            .action(picker_wheel("dynamic-restore-wheel", "Dynamic Restore", "restore.svg", vec![], |_, w| restore(w)))
            .action(picker_wheel("dynamic-close-wheel", "Dynamic Close", "close.svg", vec![], |_, w| w.close()))
            .action(picker_wheel("dynamic-fullscreen-wheel", "Dynamic Full Screen", "fullscreen.svg", vec![], |_, w| toggle_fullscreen(w).map(|_| ())))
            .action(picker_wheel("dynamic-max-restore-wheel", "Dynamic Fill / Restore", "maxrestore.svg", vec![], |_, w| toggle_fill(w).map(|_| ())))
            .action(picker_wheel("dynamic-hide-app-wheel", "Dynamic Hide App", "eye-off", vec![], |_, w| hide_app(w)))
            .action(picker_wheel("dynamic-move-next-monitor-wheel", "Dynamic Move to Next Display", "monitor.svg", vec![], |_, w| move_to_next_display(w)))
            .action(picker_wheel("dynamic-center-window-wheel", "Dynamic Center Window", "center.svg", vec![monitor_input()], |ctx, w| {
                center_in(w, resolve_display(&w.frame()?, &str_input(&ctx.inputs, "monitor"))?.work)
            }))
            .action(snap_picker("dynamic-move-left-half-wheel", "Dynamic Move Left Half", "snap-left.svg", reg(0.0, 0.0, 0.5, 1.0)))
            .action(snap_picker("dynamic-move-right-half-wheel", "Dynamic Move Right Half", "snap-right.svg", reg(0.5, 0.0, 0.5, 1.0)))
            .action(snap_picker("dynamic-move-top-half-wheel", "Dynamic Move Top Half", "snap-top.svg", reg(0.0, 0.0, 1.0, 0.5)))
            .action(snap_picker("dynamic-move-bottom-half-wheel", "Dynamic Move Bottom Half", "snap-bottom.svg", reg(0.0, 0.5, 1.0, 0.5)))
            .action(snap_picker("dynamic-move-top-left-wheel", "Dynamic Move Top Left", "snap-topleft.svg", reg(0.0, 0.0, 0.5, 0.5)))
            .action(snap_picker("dynamic-move-top-right-wheel", "Dynamic Move Top Right", "snap-topright.svg", reg(0.5, 0.0, 0.5, 0.5)))
            .action(snap_picker("dynamic-move-bottom-left-wheel", "Dynamic Move Bottom Left", "snap-bottomleft.svg", reg(0.0, 0.5, 0.5, 0.5)))
            .action(snap_picker("dynamic-move-bottom-right-wheel", "Dynamic Move Bottom Right", "snap-bottomright.svg", reg(0.5, 0.5, 0.5, 0.5)))
            .action(snap_picker("dynamic-fill-display-wheel", "Dynamic Fill Display", "fullscreen.svg", reg(0.0, 0.0, 1.0, 1.0)))
            .action(picker_wheel("dynamic-custom-position-wheel", "Dynamic Custom Position", "move.svg", custom_position_inputs(), |ctx, w| apply_custom_position(w, &ctx.inputs)));

        // ── Spaces & System (macOS shortcuts and apps) ──
        p = p
            // OS entry points first (system.rs); keystrokes only where no API exists.
            .action(Action::button("desktop-previous", "Previous Space").icon("chevrons-left").on_press(|_| system::system_hotkey(Hotkey::SpaceLeft)))
            .action(Action::button("desktop-next", "Next Space").icon("chevrons-right").on_press(|_| system::system_hotkey(Hotkey::SpaceRight)))
            .action(Action::button("mission-control", "Mission Control").icon("layout-grid").on_press(|_| system::dock(Dock::MissionControl)))
            .action(Action::button("app-windows", "App Exposé").icon("app-window").on_press(|_| system::dock(Dock::AppExpose)))
            .action(Action::button("show-desktop", "Show Desktop").icon("monitor").on_press(|_| system::dock(Dock::ShowDesktop)))
            .action(Action::button("spotlight", "Spotlight").icon("search").on_press(|_| system::system_hotkey(Hotkey::Spotlight)))
            .action(open_button("screen-snip", "Screenshot", "scissors", "com.apple.screenshot.launcher"))
            .action(Action::button("emoji-panel", "Emoji & Symbols").icon("smile").on_press(|_| system::character_viewer()))
            .action(Action::button("hide-others", "Hide Others").icon("eye-off").on_press(|_| system::hide_others()))
            // No API opens the Force Quit window; ⌥⌘Esc is reserved by loginwindow, so no app can capture it.
            .action(keys_button("force-quit", "Force Quit…", "circle-x", "cmd alt esc"))
            .action(Action::button("lock-screen", "Lock Screen").icon("lock").on_press(|_| system::lock_screen()))
            .action(open_button("finder", "Finder", "folder-open", "com.apple.finder"))
            .action(open_button("open-settings", "System Settings", "settings", "com.apple.systempreferences"))
            .action(open_button("activity-monitor", "Activity Monitor", "activity", "com.apple.ActivityMonitor"))
            .action(Action::button("sleep-display", "Sleep Display").icon("moon").on_press(|_| {
                Command::new("/usr/bin/pmset").arg("displaysleepnow").status()?;
                Ok(())
            }))
            // A button (opens Activity Monitor) whose tile shows the chosen stat live.
            .action(
                open_button("system-stats", "System Stats", "gauge", "com.apple.ActivityMonitor")
                    .inputs(vec![input::select("stat", "Show", &[("cpu", "CPU"), ("gpu", "GPU"), ("memory", "Memory")]).with("default", "cpu")])
                    .live(std::time::Duration::from_secs(1), |ic| {
                        let (name, pct) = stats::read(&ic.input_str("stat"))?;
                        Ok(match pct {
                            Some(p) => Visual::new().label(format!("{name} {p:.0}%")).icon(stats::gauge(p)),
                            // CPU warms up over one sample; a GPU without IOKit stats stays unavailable.
                            None if name == "CPU" => Visual::new().label(format!("{name} …")),
                            None => Visual::new().label(format!("{name} n/a")),
                        })
                    }),
            );

        // ── Audio ──
        p = p
            .action(
                Action::toggle("toggle-mic-mute", "Mute Microphone")
                    .icon("mic")
                    .volatile()
                    .state("unmuted", "Mic On", "mic")
                    .state("muted", "Mic Muted", "mic-off")
                    .initial_with(|_| Some((if audio::mic_muted().unwrap_or(false) { "muted" } else { "unmuted" }).into()))
                    .on_press(|ctx| {
                        let muted = audio::toggle_mic_mute()?;
                        ctx.fan_out(if muted { "muted" } else { "unmuted" });
                        Ok(())
                    }),
            )
            .action(device_wheel("output-device-wheel", "Output Device", "speaker", false))
            .action(device_wheel("input-device-wheel", "Input Device", "mic", true));

        // ── Clipboard History ──
        p.action(
            Action::wheel("clipboard-history-wheel", "Clipboard History")
                .icon("clipboard.svg")
                .on_open(|ctx| {
                    clipboard::start();
                    let items = clipboard::list();
                    if items.is_empty() {
                        return ctx.fail("Clipboard history is empty — copy something first (history is kept while Arclet runs)");
                    }
                    let mut map = HashMap::new();
                    let slices = items
                        .iter()
                        .enumerate()
                        .map(|(i, e)| {
                            let child = format!("c{i}");
                            map.insert(child.clone(), e.id);
                            json!({"id": child, "label": e.label, "icon": e.icon})
                        })
                        .collect();
                    *CLIPBOARD_IDS.lock().unwrap() = Some(map);
                    Ok(slices)
                })
                .on_child_press(|ctx| {
                    let entry = CLIPBOARD_IDS.lock().unwrap().as_ref().and_then(|m| m.get(ctx.child_id.as_deref().unwrap_or("")).copied()).and_then(clipboard::get);
                    let Some(entry) = entry else {
                        ctx.set_error("That clipboard item is no longer available");
                        return Ok(());
                    };
                    clipboard::restore(&entry)?;
                    // Paste into whatever has focus (the dial never takes it), through its Edit ▸ Paste.
                    system::paste()
                }),
        )
        .run()
    }

    fn center_button(id: &str, label: &str) -> Action {
        Action::button(id, label).icon("center.svg").inputs(vec![monitor_input()]).on_press(|ctx| {
            let w = ax::focused_window()?;
            center_in(&w, resolve_display(&w.frame()?, &str_input(&ctx.inputs, "monitor"))?.work)
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn regions_round_to_points() {
            let work = Rect { x: 0.0, y: 25.0, w: 1512.0, h: 920.0 };
            assert_eq!(region_rect(&work, reg(0.5, 0.0, 0.5, 1.0)), Rect { x: 756.0, y: 25.0, w: 756.0, h: 920.0 });
            assert_eq!(region_rect(&work, reg(0.0, 0.5, 0.5, 0.5)), Rect { x: 0.0, y: 485.0, w: 756.0, h: 460.0 });
        }

        #[test]
        fn filled_allows_grid_slack() {
            let work = Rect { x: 0.0, y: 31.0, w: 1920.0, h: 959.0 };
            assert!(covers(&Rect { x: 0.0, y: 31.0, w: 1920.0, h: 949.0 }, &work));
            assert!(!covers(&Rect { x: 0.0, y: 31.0, w: 961.0, h: 484.0 }, &work));
            assert!(!covers(&Rect { x: 100.0, y: 31.0, w: 1820.0, h: 959.0 }, &work));
        }

        #[test]
        fn parse_int_takes_leading_number() {
            assert_eq!(parse_int(" 120px"), Some(120));
            assert_eq!(parse_int("-5"), Some(-5));
            assert_eq!(parse_int("x"), None);
        }

        /// Needs Accessibility for the test runner: `cargo test -p mac-tools -- --ignored --nocapture`.
        #[test]
        #[ignore]
        fn live_windows_smoke() {
            ax::ensure_trusted().expect("grant Accessibility to the terminal running the tests");
            for w in windows::list_open_windows() {
                eprintln!("{:?} | {} | {:?} | min={} fs={}", w.app.name, w.title, w.elem.frame(), w.elem.is_minimized(), w.elem.is_fullscreen());
            }
            let f = ax::focused_window().expect("focused window");
            eprintln!("focused: {} {:?} filled={}", f.title(), f.frame(), is_filled(&f));
        }

        /// Moves, resizes, fills and closes a window titled `arclet-ax-test` (create one first).
        #[test]
        #[ignore]
        fn live_window_ops() {
            let w = windows::list_open_windows().into_iter().find(|w| w.title.contains("arclet-ax-test")).expect("arclet-ax-test window").elem;
            let work = work_area(&w).unwrap();
            snap(&w, &json!({"monitor": "current"}), reg(0.0, 0.0, 0.5, 0.5)).unwrap();
            let f = w.frame().unwrap();
            eprintln!("top-left quarter: {f:?} (work {work:?})");
            assert!((f.x - work.x).abs() <= 4.0 && (f.y - work.y).abs() <= 4.0, "{f:?}");
            fill(&w).unwrap();
            assert!(is_filled(&w), "{:?}", w.frame());
            assert_eq!(toggle_fill(&w).unwrap(), "normal");
            assert!(w.frame().unwrap().approx(&f), "restored {:?}", w.frame());
            center_in(&w, work).unwrap();
            minimize(&w).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(800));
            assert!(w.is_minimized());
            restore(&w).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(800));
            assert!(!w.is_minimized());
            w.close().unwrap();
        }

        #[test]
        fn displays_have_work_areas_inside_frames() {
            let list = screens::list();
            assert!(!list.is_empty());
            assert!(list[0].primary);
            for d in &list {
                assert!(d.work.w <= d.frame.w && d.work.h <= d.frame.h && d.work.y >= d.frame.y, "{d:?}");
                eprintln!("{d:?}");
            }
        }
    }
}
