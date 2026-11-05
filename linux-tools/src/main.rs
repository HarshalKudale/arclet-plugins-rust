//! linux-tools: window management, workspaces, audio, clipboard history and system actions for
//! KnobDeck on Linux, on **KDE Plasma 6** and **GNOME 45+** (Wayland and X11), on Debian/Ubuntu
//! and Arch based distributions alike. The Linux counterpart of Win Tools and Mac Tools.
//!
//! Windows go through the compositor (`linux-desktop`: KWin scripting, the KnobDeck GNOME
//! extension); system actions through D-Bus (KGlobalAccel, KRunner, Klipper, logind,
//! NetworkManager, BlueZ, power-profiles-daemon); audio through `pactl`. See CLAUDE.md for the
//! one keystroke left (Ctrl+V to paste).

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("linux-tools only runs on Linux");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
mod audio;
#[cfg(target_os = "linux")]
mod clipboard;
#[cfg(target_os = "linux")]
mod stats;
#[cfg(target_os = "linux")]
mod system;

#[cfg(target_os = "linux")]
fn main() {
    app::run()
}

#[cfg(target_os = "linux")]
mod app {
    use crate::{audio, clipboard, stats, system};
    use knobdeck_sdk::input::{self, InputExt};
    use knobdeck_sdk::{json, log, Action, Ctx, Error, Plugin, Result, Value, Visual};
    use linux_desktop::wm::{self, Output, Rect, Window};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Duration;

    // ── Inputs ──────────────────────────────────────────────────────────────

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

    fn monitor_input() -> Value {
        input::query_select("monitor", "Monitor", "get-monitors", &[]).with("default", "current")
    }

    // ── Geometry ────────────────────────────────────────────────────────────

    #[derive(Clone, Copy)]
    struct Region {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }

    const fn reg(x: f64, y: f64, w: f64, h: f64) -> Region {
        Region { x, y, w, h }
    }

    fn region_rect(work: Rect, r: Region) -> Rect {
        let x0 = (work.x + work.w * r.x).round();
        let y0 = (work.y + work.h * r.y).round();
        let x1 = (work.x + work.w * (r.x + r.w)).round();
        let y1 = (work.y + work.h * (r.y + r.h)).round();
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }

    /// The output picked by the `monitor` input: "current" (the window's) or an index.
    fn resolve_output(w: &Window, inputs: &Value) -> Result<Output> {
        let outs = wm::outputs()?;
        if outs.is_empty() {
            return Err(Error::msg("No displays found"));
        }
        let pick = str_input(inputs, "monitor");
        let idx = match pick.parse::<usize>() {
            Ok(i) if i < outs.len() => i,
            Ok(_) => return Err(Error::msg("That display is no longer connected")),
            Err(_) => wm::output_of(w, &outs).unwrap_or(0),
        };
        Ok(outs[idx].clone())
    }

    fn snap(id: Option<&str>, inputs: &Value, region: Region) -> Result {
        let w = wm::info(id)?;
        let out = resolve_output(&w, inputs)?;
        wm::move_to(Some(&w.id), region_rect(out.work, region))
    }

    fn center(id: Option<&str>, inputs: &Value) -> Result {
        let w = wm::info(id)?;
        let work = resolve_output(&w, inputs)?.work;
        let (cw, ch) = (w.frame.w.min(work.w), w.frame.h.min(work.h));
        wm::move_to(Some(&w.id), Rect { x: work.x + (work.w - cw) / 2.0, y: work.y + (work.h - ch) / 2.0, w: cw, h: ch })
    }

    fn move_to_next_output(id: Option<&str>) -> Result {
        let w = wm::info(id)?;
        let outs = wm::outputs()?;
        if outs.len() < 2 {
            return Err(Error::msg("Only one display is connected"));
        }
        let cur = wm::output_of(&w, &outs).unwrap_or(0);
        wm::to_output(Some(&w.id), (cur + 1) % outs.len())
    }

    /// X/Y relative to the chosen display's work area; blank or invalid keeps the current value.
    fn apply_custom_position(id: Option<&str>, inputs: &Value) -> Result {
        let w = wm::info(id)?;
        let work = resolve_output(&w, inputs)?.work;
        let x = input_int(inputs, "x").map_or(w.frame.x, |v| work.x + v as f64);
        let y = input_int(inputs, "y").map_or(w.frame.y, |v| work.y + v as f64);
        let width = input_int(inputs, "w").filter(|v| *v > 0).map_or(w.frame.w, |v| v as f64);
        let height = input_int(inputs, "h").filter(|v| *v > 0).map_or(w.frame.h, |v| v as f64);
        wm::move_to(Some(&w.id), Rect { x, y, w: width, h: height })
    }

    fn custom_position_inputs() -> Vec<Value> {
        let keep = |key: &str, label: &str| input::text(key, label).placeholder("(keep current)").with("queryLabel", "Get current");
        vec![
            monitor_input(),
            keep("x", "X (from monitor left)").with("query", json!({"id": "get-window-x", "args": ["monitor"]})),
            keep("y", "Y (from monitor top)").with("query", json!({"id": "get-window-y", "args": ["monitor"]})),
            keep("w", "Width").with("query", "get-window-w"),
            keep("h", "Height").with("query", "get-window-h"),
        ]
    }

    fn region_of(name: &str) -> Option<Region> {
        Some(match name {
            "left-half" => reg(0.0, 0.0, 0.5, 1.0),
            "right-half" => reg(0.5, 0.0, 0.5, 1.0),
            "top-half" => reg(0.0, 0.0, 1.0, 0.5),
            "bottom-half" => reg(0.0, 0.5, 1.0, 0.5),
            "top-left" => reg(0.0, 0.0, 0.5, 0.5),
            "top-right" => reg(0.5, 0.0, 0.5, 0.5),
            "bottom-left" => reg(0.0, 0.5, 0.5, 0.5),
            "bottom-right" => reg(0.5, 0.5, 0.5, 0.5),
            "left-third" => reg(0.0, 0.0, 1.0 / 3.0, 1.0),
            "center-third" => reg(1.0 / 3.0, 0.0, 1.0 / 3.0, 1.0),
            "right-third" => reg(2.0 / 3.0, 0.0, 1.0 / 3.0, 1.0),
            "fill" => reg(0.0, 0.0, 1.0, 1.0),
            _ => return None,
        })
    }

