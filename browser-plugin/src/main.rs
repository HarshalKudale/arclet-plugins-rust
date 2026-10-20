//! Shared Chrome / Edge / Brave / Firefox / Zen browser controls for Arclet:
//! shortcuts (sent to the focused browser via the host), URL opening, hold-to-repeat
//! tab cycling and zoom, a page-zoom slider, and bookmark / history wheels.

mod browsers;
mod data;

use arclet_sdk::input::{self, InputExt};
use arclet_sdk::{focus_app, json, send_keys, Action, Ctx, Error, Plugin, Result, Value};
use browsers::{detect_key, ensure_executable, meta, selection, Family, KEYS};
use std::collections::HashMap;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

static WHEEL_ITEMS: Mutex<Option<HashMap<String, (String, String)>>> = Mutex::new(None); // childId → (browser key, url)
static ZOOM_STATE: Mutex<Option<HashMap<String, i64>>> = Mutex::new(None);
static REPEATS: Mutex<Option<HashMap<String, Arc<AtomicBool>>>> = Mutex::new(None);

fn browser_input() -> Value {
    input::query_combobox("browser", "Browser", "installed_browser")
}

/// The selected browser value (exe path / app id), or "".
fn browser_value(inputs: &Value) -> String {
    selection(&inputs["browser"]).1
}

fn browser_key(inputs: &Value) -> Result<&'static str> {
    detect_key(&inputs["browser"]).ok_or_else(|| Error::msg("Select an installed browser first"))
}

fn label_for(browser: &str) -> &'static str {
    if KEYS.contains(&browser) {
        meta(browser).label
    } else {
        detect_key(&Value::String(browser.into())).map_or("Chrome", |k| meta(k).label)
    }
}

fn activate_browser(browser: &str) -> Result {
    let exe = ensure_executable(browser)?;
    if !focus_app(&exe)? {
        return Err(Error::msg(format!("{} window is not open", label_for(browser))));
    }
    Ok(())
}

fn send_browser_keys(browser: &str, keys: &str) -> Result {
    activate_browser(browser)?;
    send_keys(keys)
}

fn open_in_browser(browser: &str, url: &str) -> Result {
    let exe = ensure_executable(browser)?;
    let target = url.trim();
    if target.is_empty() {
        return Err(Error::msg("URL is required"));
    }
    let mut cmd = Command::new(exe);
    cmd.arg(target);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000 | 0x0000_0008); // CREATE_NO_WINDOW | DETACHED_PROCESS
    }
    cmd.spawn()?; // detached: the child is intentionally not waited on
    Ok(())
}

#[derive(Clone, Copy)]
enum Zoom {
    Reset,
    In,
    Out,
}

fn zoom_sequence(browser: &str, dir: Zoom, count: usize) -> Result {
    match dir {
        Zoom::Reset => send_browser_keys(browser, "ctrl 0"),
        Zoom::In | Zoom::Out => {
            let key = if matches!(dir, Zoom::In) { "ctrl equals" } else { "ctrl minus" };
            (0..count).try_for_each(|_| send_browser_keys(browser, key))
        }
    }
}

fn start_repeat(instance_id: &str, f: impl Fn() -> Result + Send + 'static) {
    stop_repeat(instance_id);
    let stop = Arc::new(AtomicBool::new(false));
    REPEATS.lock().unwrap().get_or_insert_with(HashMap::new).insert(instance_id.into(), stop.clone());
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(220));
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let _ = f();
    });
}

fn stop_repeat(instance_id: &str) {
    if let Some(stop) = REPEATS.lock().unwrap().as_mut().and_then(|m| m.remove(instance_id)) {
        stop.store(true, Ordering::SeqCst);
    }
}

fn zoom_key(ctx: &Ctx, browser: &str) -> String {
    format!("{}::{browser}", ctx.instance_id)
}

