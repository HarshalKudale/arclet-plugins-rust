//! Discord plugin for KnobDeck — controls the Discord desktop client over its local
//! RPC IPC channel (mute, deafen, PTT, channels, volume, soundboard & more).
//!
//! App ID:     1527677329524916424
//! Public Key: c5e58f44f780eff05911c52c387b9bd111e409cae85b47711a580a5046011aa9

mod common;
mod helpers;
mod rest;
mod rpc;
mod state;

use knobdeck_sdk::input::{self, InputExt};
use knobdeck_sdk::{json, log, Action, Ctx, Error, Plugin, Result, Runtime, Value};
use common::{clear_token, client_id, load_token, refresh_token};
use helpers::*;
use rpc::{Rpc, RpcError};
use state::state;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const RECONNECT_DELAYS_MS: [u64; 6] = [1000, 2000, 4000, 8000, 15_000, 30_000];
const NOT_CONNECTED: &str = "Discord is not connected — open Discord first.";

static RT: OnceLock<Runtime> = OnceLock::new();
static CLIENT: Mutex<Option<Arc<Rpc>>> = Mutex::new(None);
static RECONNECT_N: AtomicUsize = AtomicUsize::new(0);
static IS_CONNECTING: AtomicBool = AtomicBool::new(false);
static RECONNECT_SCHEDULED: AtomicBool = AtomicBool::new(false);

/// join-voice-channel instances: instanceId → configured channelId.
static JOIN_INSTANCES: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);
/// Wheel child id → payload, one map per wheel.
static VOICE_WHEEL: Mutex<Option<HashMap<String, (String, String)>>> = Mutex::new(None); // channel, guild
static MEMBER_WHEEL: Mutex<Option<HashMap<String, String>>> = Mutex::new(None); // user id
static SOUND_WHEEL: Mutex<Option<HashMap<String, (String, Option<String>)>>> = Mutex::new(None); // sound, guild

fn rerr(e: RpcError) -> Error {
    Error::Other(e.message)
}

fn num(x: f64) -> String {
    if x.fract() == 0.0 {
        format!("{}", x as i64)
    } else {
        x.to_string()
    }
}

fn join_instances() -> Vec<(String, Option<String>)> {
    JOIN_INSTANCES.lock().unwrap().iter().flatten().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// Input as a string, with `default` only when the key is absent (JS destructuring default).
fn inp(inputs: &Value, key: &str, default: &str) -> String {
    match &inputs[key] {
        Value::Null => default.into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn truthy(inputs: &Value, key: &str) -> Option<String> {
    inputs[key].as_str().filter(|s| !s.is_empty()).map(String::from)
}

/// `parseFloat(x) || default`, clamped to 0..=200.
fn step_value(text: &str) -> f64 {
    let t = text.trim();
    let end = (0..=t.len()).rev().filter(|&i| t.is_char_boundary(i)).find(|&i| t[..i].parse::<f64>().is_ok());
    let v = end.and_then(|i| t[..i].parse::<f64>().ok()).filter(|v| *v != 0.0).unwrap_or(10.0);
    v.clamp(0.0, 200.0)
}

// ── State fan-out ────────────────────────────────────────────────────────────

fn join_state(channel_id: &Option<String>) -> &'static str {
    let cur = state().current_channel_id.clone();
    if channel_id.is_some() && cur == *channel_id {
        "in"
    } else {
        "idle"
    }
}

fn push_join_states() {
    let Some(rt) = RT.get() else { return };
    for (iid, cid) in join_instances() {
        rt.set_state(&iid, join_state(&cid));
    }
}

fn push_all_states() {
    let Some(rt) = RT.get() else { return };
    {
        let s = state();
        rt.notify("mute", if s.muted { "muted" } else { "unmuted" });
        rt.notify("deafen", if s.deafened { "deafened" } else { "undeafened" });
        rt.notify("voice-mode-toggle", &s.voice_mode);
        rt.notify("leave-voice-channel", if s.current_channel_id.is_some() { "in-call" } else { "idle" });
        rt.notify("video-toggle", if s.video_enabled { "on" } else { "off" });
        rt.notify("stream-toggle", if s.stream_enabled { "on" } else { "off" });
        rt.notify("notifications", if s.notification_count > 0 { "active" } else { "none" });
        rt.notify("output-volume", num(s.output_volume));
    }
    // join-voice-channel is per-instance (each button targets a specific channel).
    push_join_states();
}

// ── Connection management ────────────────────────────────────────────────────

fn client() -> Option<Arc<Rpc>> {
    CLIENT.lock().unwrap().clone()
}

fn schedule_reconnect() {
    if RECONNECT_SCHEDULED.swap(true, Ordering::SeqCst) {
        return; // a reconnect timer is already pending
    }
    let n = RECONNECT_N.fetch_add(1, Ordering::SeqCst);
    let delay = RECONNECT_DELAYS_MS[n.min(RECONNECT_DELAYS_MS.len() - 1)];
    log(format!("reconnect in {delay}ms (attempt {})", n + 1));
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(delay));
        RECONNECT_SCHEDULED.store(false, Ordering::SeqCst);
        connect();
    });
}

/// Connect once; concurrent calls are skipped.
fn connect() {
    if IS_CONNECTING.swap(true, Ordering::SeqCst) {
        return;
    }
    connect_inner();
    IS_CONNECTING.store(false, Ordering::SeqCst);
}