    fn move_app_inputs() -> Vec<Value> {
        let custom = json!({"key": "position", "equals": "custom"});
        let keep = |key: &str, label: &str| input::text(key, label).placeholder("(keep current)").with("showIf", custom.clone());
        vec![
            input::query_combobox("app", "App", "get-open-apps"),
            input::select(
                "position",
                "Position",
                &[
                    ("left-half", "Left half"),
                    ("right-half", "Right half"),
                    ("top-half", "Top half"),
                    ("bottom-half", "Bottom half"),
                    ("top-left", "Top left"),
                    ("top-right", "Top right"),
                    ("bottom-left", "Bottom left"),
                    ("bottom-right", "Bottom right"),
                    ("left-third", "Left third"),
                    ("center-third", "Center third"),
                    ("right-third", "Right third"),
                    ("fill", "Fill display"),
                    ("center", "Center"),
                    ("maximize", "Maximize"),
                    ("minimize", "Minimize"),
                    ("front", "Bring to front only"),
                    ("custom", "Custom…"),
                ],
            )
            .with("default", "left-half"),
            monitor_input(),
            keep("x", "X (from monitor left)"),
            keep("y", "Y (from monitor top)"),
            keep("w", "Width"),
            keep("h", "Height"),
            input::checkbox("focus", "Bring it to the front"),
        ]
    }

    // ── Window wheels ───────────────────────────────────────────────────────

    /// Wheel id → (slice id → window id).
    static PICKERS: Mutex<Option<HashMap<String, HashMap<String, String>>>> = Mutex::new(None);
    /// App (desktop id or class) → icon data URI, so a wheel doesn't rescan the icon themes.
    static ICONS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

