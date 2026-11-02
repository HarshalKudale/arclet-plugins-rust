//! Shared Chrome / Edge / Brave / Firefox / Zen browser controls for KnobDeck:
//! shortcuts (sent to the focused browser via the host), URL opening, hold-to-repeat
//! tab cycling and zoom, a page-zoom slider, and bookmark / history wheels.

mod browsers;
mod data;
mod native;

use knobdeck_sdk::input::{self, InputExt};
use knobdeck_sdk::{focus_app, json, send_keys, Action, Ctx, Error, Plugin, Result, Value};
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

/// Run `action` through the browser's own interface when it has one
/// (`native.rs`), else press `keys`. Shortcuts are the last resort (CLAUDE.md).
fn browser_action(browser: &str, action: &str, keys: &str) -> Result {
    activate_browser(browser)?;
    if native::try_run(browser, action) {
        return Ok(());
    }
    send_keys(keys)
}

fn open_in_browser(browser: &str, url: &str) -> Result {
    let exe = ensure_executable(browser)?;
    let target = url.trim();
    if target.is_empty() {
        return Err(Error::msg("URL is required"));
    }
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new(exe);
        cmd.arg(target);
        cmd.creation_flags(0x0800_0000 | 0x0000_0008); // CREATE_NO_WINDOW | DETACHED_PROCESS
        cmd
    };
    // macOS: `exe` is a `.app` bundle; `open -a` launches it or hands the URL to the running one.
    #[cfg(not(windows))]
    let mut cmd = {
        let mut cmd = Command::new("/usr/bin/open");
        cmd.arg("-a").arg(exe).arg(target);
        cmd
    };
    let mut child = cmd.spawn()?;
    // `open` exits at once; reap it so it doesn't linger as a zombie. On Windows the
    // browser itself is the child and is intentionally not waited on.
    #[cfg(not(windows))]
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    #[cfg(windows)]
    let _ = &mut child;
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
        Zoom::Reset => send_browser_keys(browser, "mod 0"),
        Zoom::In | Zoom::Out => {
            let key = if matches!(dir, Zoom::In) { "mod equals" } else { "mod minus" };
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
fn basic_button(id: &'static str, label: &str, icon: &str, shortcut: fn(&Value) -> Result<String>) -> Action {
    Action::button(id, label).icon(icon).inputs(vec![browser_input()]).on_press(move |ctx| {
        browser_action(&browser_value(&ctx.inputs), id, &shortcut(&ctx.inputs)?)
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

fn tab_step(action: &'static str, keys: &'static str) -> impl Fn(&Value) -> Result {
    move |inputs| browser_action(&browser_value(inputs), action, keys)
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

/// `windows` on Windows, `mac` on macOS.
const fn per_os(windows: &'static str, mac: &'static str) -> &'static str {
    if cfg!(windows) {
        windows
    } else {
        mac
    }
}

/// Windows chord (same in every browser), or the macOS chord for the selected browser's family.
fn per_family(inputs: &Value, windows: &'static str, mac_chromium: &'static str, mac_firefox: &'static str) -> Result<&'static str> {
    if cfg!(windows) {
        return Ok(windows);
    }
    Ok(match meta(browser_key(inputs)?).family {
        Family::Chromium => mac_chromium,
        Family::Firefox => mac_firefox,
    })
}

// Ctrl+Tab works in every macOS browser; Ctrl+PgDn needs fn on Mac keyboards.
const NEXT_TAB: &str = per_os("ctrl pgdn", "ctrl tab");
const PREV_TAB: &str = per_os("ctrl pgup", "ctrl shift tab");

fn main() {
    let mut plugin = Plugin::new(if cfg!(target_os = "macos") { "browser-plugin-macos" } else { "browser-plugin-windows" }, "BrowserPlugin")
        .version("1.0.0")
        .platforms(&[std::env::consts::OS]) // one package per OS
        .author("KnobDeck")
        .description("Shared Chrome, Edge, Brave, Firefox and Zen controls, wheels, and zoom actions on Windows and macOS.")
        .icon("globe")
        .ack_timeout_ms(5000);

    // (id, label, icon, shortcut). `mod` is Ctrl on Windows and ⌘ on macOS; the
    // chords that differ by more than that are picked per OS / browser family.
    type Shortcut = fn(&Value) -> Result<String>;
    let basics: Vec<(&str, &str, &str, Shortcut)> = vec![
        ("new-tab", "New Tab", "notebook-tabs", |_| Ok("mod t".into())),
        ("new-window", "New Window", "app-window", |_| Ok("mod n".into())),
        ("new-private-window", "New Private / Incognito Window", "app-window", |i| Ok(meta(browser_key(i)?).private_shortcut.into())),
        ("close-tab", "Close Tab", "notebook-tabs", |_| Ok("mod w".into())),
        ("reopen-closed-tab", "Reopen Closed Tab", "notebook-tabs", |_| Ok("mod shift t".into())),
        ("next-tab", "Next Tab", "notebook-tabs", |_| Ok(NEXT_TAB.into())),
        ("previous-tab", "Previous Tab", "notebook-tabs", |_| Ok(PREV_TAB.into())),
        ("reload-tab", "Reload Tab", "refresh-cw", |_| Ok("mod r".into())),
        ("hard-reload-tab", "Hard Reload", "refresh-cw", |i| Ok(meta(browser_key(i)?).hard_reload_shortcut.into())),
        ("go-back", "Back", "arrow-left", |_| Ok(per_os("alt left", "cmd lbracket").into())),
        ("go-forward", "Forward", "arrow-right", |_| Ok(per_os("alt right", "cmd rbracket").into())),
        ("focus-address-bar", "Focus Address Bar", "search", |_| Ok("mod l".into())),
        ("open-downloads", "Open Downloads", "download", |i| Ok(per_family(i, "ctrl j", "cmd shift j", "cmd j")?.into())),
        ("open-history", "Open History", "history", |i| Ok(per_family(i, "ctrl h", "cmd y", "cmd shift h")?.into())),
        ("open-bookmarks-manager", "Open Bookmarks Manager", "bookmark", |i| Ok(per_family(i, "ctrl shift o", "cmd alt b", "cmd shift o")?.into())),
        ("bookmark-page", "Bookmark Page", "bookmark", |_| Ok("mod d".into())),
        ("find-in-page", "Find in Page", "search", |_| Ok("mod f".into())),
        ("print-page", "Print Page", "printer", |_| Ok("mod p".into())),
        ("save-page", "Save Page", "save", |_| Ok("mod s".into())),
        ("toggle-fullscreen", "Toggle Fullscreen", "maximize", |_| Ok(per_os("f11", "ctrl cmd f").into())),
    ];
    for (id, label, icon, shortcut) in basics {
        plugin = plugin.action(basic_button(id, label, icon, shortcut));
    }

    plugin
        .action(
            Action::button("custom-keybind", "Custom Keybind")
                .icon("keyboard")
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
            if let Err(e) = browser_action(key, "stop-loading", "esc") {
                ctx.log(format!("stop-loading: {e}"));
            }
            Ok(())
        }))
        .action(
            Action::button("open-url", "Open URL")
                .icon("link")
                .inputs(vec![browser_input(), input::text("url", "URL").placeholder("https://example.com")])
                .on_press(|ctx| open_in_browser(&browser_value(&ctx.inputs), &ctx.input_str("url"))),
        )
        .action(hold_repeat("hold-next-tab", "Hold Next Tab", "notebook-tabs", ("Idle", "chevron-right"), ("Cycling", "chevrons-right"), |i| tab_step("next-tab", NEXT_TAB)(i)))
        .action(hold_repeat("hold-previous-tab", "Hold Previous Tab", "notebook-tabs", ("Idle", "chevron-left"), ("Cycling", "chevrons-left"), |i| tab_step("previous-tab", PREV_TAB)(i)))
        .action(hold_repeat("hold-zoom-in", "Hold Zoom In", "zoom.svg", ("Idle", "zoom-in"), ("Zooming", "zoom-in"), |i| zoom_step(Zoom::In)(i)))
        .action(hold_repeat("hold-zoom-out", "Hold Zoom Out", "zoom.svg", ("Idle", "zoom-out"), ("Zooming", "zoom-out"), |i| zoom_step(Zoom::Out)(i)))
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
        .action(link_wheel("bookmarks-wheel", "Bookmarks", "bookmark", "bookmark", "bookmark", "No {} bookmarks were found", data::bookmarks))
        .action(link_wheel("history-wheel", "Recent History", "history", "history", "history", "No recent {} history items were found", data::history))
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
