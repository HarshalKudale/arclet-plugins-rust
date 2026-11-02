//! VLC plugin for KnobDeck — controls VLC through its built-in HTTP interface.

use knobdeck_sdk::input::{self, InputExt};
use knobdeck_sdk::{env_or, load_env, log, Action, Ctx, Error, Plugin, Result, Runtime, Value};
use base64::Engine;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_millis(2500);

#[derive(Default, Clone)]
struct State {
    reachable: bool,
    playing: bool,
    paused: bool,
    fullscreen: bool,
}

static STATE: Mutex<State> = Mutex::new(State { reachable: false, playing: false, paused: false, fullscreen: false });
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn base() -> String {
    format!("http://{}:{}", env_or("VLC_HOST", "127.0.0.1"), env_or("VLC_PORT", "8080"))
}

fn vlc_request(command: Option<&str>, params: &[(&str, &str)]) -> Result<Value> {
    let base = base();
    let mut req = ureq::get(&format!("{base}/requests/status.json")).timeout(Duration::from_secs(10));
    let password = env_or("VLC_PASSWORD", "");
    if !password.is_empty() {
        let token = base64::engine::general_purpose::STANDARD.encode(format!(":{password}"));
        req = req.set("Authorization", &format!("Basic {token}"));
    }
    if let Some(c) = command {
        req = req.query("command", c);
    }
    for (k, v) in params.iter().filter(|(_, v)| !v.is_empty()) {
        req = req.query(k, v);
    }
    match req.call() {
        Ok(r) => r.into_json().map_err(|e| Error::msg(format!("VLC returned invalid JSON: {e}"))),
        Err(ureq::Error::Status(401, _)) => {
            knobdeck_sdk::mark_setup_required("VLC rejected the HTTP password");
            Err(Error::msg("VLC rejected the configured HTTP password."))
        }
        Err(ureq::Error::Status(code, _)) => Err(Error::msg(format!("VLC returned HTTP {code}."))),
        Err(e) => Err(Error::msg(format!("VLC is not reachable at {base}. ({e})"))),
    }
}

fn apply_status(status: Option<&Value>) {
    *STATE.lock().unwrap() = match status {
        None => State::default(),
        Some(s) => {
            let mode = s["state"].as_str().unwrap_or("").to_lowercase();
            State {
                reachable: true,
                playing: mode == "playing",
                paused: mode == "paused",
                fullscreen: matches!(&s["fullscreen"], Value::Bool(true))
                    || s["fullscreen"].as_str() == Some("1")
                    || s["fullscreen"].as_i64() == Some(1),
            }
        }
    };
}

fn playback(s: &State) -> &'static str {
    if s.playing {
        "playing"
    } else if s.paused {
        "paused"
    } else {
        "stopped"
    }
}

fn push_all_states() {
    let Some(rt) = RUNTIME.get() else { return };
    let s = STATE.lock().unwrap().clone();
    rt.notify("playback-status", if s.reachable { playback(&s) } else { "offline" });
    rt.notify("play-pause", playback(&s));
    rt.notify("fullscreen", if s.fullscreen { "on" } else { "off" });
}

fn poll_status() {
    match vlc_request(None, &[]) {
        Ok(s) => apply_status(Some(&s)),
        Err(e) => {
            log(format!("VLC poll: {e}"));
            apply_status(None);
        }
    }
    push_all_states();
}

fn send_command(command: &str, params: &[(&str, &str)]) -> Result {
    let status = vlc_request(Some(command), params)?;
    apply_status(Some(&status));
    push_all_states();
    Ok(())
}

fn require_reachable(ctx: &Ctx) -> Result {
    if STATE.lock().unwrap().reachable {
        return Ok(());
    }
    ctx.fail("VLC is offline. Make sure VLC is open and its Web interface is enabled.")
}

/// A button that issues one VLC command.
fn command_button(id: &str, label: &str, icon: &str, command: &'static str, params: &'static [(&'static str, &'static str)]) -> Action {
    Action::button(id, label).icon(icon).on_press(move |ctx| {
        require_reachable(ctx)?;
        send_command(command, params)
    })
}

fn main() {
    load_env();
    Plugin::new(if cfg!(target_os = "macos") { "vlc-macos" } else { "vlc-windows" }, "VLC")
        .version("1.0.0")
        .platforms(&[std::env::consts::OS]) // one package per OS
        .author("KnobDeck")
        .description("Control VLC over its built-in HTTP interface.")
        .ack_timeout_ms(5000)
        .setup("vlc-setup", Some("setup.html"))
        .on_ready(|rt| {
            let _ = RUNTIME.set(rt.clone());
            log(format!("VLC plugin ready — polling {}", base()));
            let rt = rt.clone();
            std::thread::spawn(move || loop {
                if rt.has_instances() {
                    poll_status();
                }
                std::thread::sleep(POLL_INTERVAL);
            });
            Ok(())
        })
        .action(
            Action::toggle("playback-status", "Playback Status")
                .icon("play")
                .state("offline", "Offline", "triangle-alert")
                .state("stopped", "Stopped", "square")
                .state("paused", "Paused", "pause")
                .state("playing", "Playing", "play")
                .initial("offline")
                .on_press(|ctx| {
                    poll_status();
                    let s = STATE.lock().unwrap().clone();
                    ctx.fan_out(if s.reachable { playback(&s) } else { "offline" });
                    Ok(())
                }),
        )
        .action(
            Action::toggle("play-pause", "Play / Pause")
                .icon("play")
                .state("stopped", "Stopped", "play")
                .state("paused", "Paused", "pause")
                .state("playing", "Playing", "play")
                .initial("stopped")
                .on_press(|ctx| {
                    require_reachable(ctx)?;
                    send_command("pl_pause", &[])?;
                    ctx.fan_out(playback(&STATE.lock().unwrap()));
                    Ok(())
                }),
        )
        .action(command_button("stop", "Stop", "square", "pl_stop", &[]))
        .action(command_button("next", "Next", "skip-forward", "pl_next", &[]))
        .action(command_button("previous", "Previous", "skip-back", "pl_previous", &[]))
        .action(
            Action::toggle("fullscreen", "Fullscreen")
                .icon("maximize")
                .state("off", "Windowed", "square")
                .state("on", "Fullscreen", "maximize")
                .initial("off")
                .on_press(|ctx| {
                    require_reachable(ctx)?;
                    send_command("fullscreen", &[])?;
                    ctx.fan_out(if STATE.lock().unwrap().fullscreen { "on" } else { "off" });
                    Ok(())
                }),
        )
        .action(command_button("seek-backward", "Seek Back 10s", "rewind", "seek", &[("val", "-10S")]))
        .action(command_button("seek-forward", "Seek Forward 10s", "fast-forward", "seek", &[("val", "+10S")]))
        .action(command_button("volume-down", "Volume Down", "volume-1", "volume", &[("val", "-20")]))
        .action(command_button("volume-up", "Volume Up", "volume-2", "volume", &[("val", "+20")]))
        .action(
            Action::button("open-url", "Open Stream / URL")
                .icon("link")
                .inputs(vec![input::text("url", "Media URL").placeholder("https://example.com/video.mp4")])
                .on_press(|ctx| {
                    require_reachable(ctx)?;
                    let url = ctx.input_str("url").trim().to_string();
                    if url.is_empty() {
                        return ctx.fail("Media URL is required");
                    }
                    send_command("in_play", &[("input", &url)])
                }),
        )
        .run()
}