    fn app_icon(w: &Window) -> String {
        let key = format!("{}|{}", w.desktop_file, w.app);
        if let Some(i) = ICONS.lock().unwrap().as_ref().and_then(|m| m.get(&key).cloned()) {
            return i;
        }
        let icon = linux_desktop::apps::find(&w.desktop_file, &w.app).and_then(|e| linux_desktop::apps::icon_data_uri(&e.icon)).unwrap_or_else(|| "app-window".into());
        ICONS.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, icon.clone());
        icon
    }

    fn window_slices(wheel_id: &str) -> Result<Vec<Value>> {
        let windows = wm::list()?;
        if windows.is_empty() {
            return Err(Error::msg("No open windows found"));
        }
        let mut map = HashMap::new();
        let slices = windows
            .iter()
            .take(16)
            .enumerate()
            .map(|(i, w)| {
                let child = format!("w{i}");
                map.insert(child.clone(), w.id.clone());
                let label = if w.title.is_empty() { w.app_name() } else { w.title.clone() };
                let label = if w.minimized { format!("{label} (minimized)") } else { label };
                json!({"id": child, "label": label, "icon": app_icon(w)})
            })
            .collect();
        PICKERS.lock().unwrap().get_or_insert_with(HashMap::new).insert(wheel_id.to_string(), map);
        Ok(slices)
    }

    fn picked_window(wheel_id: &str, ctx: &Ctx) -> Option<String> {
        PICKERS.lock().unwrap().as_ref()?.get(wheel_id)?.get(ctx.child_id.as_deref()?).cloned()
    }

    /// A wheel listing every open window; the chosen one gets `on_select` applied.
    fn picker_wheel(id: &'static str, label: &str, icon: &str, inputs: Vec<Value>, on_select: fn(&Ctx, &str) -> Result) -> Action {
        Action::wheel(id, label)
            .icon(icon)
            .inputs(inputs)
            .on_open(move |ctx| window_slices(id).or_else(|e| ctx.fail(e.message().to_string())))
            .on_child_press(move |ctx| match picked_window(id, ctx) {
                Some(w) => on_select(ctx, &w),
                None => {
                    ctx.set_error("That window is no longer available");
                    Ok(())
                }
            })
    }

    fn snap_picker(id: &'static str, label: &str, icon: &str, region: Region) -> Action {
        Action::wheel(id, label)
            .icon(icon)
            .inputs(vec![monitor_input()])
            .on_open(move |ctx| window_slices(id).or_else(|e| ctx.fail(e.message().to_string())))
            .on_child_press(move |ctx| match picked_window(id, ctx) {
                Some(w) => snap(Some(&w), &ctx.inputs, region),
                None => {
                    ctx.set_error("That window is no longer available");
                    Ok(())
                }
            })
    }

    // ── Action builders ─────────────────────────────────────────────────────

    /// A button that runs a bridge command on the focused window.
    fn window_button(id: &str, label: &str, icon: &str, cmd: &'static str) -> Action {
        Action::button(id, label).icon(icon).on_press(move |_| wm::op(cmd, None))
    }

    /// A volatile toggle whose state follows the focused window.
    fn window_toggle(id: &str, label: &str, icon: &str, states: [(&'static str, &'static str, &'static str); 2], get: fn(&Window) -> bool, cmd: &'static str) -> Action {
        let mut a = Action::toggle(id, label).icon(icon).volatile();
        for (v, l, i) in states {
            a = a.state(v, l, i);
        }
        a.initial_with(move |_| Some(states[usize::from(wm::active().is_ok_and(|w| get(&w)))].0.to_string())).on_press(move |ctx| {
            let on = wm::toggle(cmd, None)?;
            ctx.fan_out(states[usize::from(on)].0);
            Ok(())
        })
    }

    fn snap_button(id: &str, label: &str, icon: &str, region: Region) -> Action {
        Action::button(id, label).icon(icon).inputs(vec![monitor_input()]).on_press(move |ctx| snap(None, &ctx.inputs, region))
    }

    fn button(id: &str, label: &str, icon: &str, f: fn() -> Result) -> Action {
        Action::button(id, label).icon(icon).on_press(move |_| f())
    }

    // ── Opacity ──

    fn opacity_preset_inputs() -> Vec<Value> {
        let opts = [("100", "100% — Fully Opaque"), ("90", "90%"), ("80", "80%"), ("75", "75%"), ("60", "60%"), ("50", "50%"), ("40", "40%"), ("30", "30%"), ("20", "20%"), ("10", "10%"), ("__custom__", "Custom…")];
        vec![
            input::select("preset", "Opacity", &opts).with("default", "75"),
            input::text("custom", "Custom opacity (10–100)").placeholder("75").with("showIf", json!({"key": "preset", "equals": "__custom__"})),
        ]
    }

    fn transparency_opacity_input() -> Value {
        input::select("opacity", "Opacity when enabled", &[("90", "90%"), ("80", "80%"), ("75", "75% (default)"), ("60", "60%"), ("50", "50%"), ("30", "30%")]).with("default", "75")
    }

    fn preset_percent(inputs: &Value) -> Option<i64> {
        let raw = if inputs["preset"] == "__custom__" { str_input(inputs, "custom") } else { str_input(inputs, "preset") };
        parse_int(&raw).filter(|p| (0..=100).contains(p))
    }

    /// Flip between opaque and the chosen opacity; returns the new state.
    fn toggle_transparency(id: Option<&str>, inputs: &Value) -> Result<&'static str> {
        let w = wm::info(id)?;
        if w.opacity < 0.995 {
            wm::set_opacity(Some(&w.id), 1.0)?;
            Ok("opaque")
        } else {
            wm::set_opacity(Some(&w.id), input_int(inputs, "opacity").unwrap_or(75) as f64 / 100.0)?;
            Ok("transparent")
        }
    }

    // ── Audio ──

    fn app_select_input() -> Value {
        input::query_select("app", "App", "get-audio-apps", &[]).with("default", "__foreground__")
    }

    /// The `app` input as a stream target (default: the focused window's app).
    fn resolve_app(inputs: &Value) -> Result<audio::Target> {
        match inputs["app"].as_str() {
            Some(a) if !a.is_empty() && a != "__foreground__" => Ok(audio::Target::App(a.to_string())),
            _ => {
                let w = wm::active()?;
                Ok(audio::Target::Window { pid: w.pid, class: if w.desktop_file.is_empty() { w.app } else { w.desktop_file } })
            }
        }
    }

    fn no_stream(inputs: &Value) -> String {
        match inputs["app"].as_str() {
            Some(a) if !a.is_empty() && a != "__foreground__" => format!("\"{a}\" is not playing audio"),
            _ => "The focused app is not playing audio".into(),
        }
    }

    fn app_volume_step(id: &str, label: &str, sign: f64) -> Action {
        Action::button(id, label).icon("volume-2").inputs(vec![app_select_input(), input::text("step", "Step (%)").placeholder("5").default_value("5")]).on_press(move |ctx| {
            let step = input_int(&ctx.inputs, "step").unwrap_or(5);
            if audio::adjust_app_volume(&resolve_app(&ctx.inputs)?, step as f64 * sign)?.is_none() {
                ctx.set_error(no_stream(&ctx.inputs));
            }
            Ok(())
        })
    }

    fn mute_state(muted: bool) -> &'static str {
        if muted {
            "muted"
        } else {
            "unmuted"
        }
    }

    fn is_input(inputs: &Value) -> bool {
        inputs["flow"] == "input"
    }

    fn flow_input() -> Value {
        input::select("flow", "Type", &[("output", "Playback (speakers, headphones)"), ("input", "Recording (microphones)")]).with("default", "output")
    }

    fn device_input() -> Value {
        input::query_select("device", "Device", "get-audio-devices", &["flow"]).with("default", audio::DEFAULT_DEVICE)
    }

    fn device_options(inputs: &Value, with_default: bool) -> Result<Value> {
        let mut opts = vec![];
        if with_default {
            opts.push(json!({"value": audio::DEFAULT_DEVICE, "label": "Default device"}));
        }
        opts.extend(audio::devices(is_input(inputs))?.into_iter().map(|d| json!({"value": d.id, "label": d.name})));
        Ok(Value::Array(opts))
    }

    static DEVICE_IDS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

    /// A wheel of devices; pressing one makes it the default.
    fn device_wheel(id: &'static str, label: &str, icon: &'static str, input: bool) -> Action {
        Action::wheel(id, label)
            .icon(icon)
            .on_open(move |ctx| {
                let list = audio::devices(input).or_else(|e| ctx.fail(e.message().to_string()))?;
                if list.is_empty() {
                    return ctx.fail(if input { "No microphones found" } else { "No playback devices found" });
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
                        let icon = if current.as_deref() == Some(d.id.as_str()) { "circle-check" } else { icon };
                        map.insert(child.clone(), d.id);
                        json!({"id": child, "label": d.name, "icon": icon})
                    })
                    .collect())
            })
            .on_child_press(move |ctx| {
                let dev = DEVICE_IDS.lock().unwrap().as_ref().and_then(|m| m.get(ctx.child_id.as_deref().unwrap_or("")).cloned());
                match dev {
                    Some(d) => audio::set_default(input, &d),
                    None => ctx.fail("That device is no longer available"),
                }
            })
    }

    // ── System ──

    fn radio_of(inputs: &Value) -> String {
        if inputs["radio"] == "bluetooth" { "bluetooth" } else { "wifi" }.into()
    }

    fn ping_label(host: &str) -> String {
        match system::ping(host, 2000) {
            Ok(Some(ms)) => format!("{ms} ms"),
            Ok(None) => "Timeout".into(),
            Err(e) => e.message().to_string(),
        }
    }

    static CLIPBOARD_ITEMS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

    /// GNOME needs the KnobDeck Shell extension; install or update it in the background.
    fn prepare_gnome() {
        if linux_desktop::desktop() != linux_desktop::Desktop::Gnome {
            return;
        }
        std::thread::spawn(|| match linux_desktop::gnome::ensure_installed() {
            Ok(linux_desktop::gnome::Install::Running) => log("GNOME extension running"),
            Ok(linux_desktop::gnome::Install::NeedsRelogin) => log("GNOME extension installed; it starts at the next login"),
            Err(e) => log(format!("GNOME extension: {}", e.message())),
        });
    }

    pub fn run() -> ! {
        let mut p = Plugin::new("linux-tools", "Linux Tools")
            .version(env!("CARGO_PKG_VERSION"))
            .author("KnobDeck")
            .description("Window management, workspaces, per-app volume, audio devices, clipboard history and system actions for GNOME and KDE Plasma.")
            .icon("app-window")
            .platforms(&["linux"])
            .subscribe(&["focus_changed"])
            .on_ready(|_| {
                log(format!("desktop: {:?}", linux_desktop::desktop()));
                prepare_gnome();
                Ok(())
            })
            // ── Queries ──
            .query("get-monitors", |_| {
                let mut opts = vec![json!({"value": "current", "label": "Current Monitor (where window is)"})];
                for o in wm::outputs()? {
                    let name = if o.name.is_empty() { String::new() } else { format!(" ({})", o.name) };
                    opts.push(json!({"value": o.index.to_string(), "label": format!("Monitor {}{}{} — {}×{}", o.index + 1, name, if o.primary { " Primary" } else { "" }, o.frame.w, o.frame.h)}));
                }
                Ok(Value::Array(opts))
            })
            .query("get-window-x", |ctx| {
                let w = wm::active()?;
                Ok(json!(((w.frame.x - resolve_output(&w, &ctx.inputs)?.work.x).round() as i64).to_string()))
            })
            .query("get-window-y", |ctx| {
                let w = wm::active()?;
                Ok(json!(((w.frame.y - resolve_output(&w, &ctx.inputs)?.work.y).round() as i64).to_string()))
            })
            .query("get-window-w", |_| Ok(json!((wm::active()?.frame.w.round() as i64).to_string())))
            .query("get-window-h", |_| Ok(json!((wm::active()?.frame.h.round() as i64).to_string())))
            .query("get-open-apps", |_| {
                let mut seen: Vec<(String, String)> = vec![];
                for w in wm::list()? {
                    let key = if w.desktop_file.is_empty() { w.app.clone() } else { w.desktop_file.clone() };
                    if !key.is_empty() && !seen.iter().any(|(k, _)| k.eq_ignore_ascii_case(&key)) {
                        seen.push((key, w.app_name()));
                    }
                }
                Ok(system::options(seen))
            })
            .query("get-audio-apps", |_| {
                let mut opts = vec![json!({"value": "__foreground__", "label": "Current Window (focused app)"})];
                opts.extend(audio::stream_apps()?.into_iter().map(|(v, l)| json!({"value": v, "label": l})));
                Ok(Value::Array(opts))
            })
            .query("get-audio-devices", |ctx| device_options(&ctx.inputs, true))
            .query("get-audio-devices-fixed", |ctx| device_options(&ctx.inputs, false))
            .query("get-power-profiles", |_| Ok(system::options(system::power_profiles()?.into_iter().map(|id| (id.clone(), system::profile_label(&id))).collect())))
            // ── Current window ──
            .action(window_button("minimize", "Minimize", "minus", "minimize"))
            .action(window_button("maximize", "Maximize", "square", "maximize"))
            .action(window_button("restore", "Restore", "copy", "restore"))
            .action(window_button("close-window", "Close Window", "x", "close"))
            .action(window_button("send-to-back", "Send to Back", "send-to-back", "lower"))
            .action(window_toggle("toggle-max-restore", "Maximize / Restore", "maximize-2", [("normal", "Windowed", "minimize-2"), ("maximized", "Maximized", "maximize-2")], |w| w.maximized, "toggle-maximize"))
            .action(window_toggle("toggle-fullscreen", "Full Screen", "fullscreen", [("windowed", "Windowed", "minimize-2"), ("fullscreen", "Full Screen", "fullscreen")], |w| w.fullscreen, "toggle-fullscreen"))
            .action(window_toggle("toggle-always-on-top", "Always on Top", "pin", [("off", "Normal", "circle"), ("on", "Always on Top", "pin")], |w| w.above, "toggle-above"))
            // GNOME doesn't let anyone remove a window's border; the action reports that there.
            .action(window_toggle("toggle-borderless", "Toggle Borderless (Plasma)", "square-dashed", [("bordered", "Bordered", "square"), ("borderless", "Borderless", "square-dashed")], |w| w.no_border, "toggle-border"))
            .action(
                Action::toggle("toggle-transparency", "Toggle Transparency")
                    .icon("blend")
                    .volatile()
                    .state("opaque", "Opaque", "square")
                    .state("transparent", "Transparent", "square-dashed")
                    .inputs(vec![transparency_opacity_input()])
                    .initial_with(|_| Some((if wm::active().is_ok_and(|w| w.opacity < 0.995) { "transparent" } else { "opaque" }).into()))
                    .on_press(|ctx| {
                        ctx.fan_out(toggle_transparency(None, &ctx.inputs)?);
                        Ok(())
                    }),
            )
            .action(Action::button("set-opacity", "Set Opacity").icon("droplet").inputs(opacity_preset_inputs()).on_press(|ctx| match preset_percent(&ctx.inputs) {
                Some(pct) => wm::set_opacity(None, pct as f64 / 100.0),
                None => {
                    ctx.set_error("Opacity must be a number between 10 and 100");
                    Ok(())
                }
            }))
            // ── Position ──
            .action(Action::button("move-next-monitor", "Move to Next Monitor").icon("monitor-up").on_press(|_| move_to_next_output(None)))
            .action(Action::button("center-window", "Center Window").icon("focus").inputs(vec![monitor_input()]).on_press(|ctx| center(None, &ctx.inputs)))
            .action(snap_button("move-left-half", "Move Left Half", "panel-left", reg(0.0, 0.0, 0.5, 1.0)))
            .action(snap_button("move-right-half", "Move Right Half", "panel-right", reg(0.5, 0.0, 0.5, 1.0)))
            .action(snap_button("move-top-half", "Move Top Half", "panel-top", reg(0.0, 0.0, 1.0, 0.5)))
            .action(snap_button("move-bottom-half", "Move Bottom Half", "panel-bottom", reg(0.0, 0.5, 1.0, 0.5)))
            .action(snap_button("move-top-left", "Move Top Left", "move-up-left", reg(0.0, 0.0, 0.5, 0.5)))
            .action(snap_button("move-top-right", "Move Top Right", "move-up-right", reg(0.5, 0.0, 0.5, 0.5)))
            .action(snap_button("move-bottom-left", "Move Bottom Left", "move-down-left", reg(0.0, 0.5, 0.5, 0.5)))
            .action(snap_button("move-bottom-right", "Move Bottom Right", "move-down-right", reg(0.5, 0.5, 0.5, 0.5)))
            .action(snap_button("move-left-third", "Move Left Third", "panel-left", reg(0.0, 0.0, 1.0 / 3.0, 1.0)))
            .action(snap_button("move-center-third", "Move Center Third", "focus", reg(1.0 / 3.0, 0.0, 1.0 / 3.0, 1.0)))
            .action(snap_button("move-right-third", "Move Right Third", "panel-right", reg(2.0 / 3.0, 0.0, 1.0 / 3.0, 1.0)))
            .action(snap_button("fill-display", "Fill Display", "fullscreen", reg(0.0, 0.0, 1.0, 1.0)))
            .action(Action::button("custom-position", "Custom Position").icon("move").inputs(custom_position_inputs()).on_press(|ctx| apply_custom_position(None, &ctx.inputs)))
            .action(Action::button("move-app-window", "Move App Window").icon("app-window").inputs(move_app_inputs()).on_press(|ctx| {
                let app = ctx.input_str("app");
                if app.is_empty() {
                    return ctx.fail("Choose an app first");
                }
                let Some(w) = wm::list()?.into_iter().find(|w| wm::app_matches(w, &app)) else {
                    return ctx.fail(format!("{app} has no open window"));
                };
                let id = Some(w.id.as_str());
                match ctx.input_str("position").as_str() {
                    "minimize" => return wm::op("minimize", id),
                    "maximize" => wm::op("maximize", id)?,
                    "center" => center(id, &ctx.inputs)?,
                    "custom" => apply_custom_position(id, &ctx.inputs)?,
                    "front" => {}
                    region => match region_of(region) {
                        Some(r) => snap(id, &ctx.inputs, r)?,
                        None => return ctx.fail("Choose a position"),
                    },
                }
                if ctx.input_bool("focus") || ctx.input_str("position") == "front" {
                    wm::op("activate", id)?;
                }
                Ok(())
            }))
            // ── Switch Window ──
            .action(
                Action::wheel("switch-window-wheel", "Switch to Window")
                    .icon("layout-grid")
                    .on_open(|ctx| window_slices("switch-window-wheel").or_else(|e| ctx.fail(e.message().to_string())))
                    .on_child_press(|ctx| match picked_window("switch-window-wheel", ctx) {
                        Some(w) => wm::op("activate", Some(&w)),
                        None => ctx.fail("That window is no longer available"),
                    }),
            );

        // ── Dynamic Window Actions: window-picker wheels ──
        p = p
            .action(picker_wheel("dynamic-minimize-wheel", "Dynamic Minimize", "minus", vec![], |_, w| wm::op("minimize", Some(w))))
            .action(picker_wheel("dynamic-maximize-wheel", "Dynamic Maximize", "square", vec![], |_, w| wm::op("maximize", Some(w))))
            .action(picker_wheel("dynamic-restore-wheel", "Dynamic Restore", "copy", vec![], |_, w| wm::op("restore", Some(w))))
            .action(picker_wheel("dynamic-close-wheel", "Dynamic Close", "x", vec![], |_, w| wm::op("close", Some(w))))
            .action(picker_wheel("dynamic-send-to-back-wheel", "Dynamic Send to Back", "send-to-back", vec![], |_, w| wm::op("lower", Some(w))))
            .action(picker_wheel("dynamic-always-on-top-wheel", "Dynamic Always on Top", "pin", vec![], |_, w| wm::toggle("toggle-above", Some(w)).map(|_| ())))
            .action(picker_wheel("dynamic-fullscreen-wheel", "Dynamic Full Screen", "fullscreen", vec![], |_, w| wm::toggle("toggle-fullscreen", Some(w)).map(|_| ())))
            .action(picker_wheel("dynamic-max-restore-wheel", "Dynamic Maximize / Restore", "maximize-2", vec![], |_, w| wm::toggle("toggle-maximize", Some(w)).map(|_| ())))
            .action(picker_wheel("dynamic-transparency-wheel", "Dynamic Transparency", "blend", vec![transparency_opacity_input()], |ctx, w| toggle_transparency(Some(w), &ctx.inputs).map(|_| ())))
            .action(picker_wheel("dynamic-move-next-monitor-wheel", "Dynamic Move to Next Monitor", "monitor-up", vec![], |_, w| move_to_next_output(Some(w))))
            .action(picker_wheel("dynamic-center-window-wheel", "Dynamic Center Window", "focus", vec![monitor_input()], |ctx, w| center(Some(w), &ctx.inputs)))
            .action(snap_picker("dynamic-move-left-half-wheel", "Dynamic Move Left Half", "panel-left", reg(0.0, 0.0, 0.5, 1.0)))
            .action(snap_picker("dynamic-move-right-half-wheel", "Dynamic Move Right Half", "panel-right", reg(0.5, 0.0, 0.5, 1.0)))
            .action(snap_picker("dynamic-move-top-half-wheel", "Dynamic Move Top Half", "panel-top", reg(0.0, 0.0, 1.0, 0.5)))
            .action(snap_picker("dynamic-move-bottom-half-wheel", "Dynamic Move Bottom Half", "panel-bottom", reg(0.0, 0.5, 1.0, 0.5)))
            .action(snap_picker("dynamic-move-top-left-wheel", "Dynamic Move Top Left", "move-up-left", reg(0.0, 0.0, 0.5, 0.5)))
            .action(snap_picker("dynamic-move-top-right-wheel", "Dynamic Move Top Right", "move-up-right", reg(0.5, 0.0, 0.5, 0.5)))
            .action(snap_picker("dynamic-move-bottom-left-wheel", "Dynamic Move Bottom Left", "move-down-left", reg(0.0, 0.5, 0.5, 0.5)))
            .action(snap_picker("dynamic-move-bottom-right-wheel", "Dynamic Move Bottom Right", "move-down-right", reg(0.5, 0.5, 0.5, 0.5)))
            .action(snap_picker("dynamic-fill-display-wheel", "Dynamic Fill Display", "fullscreen", reg(0.0, 0.0, 1.0, 1.0)))
            .action(picker_wheel("dynamic-custom-position-wheel", "Dynamic Custom Position", "move", custom_position_inputs(), |ctx, w| apply_custom_position(Some(w), &ctx.inputs)));

        // ── Workspaces & desktop ──
        p = p
            .action(button("desktop-previous", "Previous Desktop", "chevrons-left", || system::desktop_switch("prev")))
            .action(button("desktop-next", "Next Desktop", "chevrons-right", || system::desktop_switch("next")))
            .action(button("desktop-new", "New Desktop", "copy-plus", || system::desktop_switch("new")))
            .action(button("desktop-close", "Close Desktop", "copy-x", || system::desktop_switch("close")))
            .action(button("overview", "Overview", "layout-grid", system::overview))
            .action(button("present-windows", "Present Windows", "app-window", system::present_windows))
            .action(button("app-launcher", "Applications", "layout-dashboard", system::app_launcher))
            .action(button("search", "Search", "search", system::search))
            .action(button("show-desktop", "Show Desktop", "monitor", system::show_desktop))
            .action(button("screen-snip", "Screenshot", "scissors", system::screenshot))
            .action(button("emoji-panel", "Emoji Picker", "smile", system::emoji_picker))
            .action(button("file-manager", "Files", "folder-open", system::file_manager))
            .action(button("open-settings", "System Settings", "settings", system::settings))
            .action(button("system-monitor", "System Monitor", "activity", system::system_monitor))
            // A button (opens the system monitor) whose tile shows the chosen stat live.
            .action(
                button("system-stats", "System Stats", "gauge", system::system_monitor)
                    .inputs(vec![input::select("stat", "Show", &[("cpu", "CPU"), ("gpu", "GPU"), ("memory", "Memory")]).with("default", "cpu")])
                    .live(Duration::from_secs(1), |ic| {
                        let (name, pct) = stats::read(&ic.input_str("stat"))?;
                        Ok(match pct {
                            Some(p) => Visual::new().label(format!("{name} {p:.0}%")).icon(stats::gauge(p)),
                            None if name == "CPU" => Visual::new().label(format!("{name} …")),
                            None => Visual::new().label(format!("{name} n/a")),
                        })
                    }),
            )
            .action(button("lock-screen", "Lock Screen", "lock", system::lock_screen))
            .action(button("sleep-display", "Turn Off Display", "moon", system::sleep_display))
            .action(
                Action::toggle("radio-toggle", "Wi-Fi / Bluetooth")
                    .icon("wifi")
                    .volatile()
                    .inputs(vec![input::select("radio", "Radio", &[("wifi", "Wi-Fi"), ("bluetooth", "Bluetooth")]).with("default", "wifi")])
                    .state("off", "Off", "toggle-left")
                    .state("on", "On", "toggle-right")
                    .initial_with(|ic| Some((if system::radio_on(&radio_of(&ic.inputs)).unwrap_or(false) { "on" } else { "off" }).into()))
                    .on_press(|ctx| {
                        let kind = radio_of(&ctx.inputs);
                        let on = system::set_radio(&kind, !system::radio_on(&kind)?)?;
                        ctx.set_state(if on { "on" } else { "off" });
                        Ok(())
                    }),
            )
            .action(
                Action::toggle("power-profile", "Power Profile")
                    .icon("battery-charging")
                    .volatile()
                    .inputs(vec![input::query_select("profile", "Power profile", "get-power-profiles", &[])])
                    .state("inactive", "Not Active", "battery")
                    .state("active", "Active", "battery-charging")
                    .initial_with(|ic| Some((if system::active_profile().is_ok_and(|a| a == ic.input_str("profile")) { "active" } else { "inactive" }).into()))
                    .on_press(|ctx| {
                        system::set_profile(&ctx.input_str("profile"))?;
                        ctx.set_state("active");
                        Ok(())
                    }),
            )
            .action(
                Action::wheel("power-profile-wheel", "Power Profiles")
                    .icon("battery-charging")
                    .on_open(|ctx| {
                        let active = system::active_profile().ok();
                        let list = system::power_profiles().or_else(|e| ctx.fail(e.message().to_string()))?;
                        // The slice id is the profile id itself.
                        Ok(list.into_iter().map(|id| json!({"id": id, "label": system::profile_label(&id), "icon": if active.as_deref() == Some(id.as_str()) { "circle-check" } else { "battery" }})).collect())
                    })
                    .on_child_press(|ctx| system::set_profile(ctx.child_id.as_deref().unwrap_or(""))),
            )
            .action(
                Action::button("ping", "Ping")
                    .icon("activity")
                    .inputs(vec![input::text("host", "Host").placeholder("1.1.1.1").default_value("1.1.1.1")])
                    .live(Duration::from_secs(5), |ic| Ok(Visual::new().label(ping_label(&ic.input_str("host")))))
                    .on_press(|ctx| {
                        ctx.set_label(ping_label(&ctx.input_str("host")));
                        Ok(())
                    }),
            )
            .action(Action::button("text-snippet", "Text Snippet").icon("text-cursor-input").inputs(vec![input::textarea("text", "Text")]).on_press(|ctx| {
                let text = ctx.input_str("text");
                if text.is_empty() {
                    ctx.set_error("Add the snippet's text first");
                    return Ok(());
                }
                // Typing through the virtual keyboard depends on the keyboard layout and can't
                // produce most non-ASCII text, so the snippet always goes through the clipboard.
                clipboard::set_text(&text)?;
                clipboard::paste()
            }));

        // ── Audio ──
        p = p
            .action(
                Action::button("set-app-volume", "Set App Volume")
                    .icon("volume-2")
                    .inputs(vec![
                        app_select_input(),
                        input::select("preset", "Volume", &[("100", "100%"), ("90", "90%"), ("80", "80%"), ("75", "75%"), ("60", "60%"), ("50", "50%"), ("40", "40%"), ("30", "30%"), ("20", "20%"), ("10", "10%"), ("0", "0% (silent)"), ("__custom__", "Custom…")]).with("default", "75"),
                        input::text("custom", "Custom volume (0–100)").placeholder("75").with("showIf", json!({"key": "preset", "equals": "__custom__"})),
                    ])
                    .on_press(|ctx| {
                        let Some(pct) = preset_percent(&ctx.inputs) else {
                            ctx.set_error("Volume must be a number between 0 and 100");
                            return Ok(());
                        };
                        if audio::set_app_volume(&resolve_app(&ctx.inputs)?, pct as f64)? == 0 {
                            ctx.set_error(no_stream(&ctx.inputs));
                        }
                        Ok(())
                    }),
            )
            .action(app_volume_step("app-volume-up", "App Volume Up", 1.0))
            .action(app_volume_step("app-volume-down", "App Volume Down", -1.0))
            .action(
                Action::toggle("toggle-app-mute", "Mute App")
                    .icon("volume-x")
                    .volatile()
                    .inputs(vec![app_select_input()])
                    .state("unmuted", "Unmuted", "volume-2")
                    .state("muted", "Muted", "volume-x")
                    .initial_with(|ic| Some(mute_state(resolve_app(&ic.inputs).ok().and_then(|t| audio::app_mute(&t).ok().flatten()).unwrap_or(false)).into()))
                    .on_press(|ctx| {
                        match audio::toggle_app_mute(&resolve_app(&ctx.inputs)?)? {
                            None => ctx.set_error(no_stream(&ctx.inputs)),
                            Some(muted) => ctx.fan_out(mute_state(muted)),
                        }
                        Ok(())
                    }),
            )
            .action(
                Action::slider("app-volume", "App Volume", 0.0, 100.0)
                    .icon("volume-2")
                    .unit("%")
                    .inputs(vec![app_select_input(), input::text("step", "Step (%)").placeholder("5").default_value("5")])
                    .initial_with(|ic| Some(resolve_app(&ic.inputs).ok().and_then(|t| audio::app_volume(&t).ok().flatten()).unwrap_or(0).to_string()))
                    .on_adjust(|ctx| {
                        let target = resolve_app(&ctx.inputs)?;
                        let result = match ctx.value {
                            Some(v) => {
                                let v = v.round().clamp(0.0, 100.0);
                                if audio::set_app_volume(&target, v)? == 0 {
                                    ctx.set_error(no_stream(&ctx.inputs));
                                    return Ok(());
                                }
                                v as i64
                            }
                            None => match audio::adjust_app_volume(&target, input_int(&ctx.inputs, "step").unwrap_or(5) as f64 * ctx.delta.unwrap_or(0.0))? {
                                Some(r) => r,
                                None => {
                                    ctx.set_error(no_stream(&ctx.inputs));
                                    return Ok(());
                                }
                            },
                        };
                        ctx.set_state(result);
                        Ok(())
                    }),
            )
            .action(
                Action::toggle("toggle-mic-mute", "Mute Microphone")
                    .icon("mic")
                    .volatile()
                    .state("unmuted", "Mic On", "mic")
                    .state("muted", "Mic Muted", "mic-off")
                    .initial_with(|_| Some(mute_state(audio::device_muted(true, audio::DEFAULT_DEVICE).unwrap_or(false)).into()))
                    .on_press(|ctx| {
                        ctx.fan_out(mute_state(audio::toggle_device_mute(true, audio::DEFAULT_DEVICE)?));
                        Ok(())
                    }),
            )
            .action(
                Action::hold("push-to-talk", "Push to Talk")
                    .icon("mic-off")
                    .released("Mic Muted", "mic-off")
                    .held("Talking", "mic")
                    .on_hold_start(|_| audio::set_device_mute(true, audio::DEFAULT_DEVICE, false))
                    .on_hold_end(|_| audio::set_device_mute(true, audio::DEFAULT_DEVICE, true)),
            )
            .action(device_wheel("output-device-wheel", "Output Device", "speaker", false))
            .action(device_wheel("input-device-wheel", "Input Device", "mic", true))
            .action(
                Action::toggle("set-audio-device", "Set Audio Device")
                    .icon("speaker")
                    .volatile()
                    .inputs(vec![flow_input(), input::query_select("device", "Device", "get-audio-devices-fixed", &["flow"])])
                    .state("inactive", "Not Selected", "speaker")
                    .state("active", "Selected", "circle-check")
                    .initial_with(|ic| {
                        let id = ic.input_str("device");
                        Some((if !id.is_empty() && audio::default_device(is_input(&ic.inputs)).as_deref() == Some(id.as_str()) { "active" } else { "inactive" }).into())
                    })
                    .on_press(|ctx| {
                        let id = ctx.input_str("device");
                        if id.is_empty() {
                            return ctx.fail("Choose a device first");
                        }
                        audio::set_default(is_input(&ctx.inputs), &id)?;
                        ctx.set_state("active");
                        Ok(())
                    }),
            )
            .action(
                Action::toggle("toggle-audio-device", "Toggle Audio Device")
                    .icon("arrow-left-right")
                    .volatile()
                    .inputs(vec![flow_input(), input::query_select("device", "Device A", "get-audio-devices-fixed", &["flow"]), input::query_select("device_b", "Device B", "get-audio-devices-fixed", &["flow"])])
                    .state("a", "Device A", "headphones")
                    .state("b", "Device B", "speaker")
                    .initial_with(|ic| Some((if audio::default_device(is_input(&ic.inputs)).as_deref() == Some(ic.input_str("device_b").as_str()) { "b" } else { "a" }).into()))
                    .on_press(|ctx| {
                        let (a, b) = (ctx.input_str("device"), ctx.input_str("device_b"));
                        if a.is_empty() || b.is_empty() {
                            return ctx.fail("Choose both devices first");
                        }
                        let input = is_input(&ctx.inputs);
                        // From A (or anything else) go to B; from B go back to A.
                        let to_b = audio::default_device(input).as_deref() != Some(b.as_str());
                        audio::set_default(input, if to_b { &b } else { &a })?;
                        ctx.set_state(if to_b { "b" } else { "a" });
                        Ok(())
                    }),
            )
            .action(
                Action::toggle("device-mute", "Mute Device")
                    .icon("volume-x")
                    .volatile()
                    .inputs(vec![flow_input(), device_input()])
                    .state("unmuted", "On", "volume-2")
                    .state("muted", "Muted", "volume-x")
                    .initial_with(|ic| Some(mute_state(audio::device_muted(is_input(&ic.inputs), &ic.input_str("device")).unwrap_or(false)).into()))
                    .on_press(|ctx| {
                        ctx.set_state(mute_state(audio::toggle_device_mute(is_input(&ctx.inputs), &ctx.input_str("device"))?));
                        Ok(())
                    }),
            )
            .action(
                Action::slider("device-volume", "Device Volume", 0.0, 100.0)
                    .icon("volume-2")
                    .unit("%")
                    .inputs(vec![flow_input(), device_input(), input::text("step", "Step (%)").placeholder("5").default_value("5")])
                    .initial_with(|ic| Some(audio::device_volume(is_input(&ic.inputs), &ic.input_str("device")).unwrap_or(0).to_string()))
                    .on_adjust(|ctx| {
                        let (input, id) = (is_input(&ctx.inputs), ctx.input_str("device"));
                        let result = match ctx.value {
                            Some(v) => {
                                let v = v.round().clamp(0.0, 100.0);
                                audio::set_device_volume(input, &id, v)?;
                                v as i64
                            }
                            None => audio::adjust_device_volume(input, &id, input_int(&ctx.inputs, "step").unwrap_or(5) as f64 * ctx.delta.unwrap_or(0.0))?,
                        };
                        ctx.set_state(result);
                        Ok(())
                    }),
            );

        // ── Clipboard History ──
        p.action(
            Action::wheel("clipboard-history-wheel", "Clipboard History")
                .icon("clipboard-list")
                .on_open(|ctx| {
                    let items = clipboard::history().or_else(|e| ctx.fail(e.message().to_string()))?;
                    if items.is_empty() {
                        return ctx.fail("Clipboard history is empty — copy something first");
                    }
                    let mut map = HashMap::new();
                    let slices = items
                        .into_iter()
                        .filter(|t| !t.trim().is_empty())
                        .take(16)
                        .enumerate()
                        .map(|(i, text)| {
                            let child = format!("c{i}");
                            let slice = json!({"id": child, "label": clipboard::label(&text), "icon": "clipboard"});
                            map.insert(child, text);
                            slice
                        })
                        .collect();
                    *CLIPBOARD_ITEMS.lock().unwrap() = Some(map);
                    Ok(slices)
                })
                .on_child_press(|ctx| {
                    let text = CLIPBOARD_ITEMS.lock().unwrap().as_ref().and_then(|m| m.get(ctx.child_id.as_deref().unwrap_or("")).cloned());
                    let Some(text) = text else {
                        ctx.set_error("That clipboard item is no longer available");
                        return Ok(());
                    };
                    clipboard::set_text(&text)?;
                    // Paste into whatever has focus (the dial never takes it).
                    clipboard::paste()
                }),
        )
        .run()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn regions_snap_to_whole_pixels_without_gaps() {
            let work = Rect { x: 0.0, y: 32.0, w: 2560.0, h: 1354.0 };
            let left = region_rect(work, reg(0.0, 0.0, 1.0 / 3.0, 1.0));
            let mid = region_rect(work, reg(1.0 / 3.0, 0.0, 1.0 / 3.0, 1.0));
            let right = region_rect(work, reg(2.0 / 3.0, 0.0, 1.0 / 3.0, 1.0));
            assert_eq!(left.x + left.w, mid.x);
            assert_eq!(mid.x + mid.w, right.x);
            assert_eq!(right.x + right.w, 2560.0);
            assert_eq!(region_rect(work, reg(0.0, 0.5, 0.5, 0.5)), Rect { x: 0.0, y: 709.0, w: 1280.0, h: 677.0 });
        }

        #[test]
        fn parse_int_takes_leading_number() {
            assert_eq!(parse_int(" 120px"), Some(120));
            assert_eq!(parse_int("-5"), Some(-5));
            assert_eq!(parse_int("x"), None);
        }

        #[test]
        fn every_named_region_resolves() {
            for r in ["left-half", "right-half", "top-half", "bottom-half", "top-left", "top-right", "bottom-left", "bottom-right", "left-third", "center-third", "right-third", "fill"] {
                assert!(region_of(r).is_some(), "{r}");
            }
            assert!(region_of("custom").is_none());
        }
    }
}