/// Connect with the stored token.
///   • Discord auth error (bad/revoked token) → mark setup required, exit.
///   • Connection error (Discord not running)  → schedule reconnect.
fn connect_inner() {
    let token = load_token();
    log(format!("connecting to Discord RPC (token: {})", if token.is_some() { "yes" } else { "no" }));
    let Some(token) = token else {
        log("no stored RPC token — setup required");
        knobdeck_sdk::mark_setup_required("Sign in to Discord");
        std::process::exit(1);
    };

    let login = |token: &str| Rpc::connect(&client_id()).and_then(|c| c.authenticate(token).map(|_| c));
    let mut result = login(&token);
    if matches!(result, Err(RpcError { code: Some(_), .. })) {
        // Expired or revoked: one renewal through the token worker before asking for setup.
        if let Some(fresh) = refresh_token() {
            log("RPC token renewed");
            result = login(&fresh);
        }
    }
    let c = match result {
        Ok(c) => c,
        Err(RpcError { code: Some(code), message }) => {
            // Discord returned a numeric error code — token is bad (revoked, invalid, wrong scopes).
            log(format!("RPC auth error (code {code}): {message} — setup required"));
            clear_token();
            knobdeck_sdk::mark_setup_required("Discord sign-in expired");
            std::process::exit(1);
        }
        Err(e) => {
            // Connection-level failure — Discord is not running, pipe not found, timeout.
            log(format!("RPC connect failed: {e} — will retry"));
            schedule_reconnect();
            return;
        }
    };

    c.set_on_close(|| {
        *CLIENT.lock().unwrap() = None;
        RECONNECT_N.store(0, Ordering::SeqCst); // retries start fast after a fresh disconnect
        log("Discord RPC disconnected — scheduling reconnect");
        schedule_reconnect();
    });
    on_connected(c);
}

/// After a successful login: fetch initial state, subscribe, wire events.
fn on_connected(c: Arc<Rpc>) {
    *CLIENT.lock().unwrap() = Some(c.clone());
    RECONNECT_N.store(0, Ordering::SeqCst);
    log("Discord RPC connected");

    match c.request("GET_VOICE_SETTINGS", Value::Null) {
        Ok(vs) => {
            let mut s = state();
            s.apply_voice_settings(&vs);
            log(format!("voice settings: mute={} deaf={} mode={}", s.muted, s.deafened, s.voice_mode));
        }
        Err(e) => log(format!("failed to fetch voice settings: {e}")),
    }
    for evt in ["VOICE_SETTINGS_UPDATE", "VOICE_CHANNEL_SELECT", "NOTIFICATION_CREATE"] {
        let _ = c.subscribe(evt);
    }

    // RPC events carry raw snake_case data.
    c.set_on_event(|evt, data| match evt {
        "VOICE_SETTINGS_UPDATE" => {
            let mut s = state();
            s.apply_voice_settings(data);
            if let Some(rt) = RT.get() {
                rt.notify("mute", if s.muted { "muted" } else { "unmuted" });
                rt.notify("deafen", if s.deafened { "deafened" } else { "undeafened" });
                rt.notify("voice-mode-toggle", &s.voice_mode);
                // Keep the output-volume slider in sync from ANY source (Discord UI, hardware key…).
                rt.notify("output-volume", num(s.output_volume));
            }
            log(format!(
                "VOICE_SETTINGS_UPDATE: mute={} deaf={} mode={} inputVol={} outputVol={}",
                s.muted, s.deafened, s.voice_mode, s.input_volume, s.output_volume
            ));
        }
        "VOICE_CHANNEL_SELECT" => {
            let in_call = {
                let mut s = state();
                s.apply_channel_select(data);
                log(format!(
                    "VOICE_CHANNEL_SELECT: channel={} guild={}",
                    s.current_channel_id.as_deref().unwrap_or("none"),
                    s.current_guild_id.as_deref().unwrap_or("none")
                ));
                s.current_channel_id.is_some()
            };
            if let Some(rt) = RT.get() {
                rt.notify("leave-voice-channel", if in_call { "in-call" } else { "idle" });
                push_join_states();
            }
        }
        "NOTIFICATION_CREATE" => {
            let mut s = state();
            s.add_notification(data);
            if let Some(rt) = RT.get() {
                rt.notify("notifications", if s.notification_count > 0 { "active" } else { "none" });
            }
            let preview: String = data["body"].as_str().map(|b| b.chars().take(60).collect()).unwrap_or_else(|| "(no body)".into());
            log(format!("NOTIFICATION_CREATE: \"{preview}\" count={}", s.notification_count));
        }
        _ => {}
    });

    push_all_states();
    log("initial state pushed to app");
}