fn current_zoom(ctx: &Ctx, browser: &str) -> i64 {
    ZOOM_STATE.lock().unwrap().as_ref().and_then(|m| m.get(&zoom_key(ctx, browser)).copied()).or_else(|| ctx.state.as_deref().and_then(|s| s.trim().parse().ok())).unwrap_or(100)
}

fn set_zoom_state(ctx: &Ctx, browser: &str, value: i64) {
    let clamped = value.clamp(25, 300);
    ZOOM_STATE.lock().unwrap().get_or_insert_with(HashMap::new).insert(zoom_key(ctx, browser), clamped);
    ctx.set_state(clamped);
}

/// A button that focuses the chosen browser and sends one key chord.
fn basic_button(id: &str, label: &str, icon: &str, shortcut: fn(&Value) -> Result<String>) -> Action {
    Action::button(id, label).icon(icon).inputs(vec![browser_input()]).on_press(move |ctx| {
        send_browser_keys(&browser_value(&ctx.inputs), &shortcut(&ctx.inputs)?)
    })
}

/// Hold-to-repeat action: fires `step` immediately, then every 220 ms while held.
fn hold_repeat(id: &str, label: &str, icon: &str, released: (&str, &str), held: (&str, &str), step: fn(&Value) -> Result) -> Action {
    Action::hold(id, label)
        .icon(icon)
        .inputs(vec![browser_input()])
        .released(released.0, released.1)
        .held(held.0, held.1)
        .on_hold_start(move |ctx| {
            let result: Result = (|| {
                browser_key(&ctx.inputs)?;
                step(&ctx.inputs)?;
                let inputs = ctx.inputs.clone();
                start_repeat(&ctx.instance_id, move || step(&inputs));
                Ok(())
            })();
            if result.is_err() {
                stop_repeat(&ctx.instance_id);
            }
            result?;
            ctx.set_state("held");
            Ok(())
        })
        .on_hold_end(|ctx| {
            stop_repeat(&ctx.instance_id);
            ctx.set_state("released");
            Ok(())
        })
}

fn tab_step(keys: &'static str) -> impl Fn(&Value) -> Result {
    move |inputs| send_browser_keys(&browser_value(inputs), keys)
}

fn zoom_step(dir: Zoom) -> impl Fn(&Value) -> Result {
    move |inputs| zoom_sequence(browser_key(inputs)?, dir, 1)
}

/// A wheel listing recent bookmarks / history; pressing a child opens it.
fn link_wheel(id: &str, label: &str, icon: &str, child_prefix: &'static str, child_icon: &'static str, empty: &'static str, load: fn(&str) -> Result<Vec<data::Item>>) -> Action {
    Action::wheel(id, label)
        .icon(icon)
        .inputs(vec![browser_input()])
        .on_open(move |ctx| {
            let result: Result<Vec<Value>> = (|| {
                let browser = browser_key(&ctx.inputs)?;
                let items = load(browser)?;
                if items.is_empty() {
                    return Err(Error::msg(empty.replace("{}", meta(browser).label)));
                }
                let mut map = WHEEL_ITEMS.lock().unwrap();
                let map = map.get_or_insert_with(HashMap::new);
                map.retain(|k, _| !k.starts_with(child_prefix));
                Ok(items
                    .into_iter()
                    .enumerate()
                    .map(|(i, item)| {
                        let child = format!("{child_prefix}-{i}");
                        map.insert(child.clone(), (browser.to_string(), item.url.clone()));
                        json!({"id": child, "label": item.title.chars().take(80).collect::<String>(), "icon": child_icon})
                    })
                    .collect::<Vec<_>>())
            })();
            result.or_else(|e: Error| ctx.fail(e.message().to_string()))
        })
        .on_child_press(|ctx| {
            let child = ctx.child_id.clone().unwrap_or_default();
            let item = WHEEL_ITEMS.lock().unwrap().as_ref().and_then(|m| m.get(&child).cloned());
            let (browser, url) = item.ok_or_else(|| Error::msg("That item is no longer available"))?;
            open_in_browser(&browser, &url)
        })
}

