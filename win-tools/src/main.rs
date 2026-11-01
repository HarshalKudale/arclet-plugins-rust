//! win-tools — advanced window management for Arclet.
//!
//! Current Window (minimize/maximize/z-order/toggles), Audio (per-app volume/mute matched by
//! exe name), Position (snap to halves/quarters/fullscreen/custom on any monitor), window-picker
//! wheels ("Dynamic …"), a taskbar-style switcher, virtual-desktop/system shortcuts, live system stats, and Windows clipboard history.

#[cfg(not(windows))]
fn main() {
    eprintln!("win-tools only runs on Windows");
    std::process::exit(1);
}

#[cfg(windows)]
mod audio;
#[cfg(windows)]
mod clipboard;
#[cfg(windows)]
mod stats;
#[cfg(windows)]
mod win32;
#[cfg(windows)]
mod winicon;

#[cfg(windows)]
fn main() {
    app::run()
}

#[cfg(windows)]
mod app {
    use crate::{audio, clipboard, stats, win32, win32::Region, winicon};
    use arclet_sdk::input::{self, InputExt};
    use arclet_sdk::{json, log, Action, Ctx, Error, Plugin, Result, Value, Visual};
    use std::collections::HashMap;
    use std::sync::Mutex;

    fn parse_int(text: &str) -> Option<i64> {
        let t = text.trim();
        let end = t.char_indices().take_while(|&(i, c)| c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))).last().map(|(i, c)| i + c.len_utf8())?;
        t[..end].parse().ok()
    }

    fn input_int(ctx_inputs: &Value, key: &str) -> Option<i64> {
        match &ctx_inputs[key] {
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

    fn work_of(hwnd: win32::Hwnd, inputs: &Value) -> Result<win32::Rect> {
        Ok(win32::resolve_monitor(hwnd, &str_input(inputs, "monitor"))?.work)
    }

    fn alpha_from_opacity(inputs: &Value) -> u8 {
        match input_int(inputs, "opacity") {
            Some(pct) => ((pct as f64 / 100.0) * 255.0).round().clamp(0.0, 255.0) as u8,
            None => 191,
        }
    }

    /// Raw percent for the "preset or custom" opacity/volume inputs; `None` if invalid.
    fn preset_percent(inputs: &Value) -> Option<i64> {
        let raw = if inputs["preset"] == "__custom__" { str_input(inputs, "custom") } else { str_input(inputs, "preset") };
        parse_int(&raw).filter(|p| (0..=100).contains(p))
    }

    fn opacity_options(with_custom_label: bool) -> Vec<(&'static str, &'static str)> {
        let mut v = vec![
            ("100", if with_custom_label { "100% — Fully Opaque" } else { "100%" }),
            ("90", "90%"),
            ("80", "80%"),
            ("75", "75%"),
            ("60", "60%"),
            ("50", "50%"),
            ("40", "40%"),
            ("30", "30%"),
            ("20", "20%"),
            ("10", "10%"),
        ];
        v.push(("__custom__", "Custom…"));
        v
    }

    fn opacity_preset_inputs(select_label: &str, custom_label: &str) -> Vec<Value> {
        vec![
            input::select("preset", select_label, &opacity_options(true)).with("default", "75"),
            input::text("custom", custom_label).placeholder("75").with("showIf", json!({"key": "preset", "equals": "__custom__"})),
        ]
    }

    fn transparency_opacity_input() -> Value {
        input::select("opacity", "Opacity when enabled", &[("90", "90%"), ("80", "80%"), ("75", "75% (default)"), ("60", "60%"), ("50", "50%"), ("30", "30%")]).with("default", "75")
    }

    fn app_select_input() -> Value {
        input::query_select("app", "App", "get-audio-apps", &[]).with("default", "__foreground__")
    }

    /// Resolve the `app` input to a concrete exe name (default: the foreground window's app).
    fn resolve_app_exe(inputs: &Value) -> Result<String> {
        match inputs["app"].as_str() {
            Some(a) if !a.is_empty() && a != "__foreground__" => Ok(a.to_string()),
            _ => audio::foreground_exe_name()?.ok_or_else(|| Error::msg("Could not determine the foreground window's process")),
        }
    }

    fn no_session(exe: &str) -> String {
        format!("No active audio session for \"{exe}\"")
    }

    // ── Action builders ──────────────────────────────────────────────────────

    /// A button that runs `op` against the foreground window.
    fn window_button(id: &str, label: &str, icon: &str, op: fn(Option<win32::Hwnd>) -> Result) -> Action {
        Action::button(id, label).icon(icon).on_press(move |_| op(None))
    }

    /// A volatile toggle over a window-state getter/flipper pair.
    fn window_toggle(id: &str, label: &str, icon: &str, states: [(&'static str, &'static str, &'static str); 2], get: fn(Option<win32::Hwnd>) -> Result<&'static str>, flip: fn(&Ctx) -> Result<&'static str>) -> Action {
        let mut a = Action::toggle(id, label).icon(icon).volatile();
        for (v, l, i) in states {
            a = a.state(v, l, i);
        }
        let default = states[0].0;
        a.initial_with(move |_| Some(get(None).unwrap_or(default).to_string())).on_press(move |ctx| {
            ctx.fan_out(flip(ctx)?);
            Ok(())
        })
    }

    fn snap_button(id: &str, label: &str, icon: &str, region: Region) -> Action {
        Action::button(id, label).icon(icon).inputs(vec![monitor_input()]).on_press(move |ctx| {
            let hwnd = win32::foreground_window()?;
            win32::move_to_region(hwnd, work_of(hwnd, &ctx.inputs)?, region);
            Ok(())
        })
    }

    /// Resolve custom-position inputs against `hwnd` and apply them. Blank/invalid x/y/w/h keep the current value.
    fn apply_custom_position(hwnd: win32::Hwnd, inputs: &Value) -> Result {
        let cur = win32::window_rect(hwnd)?;
        let mon = win32::resolve_monitor(hwnd, &str_input(inputs, "monitor"))?;
        let rel_x = input_int(inputs, "x").unwrap_or((cur.x - mon.work.x) as i64) as i32;
        let rel_y = input_int(inputs, "y").unwrap_or((cur.y - mon.work.y) as i64) as i32;
        let w = input_int(inputs, "w").filter(|v| *v > 0).unwrap_or(cur.width as i64) as i32;
        let h = input_int(inputs, "h").filter(|v| *v > 0).unwrap_or(cur.height as i64) as i32;
        win32::set_window_bounds(hwnd, mon.work.x + rel_x, mon.work.y + rel_y, w, h);
        Ok(())
    }

    fn custom_position_inputs() -> Vec<Value> {
        // X/Y are relative to the selected monitor's work-area top-left. Blank keeps the current value.
        let keep = |key: &str, label: &str| input::text(key, label).placeholder("(keep current)").with("queryLabel", "Get current");
        vec![
            monitor_input(),
            keep("x", "X (from monitor left)").with("query", json!({"id": "get-window-x", "args": ["monitor"]})),
            keep("y", "Y (from monitor top)").with("query", json!({"id": "get-window-y", "args": ["monitor"]})),
            keep("w", "Width").with("query", "get-window-w"),
            keep("h", "Height").with("query", "get-window-h"),
        ]
    }

    static PICKERS: Mutex<Option<HashMap<String, HashMap<String, win32::Hwnd>>>> = Mutex::new(None);

    /// Slices for every open window (max 16) with the owning exe's real icon; records child id → hwnd under `wheel_id`.
    fn window_slices(wheel_id: &str) -> Result<Vec<Value>> {
        let windows = win32::list_open_windows();
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
                map.insert(child.clone(), w.hwnd);
                let icon = win32::process_exe_path(w.pid).and_then(|p| winicon::extract_exe_icon_data_uri(&p)).unwrap_or_else(|| "columns-2".into());
                let label = if !w.title.is_empty() { w.title.clone() } else { w.exe_name.clone().unwrap_or_else(|| "Untitled".into()) };
                json!({"id": child, "label": label, "icon": icon})
            })
            .collect();
        PICKERS.lock().unwrap().get_or_insert_with(HashMap::new).insert(wheel_id.to_string(), map);
        Ok(slices)
    }

    fn picked_window(wheel_id: &str, ctx: &Ctx) -> Option<win32::Hwnd> {
        PICKERS.lock().unwrap().as_ref()?.get(wheel_id)?.get(ctx.child_id.as_deref()?).copied()
    }

    /// A wheel listing every open window; the chosen one gets `on_select` applied.
    fn picker_wheel(id: &'static str, label: &str, icon: &str, inputs: Vec<Value>, on_select: fn(&Ctx, win32::Hwnd) -> Result) -> Action {
        Action::wheel(id, label)
            .icon(icon)
            .inputs(inputs)
            .on_open(move |ctx| match window_slices(id) {
                Ok(s) => Ok(s),
                Err(e) => ctx.fail(e.message().to_string()),
            })
            .on_child_press(move |ctx| {
                let Some(hwnd) = picked_window(id, ctx) else {
                    ctx.set_error("That window is no longer available");
                    return Ok(());
                };
                on_select(ctx, hwnd)
            })
    }

    fn snap_picker(id: &'static str, label: &str, icon: &str, region: Region) -> Action {
        Action::wheel(id, label)
            .icon(icon)
            .inputs(vec![monitor_input()])
            .on_open(move |ctx| window_slices(id).or_else(|e| ctx.fail(e.message().to_string())))
            .on_child_press(move |ctx| {
                let Some(hwnd) = picked_window(id, ctx) else {
                    ctx.set_error("That window is no longer available");
                    return Ok(());
                };
                win32::move_to_region(hwnd, work_of(hwnd, &ctx.inputs)?, region);
                Ok(())
            })
    }

    fn reg(x: f64, y: f64, w: f64, h: f64) -> Region {
        Region { x, y, w, h }
    }

    fn display(m: &win32::MonitorInfoEx, i: usize) -> String {
        format!("Monitor {}{} — {}×{} @ ({}, {})", i + 1, if m.primary { " (Primary)" } else { "" }, m.monitor.width, m.monitor.height, m.monitor.x, m.monitor.y)
    }

    pub fn run() -> ! {
        let mut p = Plugin::new("win-tools", "Win Tools")
            .version("1.1.0")
            .author("Arclet")
            .description("Advanced window management: snap, resize, transparency, always-on-top, per-app volume, and more.")
            .icon("app-window")
            .platforms(&["windows"])
            // Several actions derive their state from the currently focused window.
            .subscribe(&["focus_changed"])
            // ── Queries ──
            .query("get-monitors", |ctx| {
                ctx.log("query get-monitors");
                let mut opts = vec![json!({"value": "current", "label": "Current Monitor (where window is)"})];
                for (i, m) in win32::list_monitors().iter().enumerate() {
                    opts.push(json!({"value": i.to_string(), "label": display(m, i)}));
                }
                Ok(Value::Array(opts))
            })
            .query("get-audio-apps", |ctx| {
                ctx.log("query get-audio-apps");
                let apps = audio::list_session_apps().or_else(|e| ctx.fail(format!("Failed to list audio sessions: {e}")))?;
                let mut opts = vec![json!({"value": "__foreground__", "label": "Current Window (foreground app)"})];
                opts.extend(apps.into_iter().map(|a| json!({"value": a, "label": a})));
                Ok(Value::Array(opts))
            })
            // Current window position/size, to auto-fill the custom-position fields. X/Y are relative to
            // the selected monitor's work-area top-left so the value can be pasted straight back in.
            .query("get-window-x", |ctx| {
                let hwnd = win32::foreground_window()?;
                let mon = win32::resolve_monitor(hwnd, &str_input(&ctx.inputs, "monitor"))?;
                Ok(json!((win32::window_rect(hwnd)?.x - mon.work.x).to_string()))
            })
            .query("get-window-y", |ctx| {
                let hwnd = win32::foreground_window()?;
                let mon = win32::resolve_monitor(hwnd, &str_input(&ctx.inputs, "monitor"))?;
                Ok(json!((win32::window_rect(hwnd)?.y - mon.work.y).to_string()))
            })
            .query("get-window-w", |_| Ok(json!(win32::window_rect(win32::foreground_window()?)?.width.to_string())))
            .query("get-window-h", |_| Ok(json!(win32::window_rect(win32::foreground_window()?)?.height.to_string())))
            // ── Current Window ──
            .action(window_button("minimize", "Minimize", "minimize.svg", win32::minimize))
            .action(window_button("maximize", "Maximize", "maximize.svg", win32::maximize))
            .action(window_button("restore", "Restore", "restore.svg", win32::restore))
            .action(window_button("close-window", "Close Window", "close.svg", win32::close_win))
            .action(window_button("bring-to-front", "Bring to Front", "front.svg", win32::bring_to_front))
            .action(window_button("send-to-back", "Send to Back", "back.svg", win32::send_to_back))
            // ── Toggles ──
            .action(window_toggle("toggle-always-on-top", "Always on Top", "topmost.svg", [("off", "Normal", "circle"), ("on", "Always on Top", "pin")], win32::always_on_top_state, |_| win32::toggle_always_on_top(None)))
            .action(window_toggle("toggle-borderless", "Toggle Borderless", "borderless.svg", [("bordered", "Bordered", "square"), ("borderless", "Borderless", "square-dashed")], win32::borderless_state, |_| win32::toggle_borderless(None)))
            .action(
                window_toggle("toggle-transparency", "Toggle Transparency", "transparent.svg", [("opaque", "Opaque", "square"), ("transparent", "Transparent", "square-dashed")], win32::transparency_state, |ctx| {
                    win32::toggle_transparency(alpha_from_opacity(&ctx.inputs), None)
                })
                .inputs(vec![transparency_opacity_input()]),
            )
            .action(Action::button("set-opacity", "Set Opacity").icon("opacity.svg").inputs(opacity_preset_inputs("Opacity", "Custom opacity (0–100)")).on_press(|ctx| {
                match preset_percent(&ctx.inputs) {
                    Some(pct) => win32::set_opacity(pct, None),
                    None => {
                        ctx.set_error("Opacity must be a number between 0 and 100");
                        Ok(())
                    }
                }
            }))
            .action(window_toggle("toggle-click-through", "Toggle Click-Through", "clickthrough.svg", [("normal", "Normal", "mouse-pointer-2"), ("clickthrough", "Click-Through", "mouse-pointer-ban")], win32::click_through_state, |_| win32::toggle_click_through(None)))
            .action(window_toggle("toggle-max-restore", "Maximize / Restore", "maxrestore.svg", [("normal", "Windowed", "minimize-2"), ("maximized", "Maximized", "maximize-2")], win32::maximize_restore_state, |_| win32::toggle_maximize_restore(None)))
            // ── Audio (per-app volume) ──
            .action(
                Action::button("set-app-volume", "Set App Volume")
                    .icon("volume.svg")
                    .inputs({
                        let mut v = vec![app_select_input()];
                        v.push(
                            input::select(
                                "preset",
                                "Volume",
                                &[("100", "100%"), ("90", "90%"), ("80", "80%"), ("75", "75%"), ("60", "60%"), ("50", "50%"), ("40", "40%"), ("30", "30%"), ("20", "20%"), ("10", "10%"), ("0", "0% (silent)"), ("__custom__", "Custom…")],
                            )
                            .with("default", "75"),
                        );
                        v.push(input::text("custom", "Custom volume (0–100)").placeholder("75").with("showIf", json!({"key": "preset", "equals": "__custom__"})));
                        v
                    })
                    .on_press(|ctx| {
                        let Some(pct) = preset_percent(&ctx.inputs) else {
                            ctx.set_error("Volume must be a number between 0 and 100");
                            return Ok(());
                        };
                        let exe = resolve_app_exe(&ctx.inputs)?;
                        if audio::set_app_volume(&exe, pct as f64)? == 0 {
                            ctx.set_error(no_session(&exe));
                        }
                        Ok(())
                    }),
            )
            .action(app_volume_step("app-volume-up", "App Volume Up", 1.0))
            .action(app_volume_step("app-volume-down", "App Volume Down", -1.0))
            .action(
                Action::toggle("toggle-app-mute", "Mute App")
                    .icon("mute.svg")
                    .volatile()
                    .inputs(vec![app_select_input()])
                    .state("unmuted", "Unmuted", "volume-2")
                    .state("muted", "Muted", "volume-x")
                    .initial_with(|ic| {
                        let muted = resolve_app_exe(&ic.inputs).ok().and_then(|exe| audio::app_mute(&exe).ok().flatten()).unwrap_or(false);
                        Some((if muted { "muted" } else { "unmuted" }).into())
                    })
                    .on_press(|ctx| {
                        let exe = resolve_app_exe(&ctx.inputs)?;
                        match audio::toggle_app_mute(&exe)? {
                            None => ctx.set_error(no_session(&exe)),
                            Some(muted) => ctx.fan_out(if muted { "muted" } else { "unmuted" }),
                        }
                        Ok(())
                    }),
            )
            // A persistent slider tracking the resolved app's volume.
            .action(
                Action::slider("app-volume", "App Volume", 0.0, 100.0)
                    .icon("volume.svg")
                    .unit("%")
                    .inputs(vec![app_select_input(), input::text("step", "Step (%)").placeholder("5").default_value("5")])
                    .initial_with(|ic| Some(resolve_app_exe(&ic.inputs).ok().and_then(|exe| audio::app_volume(&exe).ok().flatten()).unwrap_or(0).to_string()))
                    .on_adjust(|ctx| {
                        let exe = resolve_app_exe(&ctx.inputs)?;
                        let result = match ctx.value {
                            // Drag / dial commit: an absolute target.
                            Some(v) => {
                                let target = v.round().clamp(0.0, 100.0);
                                if audio::set_app_volume(&exe, target)? == 0 {
                                    ctx.set_error(no_session(&exe));
                                    return Ok(());
                                }
                                target as i64
                            }
                            // +/- stepper nudge.
                            None => {
                                let step = input_int(&ctx.inputs, "step").unwrap_or(5);
                                match audio::adjust_app_volume(&exe, step as f64 * ctx.delta.unwrap_or(0.0))? {
                                    Some(r) => r,
                                    None => {
                                        ctx.set_error(no_session(&exe));
                                        return Ok(());
                                    }
                                }
                            }
                        };
                        ctx.set_state(result);
                        Ok(())
                    }),
            )
            // ── Switch Window (taskbar-style wheel) ──
            .action(
                Action::wheel("switch-window-wheel", "Switch to Window")
                    .icon("switch-app.svg")
                    .on_open(|ctx| window_slices("switch-window-wheel").or_else(|e| ctx.fail(e.message().to_string())))
                    .on_child_press(|ctx| {
                        let Some(hwnd) = picked_window("switch-window-wheel", ctx) else {
                            ctx.set_error("That window is no longer available");
                            return Ok(());
                        };
                        win32::bring_window_to_front(hwnd);
                        Ok(())
                    }),
            )
            // ── Position ──
            .action(window_button("move-next-monitor", "Move to Next Monitor", "monitor.svg", win32::move_to_next_monitor))
            .action(center_button("center-window", "Center Window"))
            .action(snap_button("move-left-half", "Move Left Half", "snap-left.svg", reg(0.0, 0.0, 0.5, 1.0)))
            .action(snap_button("move-right-half", "Move Right Half", "snap-right.svg", reg(0.5, 0.0, 0.5, 1.0)))
            .action(snap_button("move-top-half", "Move Top Half", "snap-top.svg", reg(0.0, 0.0, 1.0, 0.5)))
            .action(snap_button("move-bottom-half", "Move Bottom Half", "snap-bottom.svg", reg(0.0, 0.5, 1.0, 0.5)))
            .action(snap_button("move-top-left", "Move Top Left", "snap-topleft.svg", reg(0.0, 0.0, 0.5, 0.5)))
            .action(snap_button("move-top-right", "Move Top Right", "snap-topright.svg", reg(0.5, 0.0, 0.5, 0.5)))
            .action(snap_button("move-bottom-left", "Move Bottom Left", "snap-bottomleft.svg", reg(0.0, 0.5, 0.5, 0.5)))
            .action(snap_button("move-bottom-right", "Move Bottom Right", "snap-bottomright.svg", reg(0.5, 0.5, 0.5, 0.5)))
            .action(center_button("move-center", "Move Center"))
            .action(Action::button("fullscreen", "Fullscreen").icon("fullscreen.svg").inputs(vec![monitor_input()]).on_press(|ctx| {
                let hwnd = win32::foreground_window()?;
                win32::make_fullscreen(hwnd, win32::resolve_monitor(hwnd, &str_input(&ctx.inputs, "monitor"))?.monitor);
                Ok(())
            }))
            .action(Action::button("custom-position", "Custom Position").icon("move.svg").inputs(custom_position_inputs()).on_press(|ctx| apply_custom_position(win32::foreground_window()?, &ctx.inputs)));

        // ── Dynamic Window Actions: window-picker wheels ──
        // "Bring to Front" is already covered by "Switch to Window", so it isn't duplicated here.
        p = p
            .action(picker_wheel("dynamic-minimize-wheel", "Dynamic Minimize", "minimize.svg", vec![], |_, w| win32::minimize(Some(w))))
            .action(picker_wheel("dynamic-maximize-wheel", "Dynamic Maximize", "maximize.svg", vec![], |_, w| win32::maximize(Some(w))))
            .action(picker_wheel("dynamic-restore-wheel", "Dynamic Restore", "restore.svg", vec![], |_, w| win32::restore(Some(w))))
            .action(picker_wheel("dynamic-close-wheel", "Dynamic Close", "close.svg", vec![], |_, w| win32::close_win(Some(w))))
            .action(picker_wheel("dynamic-send-to-back-wheel", "Dynamic Send to Back", "back.svg", vec![], |_, w| win32::send_to_back(Some(w))))
            .action(picker_wheel("dynamic-always-on-top-wheel", "Dynamic Always on Top", "topmost.svg", vec![], |_, w| win32::toggle_always_on_top(Some(w)).map(|_| ())))
            .action(picker_wheel("dynamic-borderless-wheel", "Dynamic Borderless", "borderless.svg", vec![], |_, w| win32::toggle_borderless(Some(w)).map(|_| ())))
            .action(picker_wheel("dynamic-transparency-wheel", "Dynamic Transparency", "transparent.svg", vec![transparency_opacity_input()], |ctx, w| {
                win32::toggle_transparency(alpha_from_opacity(&ctx.inputs), Some(w)).map(|_| ())
            }))
            .action(picker_wheel("dynamic-click-through-wheel", "Dynamic Click-Through", "clickthrough.svg", vec![], |_, w| win32::toggle_click_through(Some(w)).map(|_| ())))
            .action(picker_wheel("dynamic-max-restore-wheel", "Dynamic Maximize / Restore", "maxrestore.svg", vec![], |_, w| win32::toggle_maximize_restore(Some(w)).map(|_| ())))
            .action(picker_wheel("dynamic-set-opacity-wheel", "Dynamic Set Opacity", "opacity.svg", {
                let mut v = vec![input::select("preset", "Opacity", &opacity_options(true)).with("default", "75")];
                v.push(input::text("custom", "Custom opacity (0–100)").placeholder("75").with("showIf", json!({"key": "preset", "equals": "__custom__"})));
                v
            }, |ctx, w| match preset_percent(&ctx.inputs) {
                Some(pct) => win32::set_opacity(pct, Some(w)),
                None => {
                    ctx.set_error("Opacity must be a number between 0 and 100");
                    Ok(())
                }
            }))
            .action(picker_wheel("dynamic-move-next-monitor-wheel", "Dynamic Move to Next Monitor", "monitor.svg", vec![], |_, w| win32::move_to_next_monitor(Some(w))))
            .action(picker_wheel("dynamic-center-window-wheel", "Dynamic Center Window", "center.svg", vec![monitor_input()], |ctx, w| win32::center_window(w, work_of(w, &ctx.inputs)?)))
            .action(picker_wheel("dynamic-fullscreen-wheel", "Dynamic Fullscreen", "fullscreen.svg", vec![monitor_input()], |ctx, w| {
                win32::make_fullscreen(w, win32::resolve_monitor(w, &str_input(&ctx.inputs, "monitor"))?.monitor);
                Ok(())
            }))
            .action(snap_picker("dynamic-move-left-half-wheel", "Dynamic Move Left Half", "snap-left.svg", reg(0.0, 0.0, 0.5, 1.0)))
            .action(snap_picker("dynamic-move-right-half-wheel", "Dynamic Move Right Half", "snap-right.svg", reg(0.5, 0.0, 0.5, 1.0)))
            .action(snap_picker("dynamic-move-top-half-wheel", "Dynamic Move Top Half", "snap-top.svg", reg(0.0, 0.0, 1.0, 0.5)))
            .action(snap_picker("dynamic-move-bottom-half-wheel", "Dynamic Move Bottom Half", "snap-bottom.svg", reg(0.0, 0.5, 1.0, 0.5)))
            .action(snap_picker("dynamic-move-top-left-wheel", "Dynamic Move Top Left", "snap-topleft.svg", reg(0.0, 0.0, 0.5, 0.5)))
            .action(snap_picker("dynamic-move-top-right-wheel", "Dynamic Move Top Right", "snap-topright.svg", reg(0.5, 0.0, 0.5, 0.5)))
            .action(snap_picker("dynamic-move-bottom-left-wheel", "Dynamic Move Bottom Left", "snap-bottomleft.svg", reg(0.0, 0.5, 0.5, 0.5)))
            .action(snap_picker("dynamic-move-bottom-right-wheel", "Dynamic Move Bottom Right", "snap-bottomright.svg", reg(0.5, 0.5, 0.5, 0.5)))
            .action(picker_wheel("dynamic-custom-position-wheel", "Dynamic Custom Position", "move.svg", custom_position_inputs(), |ctx, w| apply_custom_position(w, &ctx.inputs)));

        // ── Virtual Desktops & System ──
        // Shell objects / URIs / programs through ShellExecute; keystrokes only where
        // Windows has no API (see CLAUDE.md).
        p = p
            // No public API switches, creates or closes virtual desktops (the internal COM
            // interfaces change between Windows builds); Win+Ctrl chords are reserved by the shell.
            .action(keys_button("desktop-previous", "Previous Desktop", "chevrons-left", "ctrl win left"))
            .action(keys_button("desktop-next", "Next Desktop", "chevrons-right", "ctrl win right"))
            .action(keys_button("desktop-new", "New Desktop", "copy-plus", "ctrl win d"))
            .action(keys_button("desktop-close", "Close Desktop", "copy-x", "ctrl win f4"))
            .action(shell_button("task-view", "Task View", "layout-grid", "shell:::{3080F90E-D7AD-11D9-BD98-0000947B0257}"))
            .action(shell_button("show-desktop", "Show Desktop", "monitor", "shell:::{3080F90D-D7AD-11D9-BD98-0000947B0257}"))
            .action(shell_button("screen-snip", "Screen Snip", "scissors", "ms-screenclip:"))
            .action(shell_button("file-explorer", "File Explorer", "folder-open", "explorer.exe"))
            .action(shell_button("open-settings", "Windows Settings", "settings", "ms-settings:"))
            .action(shell_button("run-dialog", "Run Dialog", "square-terminal", "shell:::{2559a1f3-21d7-11d4-bdaf-00c04f60b9f0}"))
            // No API opens the emoji panel; Win+. is reserved by the shell.
            .action(keys_button("emoji-panel", "Emoji Panel", "smile", "win period"))
            .action(shell_button("task-manager", "Task Manager", "activity", "taskmgr.exe"))
            // A plain button (opens Task Manager) whose tile shows the chosen stat live.
            .action(
                shell_button("system-stats", "System Stats", "gauge", "taskmgr.exe")
                    .inputs(vec![input::select("stat", "Show", &[("cpu", "CPU"), ("gpu", "GPU"), ("memory", "Memory")]).with("default", "cpu")])
                    .live(std::time::Duration::from_secs(1), |ic| {
                        let (name, pct) = stats::read(&ic.input_str("stat"))?;
                        Ok(match pct {
                            Some(p) => Visual::new().label(format!("{name} {p:.0}%")).icon(stats::gauge(p)),
                            None => Visual::new().label(format!("{name} …")),
                        })
                    }),
            )
            // Win+L can't be injected via SendInput, so lock through the API.
            .action(Action::button("lock-pc", "Lock PC").icon("lock").on_press(|_| win32::lock_workstation()));

        // ── Clipboard History ──
        p.action(
            Action::wheel("clipboard-history-wheel", "Clipboard History").icon("clipboard.svg").on_open(|ctx| {
                let history = clipboard::list_history().or_else(|e| ctx.fail(e.message().to_string()))?;
                log(format!("historyEnabled: {}, items: {}", history.enabled, history.items.len()));
                if !history.enabled {
                    return ctx.fail("Clipboard history is off — enable it in Settings > System > Clipboard");
                }
                if history.items.is_empty() {
                    return ctx.fail("Clipboard history is empty");
                }
                let mut map = HashMap::new();
                let slices = history
                    .items
                    .iter()
                    .enumerate()
                    .map(|(i, it)| {
                        let child = format!("c{i}");
                        map.insert(child.clone(), it.id.clone());
                        // Image entries carry their own thumbnail data URI; wheel slices can only use Lucide names or data: URIs.
                        let icon = match (it.kind, &it.icon) {
                            ("image", Some(uri)) => uri.clone(),
                            ("files", _) => "folder".into(),
                            _ => "clipboard".into(),
                        };
                        json!({"id": child, "label": it.label, "icon": icon})
                    })
                    .collect();
                *CLIPBOARD_IDS.lock().unwrap() = Some(map);
                Ok(slices)
            })
            .on_child_press(|ctx| {
                let id = CLIPBOARD_IDS.lock().unwrap().as_ref().and_then(|m| m.get(ctx.child_id.as_deref().unwrap_or("")).cloned());
                let Some(id) = id else {
                    ctx.set_error("That clipboard item is no longer available");
                    return Ok(());
                };
                clipboard::apply_history_item(&id)?;
                // Paste targets whatever window currently has focus — no window switch.
                clipboard::send_paste();
                Ok(())
            }),
        )
        .run()
    }

    static CLIPBOARD_IDS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

    fn app_volume_step(id: &str, label: &str, sign: f64) -> Action {
        Action::button(id, label)
            .icon("volume.svg")
            .inputs(vec![app_select_input(), input::text("step", "Step (%)").placeholder("5").default_value("5")])
            .on_press(move |ctx| {
                let exe = resolve_app_exe(&ctx.inputs)?;
                let step = input_int(&ctx.inputs, "step").unwrap_or(5);
                if audio::adjust_app_volume(&exe, step as f64 * sign)?.is_none() {
                    ctx.set_error(no_session(&exe));
                }
                Ok(())
            })
    }

    /// A button that opens a shell target (URI, `shell:::{CLSID}`, program).
    fn shell_button(id: &str, label: &str, icon: &str, target: &'static str) -> Action {
        Action::button(id, label).icon(icon).on_press(move |_| win32::shell_open(target))
    }

    /// A button that presses a Windows shell shortcut through the host. Last
    /// resort only: use it when no API or shell target does the job (see CLAUDE.md).
    fn keys_button(id: &str, label: &str, icon: &str, keys: &'static str) -> Action {
        Action::button(id, label).icon(icon).on_press(move |_| arclet_sdk::send_keys(keys))
    }

    fn center_button(id: &str, label: &str) -> Action {
        Action::button(id, label).icon("center.svg").inputs(vec![monitor_input()]).on_press(|ctx| {
            let hwnd = win32::foreground_window()?;
            win32::center_window(hwnd, work_of(hwnd, &ctx.inputs)?)
        })
    }
}