/// Try an immediate connect if needed, then wait up to `timeout` for a connection.
fn wait_for_connection(timeout: Duration) -> Option<Arc<Rpc>> {
    if let Some(c) = client() {
        return Some(c);
    }
    if !IS_CONNECTING.load(Ordering::SeqCst) {
        log("not connected — attempting immediate connect");
        std::thread::spawn(connect); // the IS_CONNECTING guard prevents duplicates
    }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(c) = client() {
            return Some(c);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    client()
}

/// The connected client, or the standard "open Discord" error.
fn ensure_ready() -> Result<Arc<Rpc>> {
    wait_for_connection(Duration::from_secs(5)).ok_or_else(|| {
        log(format!("ensureReady: {NOT_CONNECTED}"));
        Error::msg(NOT_CONNECTED)
    })
}

// ── RPC helpers ──────────────────────────────────────────────────────────────

fn req(c: &Rpc, cmd: &str, args: Value) -> Result<Value> {
    c.request(cmd, args).map_err(rerr)
}

fn set_voice_settings(c: &Rpc, patch: Value) -> Result {
    req(c, "SET_VOICE_SETTINGS", patch).map(|_| ())
}

fn arr(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

fn guilds_list(c: &Rpc) -> Result<Vec<Value>> {
    let data = req(c, "GET_GUILDS", json!({}))?;
    Ok(if data.is_array() { arr(&data) } else { arr(&data["guilds"]) })
}

fn guild_channels(c: &Rpc, guild_id: &str) -> Result<Vec<Value>> {
    if guild_id.is_empty() {
        return Ok(vec![]);
    }
    Ok(arr(&req(c, "GET_CHANNELS", json!({"guild_id": guild_id}))?["channels"]))
}

fn text_channels(c: &Rpc, guild_id: &str) -> Result<Vec<Value>> {
    Ok(guild_channels(c, guild_id)?.into_iter().filter(|ch| ch["type"] == 0).collect())
}

fn voice_channels(c: &Rpc, guild_id: &str) -> Result<Vec<Value>> {
    Ok(guild_channels(c, guild_id)?.into_iter().filter(|ch| matches!(ch["type"].as_i64(), Some(2 | 13))).collect())
}

fn friend_relationships(c: &Rpc) -> Result<Vec<Value>> {
    // Relationship type 1 = FRIEND.
    Ok(arr(&req(c, "GET_RELATIONSHIPS", Value::Null)?["relationships"]).into_iter().filter(|r| r["type"] == 1).collect())
}

fn soundboard_sounds(guild_id: &str) -> Vec<Value> {
    if !guild_id.is_empty() && guild_id != "__default__" {
        rest::soundboard(guild_id)
    } else {
        rest::default_sounds()
    }
}

fn selected_voice_channel(c: &Rpc) -> Option<Value> {
    let cur = state().current_channel_id.clone()?;
    if let Ok(sel) = req(c, "GET_SELECTED_VOICE_CHANNEL", Value::Null) {
        if ["id", "channel_id", "channelId"].iter().any(|k| sel[k].as_str().map_or(false, |s| !s.is_empty())) {
            return Some(sel);
        }
    }
    req(c, "GET_CHANNEL", json!({"channel_id": cur})).ok()
}

fn current_voice_participants(c: &Rpc) -> Vec<Value> {
    let Some(sel) = selected_voice_channel(c) else { return vec![] };
    let states = if sel["voice_states"].is_array() { arr(&sel["voice_states"]) } else { arr(&sel["voiceStates"]) };
    let from_states: Vec<Value> = states
        .iter()
        .filter_map(|vs| Some(if vs["user"].is_object() { vs["user"].clone() } else { vs["member"]["user"].clone() }).filter(Value::is_object))
        .collect();
    if !from_states.is_empty() {
        return from_states;
    }
    let users = arr(&sel["users"]);
    if !users.is_empty() {
        return users;
    }
    arr(&sel["recipients"])
}

fn set_user_voice_settings(c: &Rpc, user_id: &str, mut args: Value) -> Result {
    args["user_id"] = user_id.into();
    if req(c, "SET_USER_VOICE_SETTINGS", args.clone()).is_err() {
        req(c, "SET_USER_VOICE_SETTINGS", args)?;
    }
    Ok(())
}

fn options(items: Vec<Value>, value: impl Fn(&Value) -> Value, label: impl Fn(&Value) -> String) -> Value {
    Value::Array(items.iter().map(|i| json!({"value": value(i), "label": label(i)})).collect())
}

fn name_of(v: &Value) -> String {
    v["name"].as_str().unwrap_or("").to_string()
}

fn mode_input() -> Value {
    input::select("mode", "Mode", &[("adjust", "Adjust by step"), ("set", "Set exact value"), ("mute", "Toggle mute")])
}

fn guild_input() -> Value {
    input::query_select("guildId", "Server", "discord-get-guilds", &[])
}

fn sound_source_input() -> Value {
    input::select("guildId", "Sound Source", &[("__default__", "Discord Default Sounds")])
        .with("default", "__default__")
        .with("query", "discord-get-guilds-with-default-sounds")
}

// ── Actions ──────────────────────────────────────────────────────────────────

fn voice_toggle(id: &str, label: &str, icon: &str, states: [(&'static str, &'static str, &'static str); 2], flag: fn(&state::State) -> bool, setting: &'static str, apply: fn(&mut state::State, bool)) -> Action {
    let mut a = Action::toggle(id, label).icon(icon);
    for (v, l, i) in states {
        a = a.state(v, l, i);
    }
    let (off, on) = (states[0].0, states[1].0);
    a.initial_with(move |_| Some((if flag(&state()) { on } else { off }).into())).on_press(move |ctx| {
        let c = ensure_ready()?;
        let next = !flag(&state());
        set_voice_settings(&c, json!({ setting: next }))?;
        apply(&mut state(), next);
        ctx.fan_out(if next { on } else { off });
        Ok(())
    })
}

fn hold_voice(id: &str, label: &str, icon: &str, released: (&str, &str), held: (&str, &str), mute_on_start: bool) -> Action {
    Action::hold(id, label)
        .icon(icon)
        .released(released.0, released.1)
        .held(held.0, held.1)
        .on_hold_start(move |ctx| {
            let c = ensure_ready()?;
            set_voice_settings(&c, json!({"mute": mute_on_start}))?;
            ctx.set_state("held");
            Ok(())
        })
        .on_hold_end(move |ctx| {
            let c = ensure_ready()?;
            set_voice_settings(&c, json!({"mute": !mute_on_start}))?;
            ctx.set_state("released");
            Ok(())
        })
}

fn select_voice_channel(c: &Rpc, channel_id: &str, guild_id: &str, force: bool) -> Result {
    let mut args = json!({"channel_id": channel_id, "timeout": 10, "force": force});
    if !guild_id.is_empty() {
        args["guild_id"] = guild_id.into();
    }
    req(c, "SELECT_VOICE_CHANNEL", args).map(|_| ())
}

fn select_text_channel(c: &Rpc, channel_id: &str, guild_id: &str) -> Result {
    let mut args = json!({"channel_id": channel_id, "timeout": 5});
    if !guild_id.is_empty() {
        args["guild_id"] = guild_id.into();
    }
    req(c, "SELECT_TEXT_CHANNEL", args).map(|_| ())
}

fn main() {
    let plugin = Plugin::new(if cfg!(target_os = "macos") { "discord-macos" } else { "discord-windows" }, "Discord")
        .version(env!("CARGO_PKG_VERSION"))
        .platforms(&[std::env::consts::OS]) // one package per OS
        .author("KnobDeck")
        .description("Control Discord — mute, deafen, PTT, channels, volume, soundboard & more.")
        .icon("discord-logo.svg")
        .ack_timeout_ms(5000)
        .setup("discord-setup", Some("setup.html"))
        // onReady fires once after the first sync — all placed instances are known, so
        // runtime.notify() is available immediately for live pushes from RPC events.
        .on_ready(|rt| {
            let _ = RT.set(rt.clone());
            log("plugin ready — starting Discord RPC connection");
            connect();
            Ok(())
        });

    plugin
        // 1–2. Mute, deafen
        .action(voice_toggle("mute", "Mute", "mute_0.png", [("unmuted", "Unmuted", "mute_0.png"), ("muted", "Muted", "mute_1.png")], |s| s.muted, "mute", |s, v| s.muted = v))
        .action(voice_toggle("deafen", "Deafen", "deafen_0.png", [("undeafened", "Undeafened", "deafen_0.png"), ("deafened", "Deafened", "deafen_1.png")], |s| s.deafened, "deaf", |s, v| s.deafened = v))
        // 3–4. Push to talk / mute
        .action(hold_voice("push-to-talk", "Push to Talk", "pushtotalk_0.svg", ("Idle", "pushtotalk_0.svg"), ("Talking", "pushtotalk_1.svg"), false))
        .action(hold_voice("push-to-mute", "Push to Mute", "pushtomute_0.svg", ("Active", "pushtomute_0.svg"), ("Muted", "pushtomute_1.svg"), true))
        // 5. Voice input mode
        .action(
            Action::toggle("voice-mode-toggle", "Voice Input Mode")
                .icon("headphonesAndMicrophone.svg")
                .state("VOICE_ACTIVITY", "Voice Activity", "headphonesAndMicrophone.svg")
                .state("PUSH_TO_TALK", "Push to Talk", "ptttoggle_0.svg")
                .initial_with(|_| Some(state().voice_mode.clone()))
                .on_press(|ctx| {
                    let c = ensure_ready()?;
                    let next = if state().voice_mode == "PUSH_TO_TALK" { "VOICE_ACTIVITY" } else { "PUSH_TO_TALK" };
                    set_voice_settings(&c, json!({"mode": {"type": next}}))?;
                    state().voice_mode = next.into();
                    ctx.fan_out(next);
                    Ok(())
                }),
        )
        // 6a. Join voice channel
        .action(
            Action::toggle("join-voice-channel", "Join Voice Channel")
                .icon("voice_1.png")
                .state("idle", "Join", "voice_1.png")
                .state("in", "In Channel", "voice_0.png")
                .inputs(vec![
                    guild_input(),
                    input::query_select("channelId", "Voice Channel", "discord-get-voice-channels", &["guildId"]).with("showIf", json!({"key": "guildId", "truthy": true})),
                    input::checkbox("force", "Force Join (auto-disconnect current channel first)").default_value(true),
                ])
                .initial_with(|ic| {
                    let channel = truthy(&ic.inputs, "channelId");
                    // Register so VOICE_CHANNEL_SELECT can push per-instance state.
                    JOIN_INSTANCES.lock().unwrap().get_or_insert_with(HashMap::new).insert(ic.instance_id.clone(), channel.clone());
                    Some(join_state(&channel).into())
                })
                .on_press(|ctx| {
                    let channel = truthy(&ctx.inputs, "channelId");
                    let guild = truthy(&ctx.inputs, "guildId").unwrap_or_default();
                    // Already in this channel — button is in the "in" state; do nothing.
                    if channel.is_some() && state().current_channel_id == channel {
                        return Ok(());
                    }
                    let c = ensure_ready()?;
                    let Some(channel) = channel else {
                        ctx.set_error("No voice channel configured.");
                        return Ok(());
                    };
                    select_voice_channel(&c, &channel, &guild, ctx.inputs["force"] != Value::Bool(false))?;
                    {
                        let mut s = state();
                        s.current_channel_id = Some(channel);
                        s.current_guild_id = Some(guild).filter(|g| !g.is_empty());
                    }
                    ctx.set_state("in");
                    // Flip every other join-voice-channel instance back to idle.
                    for (iid, _) in join_instances() {
                        if iid != ctx.instance_id {
                            ctx.runtime().set_state(&iid, "idle");
                        }
                    }
                    ctx.runtime().notify("leave-voice-channel", "in-call");
                    Ok(())
                }),
        )
        // 6b. Leave voice channel
        .action(
            Action::toggle("leave-voice-channel", "Leave Voice Channel")
                .icon("Disconnect.png")
                .state("idle", "Not in Call", "Disconnect.png")
                .state("in-call", "In Call — Disconnect", "end_call.png")
                .initial_with(|_| Some((if state().current_channel_id.is_some() { "in-call" } else { "idle" }).into()))
                .on_press(|ctx| {
                    // Not in any channel — button is in the "idle" state; do nothing.
                    if state().current_channel_id.is_none() {
                        return Ok(());
                    }
                    let c = ensure_ready()?;
                    req(&c, "SELECT_VOICE_CHANNEL", json!({"channel_id": null, "force": true}))?;
                    {
                        let mut s = state();
                        s.current_channel_id = None;
                        s.current_guild_id = None;
                    }
                    ctx.fan_out("idle");
                    for (iid, _) in join_instances() {
                        ctx.runtime().set_state(&iid, "idle");
                    }
                    Ok(())
                }),
        )
        // 7. Text channel
        .action(
            Action::button("text-channel", "Text Channel")
                .icon("textchannel.svg")
                .inputs(vec![
                    guild_input(),
                    input::query_select("channelId", "Text Channel", "discord-get-text-channels", &["guildId"]).with("showIf", json!({"key": "guildId", "truthy": true})),
                ])
                .on_press(|ctx| {
                    let c = ensure_ready()?;
                    let Some(channel) = truthy(&ctx.inputs, "channelId") else {
                        ctx.set_error("No text channel configured.");
                        return Ok(());
                    };
                    select_text_channel(&c, &channel, &truthy(&ctx.inputs, "guildId").unwrap_or_default())
                }),
        )
        // 7b. Server jump
        .action(
            Action::button("server-jump", "Server Jump")
                .icon("textchannel.svg")
                .inputs(vec![
                    guild_input(),
                    input::query_select("channelId", "Landing Channel (optional)", "discord-get-text-channels", &["guildId"]).with("showIf", json!({"key": "guildId", "truthy": true})),
                ])
                .on_press(|ctx| {
                    let c = ensure_ready()?;
                    let Some(guild) = truthy(&ctx.inputs, "guildId") else {
                        ctx.set_error("No server configured.");
                        return Ok(());
                    };
                    let target = match truthy(&ctx.inputs, "channelId") {
                        Some(ch) => ch,
                        None => match find_best_text_channel(&text_channels(&c, &guild)?) {
                            Some(best) => best["id"].as_str().unwrap_or("").to_string(),
                            None => {
                                ctx.set_error("That server has no text channels KnobDeck can open.");
                                return Ok(());
                            }
                        },
                    };
                    select_text_channel(&c, &target, &guild)
                }),
        )
        // 7c. Open direct message
        .action(
            Action::button("open-direct-message", "Open Direct Message")
                .icon("textchannel.svg")
                .inputs(vec![input::query_select("friendId", "Friend", "discord-get-friends", &[])])
                .on_press(|ctx| {
                    let c = ensure_ready()?;
                    let Some(friend) = truthy(&ctx.inputs, "friendId") else {
                        ctx.set_error("No friend configured.");
                        return Ok(());
                    };
                    let Some(rel) = friend_relationships(&c)?.into_iter().find(|r| r["user"]["id"] == friend.as_str()) else {
                        ctx.set_error("That friend is no longer available.");
                        return Ok(());
                    };
                    let channels = arr(&req(&c, "GET_CHANNELS", json!({}))?["channels"]);
                    let Some(dm) = find_direct_message_channel(&channels, &rel) else {
                        ctx.set_error(format!("No direct message thread is open with {}. Open one in Discord first.", format_relationship_label(&rel)));
                        return Ok(());
                    };
                    select_text_channel(&c, dm["id"].as_str().unwrap_or(""), "")
                }),
        )
        // 8–9. Video, stream
        .action(screen_toggle("video-toggle", "Video Toggle", "video_0_off.svg", [("off", "Camera Off", "video_0.svg"), ("on", "Camera On", "video_1.svg")], "TOGGLE_VIDEO", |s| s.video_enabled, |s, v| s.video_enabled = v))
        .action(screen_toggle("stream-toggle", "Stream Toggle", "stream_0.png", [("off", "Share Off", "stream_0.png"), ("on", "Sharing", "stream_1.png")], "TOGGLE_SCREENSHARE", |s| s.stream_enabled, |s, v| s.stream_enabled = v))
        // 10. Volume control
        .action(
            Action::button("volume-control", "Volume Control")
                .icon("volume-2")
                .inputs(vec![
                    input::select("device", "Device", &[("output", "Output (Speakers/Headphones)"), ("input", "Input (Microphone)"), ("both", "Both")]),
                    mode_input(),
                    input::text("step", "Step / Target Volume (0–200)").placeholder("10").with("showIf", json!({"key": "mode", "in": ["adjust", "set"]})),
                ])
                .on_press(|ctx| {
                    let c = ensure_ready()?;
                    let (device, mode) = (inp(&ctx.inputs, "device", "output"), inp(&ctx.inputs, "mode", "adjust"));
                    let step = step_value(&inp(&ctx.inputs, "step", "10"));
                    let (inp_side, out_side) = (device == "input" || device == "both", device == "output" || device == "both");
                    let mut patch = json!({});
                    let mut s = state();
                    match mode.as_str() {
                        "mute" => {
                            if inp_side {
                                s.muted = !s.muted;
                                patch["mute"] = s.muted.into();
                            }
                            if out_side {
                                s.deafened = !s.deafened;
                                patch["deaf"] = s.deafened.into();
                            }
                        }
                        "set" => {
                            if inp_side {
                                patch["input"] = json!({"volume": step});
                            }
                            if out_side {
                                patch["output"] = json!({"volume": step});
                            }
                        }
                        _ => {
                            if inp_side {
                                s.input_volume = (s.input_volume + step).clamp(0.0, 200.0);
                                patch["input"] = json!({"volume": s.input_volume});
                            }
                            if out_side {
                                s.output_volume = (s.output_volume + step).clamp(0.0, 200.0);
                                patch["output"] = json!({"volume": s.output_volume});
                            }
                        }
                    }
                    drop(s);
                    set_voice_settings(&c, patch)
                }),
        )
        // 10b. Output volume slider
        .action(
            Action::slider("output-volume", "Output Volume", 0.0, 200.0)
                .icon("volume-2")
                .unit("%")
                .initial_with(|_| Some(num(state().output_volume)))
                .on_adjust(|ctx| {
                    let c = ensure_ready()?;
                    // A drag sends an absolute target (ctx.value); a +/- stepper sends ctx.delta.
                    let target = match ctx.value {
                        Some(v) => v.round(),
                        None => state().output_volume + ctx.delta.unwrap_or(0.0),
                    };
                    let clamped = target.clamp(0.0, 200.0);
                    ctx.log(format!("output-volume onAdjust: value={:?} delta={:?} target={target} clamped={clamped}", ctx.value, ctx.delta));
                    set_voice_settings(&c, json!({"output": {"volume": clamped}}))?;
                    state().output_volume = clamped;
                    ctx.set_state(num(clamped));
                    Ok(())
                }),
        )
        // 11. User volume
        .action(
            Action::button("user-volume-control", "User Volume")
                .icon("volume-2")
                .inputs(vec![
                    input::query_select("userId", "Call Member", "discord-get-current-voice-users", &[]),
                    mode_input(),
                    input::text("step", "Step / Target (0–200)").placeholder("10").with("showIf", json!({"key": "mode", "in": ["adjust", "set"]})),
                ])
                .on_press(|ctx| {
                    let c = ensure_ready()?;
                    let Some(user) = truthy(&ctx.inputs, "userId") else {
                        ctx.set_error("No call member configured.");
                        return Ok(());
                    };
                    let mode = inp(&ctx.inputs, "mode", "adjust");
                    let step = step_value(&inp(&ctx.inputs, "step", "10"));
                    let (volume, mute) = state().user_volume(&user);
                    let (args, next) = match mode.as_str() {
                        "mute" => (json!({"mute": !mute}), (volume, !mute)),
                        "set" => (json!({"volume": step}), (step, mute)),
                        _ => {
                            let nv = (volume + step).clamp(0.0, 200.0);
                            (json!({"volume": nv}), (nv, mute))
                        }
                    };
                    state().user_volumes.insert(user.clone(), next);
                    set_user_voice_settings(&c, &user, args)
                }),
        )
        // 12. Audio device
        .action(
            Action::button("set-audio-device", "Set Audio Device")
                .icon("headphonesAndMicrophone_1.svg")
                .inputs(vec![
                    input::select("deviceType", "Device Type", &[("output", "Output (Speakers)"), ("input", "Input (Microphone)"), ("both", "Both")]),
                    input::query_select("deviceId", "Device", "discord-get-audio-devices", &["deviceType"]),
                ])
                .on_press(|ctx| {
                    let c = ensure_ready()?;
                    let (kind, device) = (inp(&ctx.inputs, "deviceType", "output"), inp(&ctx.inputs, "deviceId", "default"));
                    let mut patch = json!({});
                    if kind == "input" || kind == "both" {
                        patch["input"] = json!({"device_id": device});
                    }
                    if kind == "output" || kind == "both" {
                        patch["output"] = json!({"device_id": device});
                    }
                    set_voice_settings(&c, patch)
                }),
        )
        // 13. Soundboard
        .action(
            Action::button("soundboard", "Soundboard")
                .icon("soundboard.svg")
                .inputs(vec![sound_source_input(), input::query_select("soundId", "Sound", "discord-get-soundboard-sounds", &["guildId"])])
                .on_press(|ctx| {
                    let c = ensure_ready()?;
                    let Some(sound) = truthy(&ctx.inputs, "soundId") else {
                        ctx.set_error("No sound configured.");
                        return Ok(());
                    };
                    play_sound(&c, &sound, truthy(&ctx.inputs, "guildId").filter(|g| g != "__default__"))
                }),
        )
        // 13b. Voice channel wheel
        .action(
            Action::wheel("voice-channel-wheel", "Voice Channels")
                .icon("voice_1.png")
                .inputs(vec![guild_input(), input::checkbox("force", "Force Join").default_value(true)])
                .on_open(|ctx| {
                    let c = ensure_ready()?;
                    let Some(guild) = truthy(&ctx.inputs, "guildId") else { return ctx.fail("No server configured.") };
                    let channels = voice_channels(&c, &guild)?;
                    if channels.is_empty() {
                        return ctx.fail("That server has no voice channels.");
                    }
                    let mut map = HashMap::new();
                    let cur = state().current_channel_id.clone();
                    let slices = channels
                        .iter()
                        .take(16)
                        .enumerate()
                        .map(|(i, ch)| {
                            let child = format!("voice-{i}");
                            let id = ch["id"].as_str().unwrap_or("").to_string();
                            let icon = if cur.as_deref() == Some(id.as_str()) { "voice_0.png" } else { "voice_1.png" };
                            map.insert(child.clone(), (id, guild.clone()));
                            json!({"id": child, "label": name_of(ch), "icon": icon})
                        })
                        .collect();
                    *VOICE_WHEEL.lock().unwrap() = Some(map);
                    Ok(slices)
                })
                .on_child_press(|ctx| {
                    let c = ensure_ready()?;
                    let target = VOICE_WHEEL.lock().unwrap().as_ref().and_then(|m| m.get(ctx.child_id.as_deref().unwrap_or("")).cloned());
                    let Some((channel, guild)) = target else {
                        ctx.set_error("That voice channel is no longer available.");
                        return Ok(());
                    };
                    select_voice_channel(&c, &channel, &guild, ctx.inputs["force"] != Value::Bool(false))?;
                    {
                        let mut s = state();
                        s.current_channel_id = Some(channel);
                        s.current_guild_id = Some(guild);
                    }
                    push_all_states();
                    Ok(())
                }),
        )
        // 13c. Call member mute wheel
        .action(
            Action::wheel("call-member-mute-wheel", "Call Member Mute")
                .icon("mute_0.png")
                .on_open(|ctx| {
                    let c = ensure_ready()?;
                    if state().current_channel_id.is_none() {
                        return ctx.fail("Join a voice channel first.");
                    }
                    let participants = current_voice_participants(&c);
                    if participants.is_empty() {
                        return ctx.fail("No call members were found.");
                    }
                    let mut map = HashMap::new();
                    let slices = participants
                        .iter()
                        .take(16)
                        .enumerate()
                        .map(|(i, user)| {
                            let child = format!("member-{i}");
                            let uid = user["id"].as_str().unwrap_or("").to_string();
                            let muted = state().user_volume(&uid).1;
                            map.insert(child.clone(), uid);
                            json!({"id": child, "label": display_name_for_user(user), "icon": if muted { "mute_1.png" } else { "mute_0.png" }})
                        })
                        .collect();
                    *MEMBER_WHEEL.lock().unwrap() = Some(map);
                    Ok(slices)
                })
                .on_child_press(|ctx| {
                    let c = ensure_ready()?;
                    let target = MEMBER_WHEEL.lock().unwrap().as_ref().and_then(|m| m.get(ctx.child_id.as_deref().unwrap_or("")).cloned());
                    let Some(user) = target else {
                        ctx.set_error("That call member is no longer available.");
                        return Ok(());
                    };
                    let (volume, mute) = state().user_volume(&user);
                    set_user_voice_settings(&c, &user, json!({"mute": !mute}))?;
                    state().user_volumes.insert(user, (volume, !mute));
                    Ok(())
                }),
        )
        // 13d. Activity preset
        .action(
            Action::button("activity-preset", "Activity Preset")
                .icon("discord.png")
                .inputs(vec![
                    input::text("details", "Details").placeholder("Pair programming"),
                    input::text("stateText", "State").placeholder("In KnobDeck"),
                    input::checkbox("clearExisting", "Clear current activity instead").default_value(false),
                ])
                .on_press(|ctx| {
                    let c = ensure_ready()?;
                    let pid = std::process::id();
                    if ctx.input_bool("clearExisting") {
                        return req(&c, "SET_ACTIVITY", json!({"pid": pid})).map(|_| ());
                    }
                    let (details, state_text) = (ctx.input_str("details"), ctx.input_str("stateText"));
                    if details.is_empty() && state_text.is_empty() {
                        ctx.set_error("Set Details or State, or enable \"Clear current activity instead\".");
                        return Ok(());
                    }
                    let mut activity = json!({"instance": false});
                    if !state_text.is_empty() {
                        activity["state"] = state_text.into();
                    }
                    if !details.is_empty() {
                        activity["details"] = details.into();
                    }
                    req(&c, "SET_ACTIVITY", json!({"pid": pid, "activity": activity})).map(|_| ())
                }),
        )
        // 13e. Soundboard wheel
        .action(
            Action::wheel("soundboard-wheel", "Soundboard Wheel")
                .icon("soundboard.svg")
                .inputs(vec![sound_source_input()])
                .on_open(|ctx| {
                    ensure_ready()?;
                    let guild = truthy(&ctx.inputs, "guildId").unwrap_or_else(|| "__default__".into());
                    let sounds = soundboard_sounds(&guild);
                    if sounds.is_empty() {
                        return ctx.fail("No soundboard sounds were found for that source.");
                    }
                    let mut map = HashMap::new();
                    let slices = sounds
                        .iter()
                        .take(16)
                        .enumerate()
                        .map(|(i, sound)| {
                            let child = format!("sound-{i}");
                            map.insert(child.clone(), (sound_id(sound), Some(guild.clone()).filter(|g| g != "__default__")));
                            let label = sound["name"].as_str().filter(|n| !n.is_empty()).map(String::from).unwrap_or_else(|| format!("Sound {}", i + 1));
                            json!({"id": child, "label": label, "icon": "soundboard.svg"})
                        })
                        .collect();
                    *SOUND_WHEEL.lock().unwrap() = Some(map);
                    Ok(slices)
                })
                .on_child_press(|ctx| {
                    let c = ensure_ready()?;
                    let target = SOUND_WHEEL.lock().unwrap().as_ref().and_then(|m| m.get(ctx.child_id.as_deref().unwrap_or("")).cloned());
                    let Some((sound, guild)) = target else {
                        ctx.set_error("That sound is no longer available.");
                        return Ok(());
                    };
                    play_sound(&c, &sound, guild)
                }),
        )
        // 14. Notifications
        .action(
            Action::toggle("notifications", "Notifications")
                .icon("notifications.svg")
                .state("none", "No Alerts", "notifications.svg")
                .state("active", "Unread!", "notifications.svg")
                .inputs(vec![input::select("onPress", "On Press Action", &[("nothing", "Do nothing"), ("clear", "Clear count"), ("open_channel", "Open most recent channel")])])
                .initial_with(|_| Some((if state().notification_count > 0 { "active" } else { "none" }).into()))
                .on_press(|ctx| {
                    let c = ensure_ready()?;
                    match inp(&ctx.inputs, "onPress", "clear").as_str() {
                        "clear" => {
                            state().clear_notifications();
                            ctx.fan_out("none");
                        }
                        "open_channel" => {
                            let latest = state().recent_notifications.first().cloned();
                            if let Some((Some(channel), guild)) = latest {
                                let _ = select_text_channel(&c, &channel, &guild.unwrap_or_default());
                            }
                            state().clear_notifications();
                            ctx.fan_out("none");
                        }
                        _ => {} // 'nothing' → leave state
                    }
                    Ok(())
                }),
        )
        // ── Queries (config-time dynamic dropdowns) ──
        .query("discord-get-guilds", |_| {
            let c = connected_for_query()?;
            Ok(options(guilds_list(&c)?, |g| g["id"].clone(), |g| name_of(g)))
        })
        .query("discord-get-guilds-with-default-sounds", |_| {
            let c = connected_for_query()?;
            let mut out = vec![json!({"value": "__default__", "label": "Discord Default Sounds"})];
            out.extend(arr(&options(guilds_list(&c)?, |g| g["id"].clone(), |g| name_of(g))));
            Ok(Value::Array(out))
        })
        // Discord channel type 2 = GUILD_VOICE, 13 = GUILD_STAGE_VOICE.
        .query("discord-get-voice-channels", |ctx| {
            let Some(guild) = truthy(&ctx.inputs, "guildId") else { return Ok(json!([])) };
            let c = connected_for_query()?;
            Ok(options(voice_channels(&c, &guild)?, |ch| ch["id"].clone(), name_of))
        })
        .query("discord-get-text-channels", |ctx| {
            let Some(guild) = truthy(&ctx.inputs, "guildId") else { return Ok(json!([])) };
            let c = connected_for_query()?;
            Ok(options(text_channels(&c, &guild)?, |ch| ch["id"].clone(), |ch| format!("# {}", name_of(ch))))
        })
        .query("discord-get-friends", |_| {
            let c = connected_for_query()?;
            Ok(options(friend_relationships(&c)?, |r| r["user"]["id"].clone(), format_relationship_label))
        })
        .query("discord-get-current-voice-users", |_| {
            let c = connected_for_query()?;
            if state().current_channel_id.is_none() {
                return Ok(json!([]));
            }
            Ok(options(current_voice_participants(&c), |u| u["id"].clone(), display_name_for_user))
        })
        .query("discord-get-audio-devices", |ctx| {
            let c = connected_for_query()?;
            let kind = inp(&ctx.inputs, "deviceType", "output");
            let vs = req(&c, "GET_VOICE_SETTINGS", Value::Null)?;
            let mut s = state();
            s.apply_voice_settings(&vs);
            let devices: Vec<Value> = match kind.as_str() {
                "input" => s.input_devices.clone(),
                "both" => {
                    // Union keyed by device id: later entries replace earlier ones in place.
                    let mut out: Vec<Value> = vec![];
                    for d in s.input_devices.iter().chain(s.output_devices.iter()) {
                        match out.iter_mut().find(|o| o["id"] == d["id"]) {
                            Some(slot) => *slot = d.clone(),
                            None => out.push(d.clone()),
                        }
                    }
                    out
                }
                _ => s.output_devices.clone(),
            };
            drop(s);
            let mut opts = arr(&options(devices, |d| d["id"].clone(), format_device_label));
            if !opts.iter().any(|d| d["value"] == "default") {
                opts.insert(0, json!({"value": "default", "label": "System Default (Default)"}));
            }
            Ok(Value::Array(opts))
        })
        .query("discord-get-soundboard-sounds", |ctx| {
            connected_for_query()?;
            let guild = truthy(&ctx.inputs, "guildId").unwrap_or_else(|| "__default__".into());
            Ok(options(
                soundboard_sounds(&guild),
                |s| json!(sound_id(s)),
                |s| s["name"].as_str().filter(|n| !n.is_empty()).map(String::from).unwrap_or_else(|| sound_id(s)),
            ))
        })
        .run()
}

fn connected_for_query() -> Result<Arc<Rpc>> {
    wait_for_connection(Duration::from_secs(5)).ok_or_else(|| Error::msg(NOT_CONNECTED))
}

fn sound_id(sound: &Value) -> String {
    ["sound_id", "id"].iter().find_map(|k| sound[k].as_str().filter(|s| !s.is_empty())).unwrap_or("").to_string()
}

fn play_sound(c: &Rpc, sound: &str, guild: Option<String>) -> Result {
    let mut args = json!({"sound_id": sound});
    if let Some(g) = guild {
        args["guild_id"] = g.into();
    }
    req(c, "PLAY_SOUNDBOARD_SOUND", args).map(|_| ())
}

/// Video / screenshare toggle: needs a call; tries the dedicated RPC command, falls back to voice settings.
fn screen_toggle(id: &str, label: &str, icon: &str, states: [(&'static str, &'static str, &'static str); 2], command: &'static str, flag: fn(&state::State) -> bool, apply: fn(&mut state::State, bool)) -> Action {
    let mut a = Action::toggle(id, label).icon(icon);
    for (v, l, i) in states {
        a = a.state(v, l, i);
    }
    a.initial_with(move |_| Some((if flag(&state()) { "on" } else { "off" }).into())).on_press(move |ctx: &Ctx| {
        let c = ensure_ready()?;
        if state().current_channel_id.is_none() {
            ctx.set_error("Not in a voice channel — join a call first.");
            return Ok(());
        }
        let next = !flag(&state());
        if req(&c, command, json!({})).is_err() {
            set_voice_settings(&c, json!({}))?;
        }
        apply(&mut state(), next);
        ctx.fan_out(if next { "on" } else { "off" });
        Ok(())
    })
}