fn main() {
    let mut plugin = Plugin::new("browser-plugin", "BrowserPlugin")
        .version("1.0.0")
        .author("Arclet")
        .description("Shared Chrome and Firefox browser controls, wheels, and zoom actions.")
        .icon("icon:lucide/globe")
        .ack_timeout_ms(5000);

    // (id, label, icon, shortcut)
    type Shortcut = fn(&Value) -> Result<String>;
    let basics: Vec<(&str, &str, &str, Shortcut)> = vec![
        ("new-tab", "New Tab", "icon:lucide/notebook-tabs", |_| Ok("ctrl t".into())),
        ("new-window", "New Window", "icon:lucide/app-window", |_| Ok("ctrl n".into())),
        ("new-private-window", "New Private / Incognito Window", "icon:lucide/app-window", |i| Ok(meta(browser_key(i)?).private_shortcut.into())),
        ("close-tab", "Close Tab", "icon:lucide/notebook-tabs", |_| Ok("ctrl w".into())),
        ("reopen-closed-tab", "Reopen Closed Tab", "icon:lucide/notebook-tabs", |_| Ok("ctrl shift t".into())),
        ("next-tab", "Next Tab", "icon:lucide/notebook-tabs", |_| Ok("ctrl pgdn".into())),
        ("previous-tab", "Previous Tab", "icon:lucide/notebook-tabs", |_| Ok("ctrl pgup".into())),
        ("reload-tab", "Reload Tab", "icon:lucide/refresh-cw", |_| Ok("ctrl r".into())),
        ("hard-reload-tab", "Hard Reload", "icon:lucide/refresh-cw", |i| Ok(meta(browser_key(i)?).hard_reload_shortcut.into())),
        ("go-back", "Back", "icon:lucide/arrow-left", |_| Ok("alt left".into())),
        ("go-forward", "Forward", "icon:lucide/arrow-right", |_| Ok("alt right".into())),
        ("focus-address-bar", "Focus Address Bar", "icon:lucide/search", |_| Ok("ctrl l".into())),
        ("open-downloads", "Open Downloads", "icon:lucide/download", |_| Ok("ctrl j".into())),
        ("open-history", "Open History", "icon:lucide/history", |_| Ok("ctrl h".into())),
        ("open-bookmarks-manager", "Open Bookmarks Manager", "icon:lucide/bookmark", |_| Ok("ctrl shift o".into())),
        ("bookmark-page", "Bookmark Page", "icon:lucide/bookmark", |_| Ok("ctrl d".into())),
        ("find-in-page", "Find in Page", "icon:lucide/search", |_| Ok("ctrl f".into())),
        ("print-page", "Print Page", "icon:lucide/printer", |_| Ok("ctrl p".into())),
        ("save-page", "Save Page", "icon:lucide/save", |_| Ok("ctrl s".into())),
        ("toggle-fullscreen", "Toggle Fullscreen", "icon:lucide/maximize", |_| Ok("f11".into())),
    ];
    for (id, label, icon, shortcut) in basics {
        plugin = plugin.action(basic_button(id, label, icon, shortcut));
    }

    plugin
        .action(
            Action::button("custom-keybind", "Custom Keybind")
                .icon("icon:lucide/keyboard")
                .inputs(vec![browser_input(), input::keybind("keys", "Keys").placeholder("k")])
                .on_press(|ctx| {
                    let keys = ctx.input_str("keys").trim().to_string();
                    if keys.is_empty() {
                        return Err(Error::msg("Keys are required"));
                    }
                    send_browser_keys(&browser_value(&ctx.inputs), &keys)
                }),
        )
        .action(Action::button("stop-loading", "Stop Loading").icon("stop.svg").inputs(vec![browser_input()]).on_press(|ctx| {
            // Fire-and-forget like the JS plugin: a failed Esc is logged, not surfaced.
            let key = browser_key(&ctx.inputs)?;
            if let Err(e) = send_browser_keys(key, "esc") {
                ctx.log(format!("stop-loading: {e}"));
            }
            Ok(())
        }))
        .action(
            Action::button("open-url", "Open URL")
                .icon("icon:lucide/link")
                .inputs(vec![browser_input(), input::text("url", "URL").placeholder("https://example.com")])
                .on_press(|ctx| open_in_browser(&browser_value(&ctx.inputs), &ctx.input_str("url"))),
        )
        .action(hold_repeat("hold-next-tab", "Hold Next Tab", "icon:lucide/notebook-tabs", ("Idle", "▷"), ("Cycling", "▶"), |i| tab_step("ctrl pgdn")(i)))
        .action(hold_repeat("hold-previous-tab", "Hold Previous Tab", "icon:lucide/notebook-tabs", ("Idle", "◁"), ("Cycling", "◀"), |i| tab_step("ctrl pgup")(i)))
        .action(hold_repeat("hold-zoom-in", "Hold Zoom In", "zoom.svg", ("Idle", "＋"), ("Zooming", "🔍"), |i| zoom_step(Zoom::In)(i)))
        .action(hold_repeat("hold-zoom-out", "Hold Zoom Out", "zoom.svg", ("Idle", "－"), ("Zooming", "🔎"), |i| zoom_step(Zoom::Out)(i)))
        .action(
            Action::slider("page-zoom", "Page Zoom", 25.0, 300.0)
                .icon("zoom.svg")
                .step(5.0)
                .unit("%")
                .inputs(vec![browser_input()])
                .initial(100)
                .on_adjust(|ctx| {
                    let browser = browser_key(&ctx.inputs)?;
                    let current = current_zoom(ctx, browser);
                    if let Some(v) = ctx.value {
                        let target = (((v / 5.0).round() as i64) * 5).clamp(25, 300);
                        if target == 100 {
                            zoom_sequence(browser, Zoom::Reset, 1)?;
                        } else if target != current {
                            let dir = if target > current { Zoom::In } else { Zoom::Out };
                            zoom_sequence(browser, dir, (((target - current).abs() as f64 / 5.0).round() as usize).max(1))?;
                        }
                        set_zoom_state(ctx, browser, target);
                        return Ok(());
                    }
                    let Some(delta) = ctx.delta else { return Ok(()) };
                    let step = ctx.input_str("step").trim().parse::<i64>().ok().filter(|s| *s != 0).map_or(5, |s| s.abs());
                    let target = (current + delta as i64 * step).clamp(25, 300);
                    let dir = if target > current { Zoom::In } else { Zoom::Out };
                    zoom_sequence(browser, dir, (((target - current).abs() as f64 / 5.0).round() as usize).max(1))?;
                    set_zoom_state(ctx, browser, target);
                    Ok(())
                }),
        )
        .action(link_wheel("bookmarks-wheel", "Bookmarks", "icon:lucide/bookmark", "bookmark", "🔖", "No {} bookmarks were found", data::bookmarks))
        .action(link_wheel("history-wheel", "Recent History", "icon:lucide/history", "history", "🕘", "No recent {} history items were found", data::history))
        .query("installed_browser", |ctx| {
            let mut apps: Vec<Value> = ctx
                .installed_apps()?
                .into_iter()
                .filter(|app| {
                    let value = app["value"].as_str().unwrap_or("");
                    std::path::Path::new(value).is_absolute() && detect_key(app).is_some()
                })
                .map(|app| {
                    let value = app["value"].as_str().unwrap_or("").to_string();
                    let label = app["label"].as_str().map(String::from).unwrap_or_else(|| value.clone());
                    let family = match detect_key(&app).map(|k| meta(k).family) {
                        Some(Family::Firefox) => "firefox",
                        _ => "chromium",
                    };
                    let keywords: Vec<&str> = [label.as_str(), value.as_str(), family].into_iter().filter(|s| !s.is_empty()).collect();
                    json!({"label": label, "value": value, "keywords": keywords})
                })
                .collect();
            apps.sort_by_key(|a| a["label"].as_str().unwrap_or("").to_lowercase());
            Ok(Value::Array(apps))
        })
        .run()
}
