//! OBS Studio plugin for KnobDeck — controls OBS via obs-websocket v5.
//!
//! Config: run the plugin setup flow (or write `.env` with OBS_HOST / OBS_PORT / OBS_PASSWORD).

mod obs_ws;

use knobdeck_sdk::input::{self, InputExt};
use knobdeck_sdk::{env_or, json, load_env, log, Action, Ctx, Plugin, Result, Runtime, Value};
use obs_ws::{call, connected};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Default, Clone)]
struct Cache {
    recording: bool,
    record_paused: bool,
    streaming: bool,
    vcam: bool,
    replay_buffer: bool,
    studio_mode: bool,
}

static CACHE: Mutex<Cache> = Mutex::new(Cache {
    recording: false,
    record_paused: false,
    streaming: false,
    vcam: false,
    replay_buffer: false,
    studio_mode: false,
});
/// instanceId → (actionId, inputs); filled by `initial` at sync time and refreshed on press.
static INSTANCES: Mutex<Option<HashMap<String, (String, Value)>>> = Mutex::new(None);
/// instanceId → scene item id, for matching SceneItemEnableStateChanged.
static SCENE_ITEM_IDS: Mutex<Option<HashMap<String, i64>>> = Mutex::new(None);
static RT: OnceLock<Runtime> = OnceLock::new();

fn pick(b: bool, yes: &'static str, no: &'static str) -> &'static str {
    if b {
        yes
    } else {
        no
    }
}

fn register(instance_id: &str, action: &str, inputs: &Value) {
    INSTANCES.lock().unwrap().get_or_insert_with(HashMap::new).insert(instance_id.into(), (action.into(), inputs.clone()));
}

fn instances() -> Vec<(String, String, Value)> {
    INSTANCES.lock().unwrap().iter().flatten().map(|(i, (a, v))| (i.clone(), a.clone(), v.clone())).collect()
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

fn parse_int(text: &str) -> Option<i64> {
    let t = text.trim();
    let end = t.char_indices().take_while(|&(i, c)| c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))).last().map(|(i, c)| i + c.len_utf8())?;
    t[..end].parse().ok()
}

fn notify_globals() {
    let Some(rt) = RT.get() else { return };
    let c = CACHE.lock().unwrap().clone();
    rt.notify("record", pick(c.recording, "recording", "stopped"));
    rt.notify("record-pause", pick(c.record_paused, "paused", "recording"));
    rt.notify("stream", pick(c.streaming, "streaming", "stopped"));
    rt.notify("virtual-cam", pick(c.vcam, "on", "off"));
    rt.notify("replay-buffer", pick(c.replay_buffer, "active", "stopped"));
    rt.notify("studio-mode", pick(c.studio_mode, "on", "off"));
}

/// Fetch all global statuses and push them to placed instances.
fn sync_obs_state() {
    if !connected() {
        return;
    }
    // Fetch first (blocking calls), then store, so events never wait on a held lock.
    let ok = |t: &str| call(t, json!({})).ok();
    let (rec, stream, vcam, rb, studio) =
        (ok("GetRecordStatus"), ok("GetStreamStatus"), ok("GetVirtualCamStatus"), ok("GetReplayBufferStatus"), ok("GetStudioModeEnabled"));
    {
        let mut c = CACHE.lock().unwrap();
        if let Some(r) = rec {
            c.recording = r["outputActive"].as_bool().unwrap_or(false);
            c.record_paused = r["outputPaused"].as_bool().unwrap_or(false);
        }
        if let Some(r) = stream {
            c.streaming = r["outputActive"].as_bool().unwrap_or(false);
        }
        if let Some(r) = vcam {
            c.vcam = r["outputActive"].as_bool().unwrap_or(false);
        }
        if let Some(r) = rb {
            c.replay_buffer = r["outputActive"].as_bool().unwrap_or(false);
        }
        if let Some(r) = studio {
            c.studio_mode = r["studioModeEnabled"].as_bool().unwrap_or(false);
        }
    }
    if RT.get().is_some() {
        notify_globals();
        sync_per_instance_states();
    }
}

/// Per-instance toggles (source-visibility, audio-mixer, filter) query their actual state.
fn sync_per_instance_states() {
    let Some(rt) = RT.get() else { return };
    for (iid, action, inputs) in instances() {
        let r: Result = (|| {
            match action.as_str() {
                "source-visibility" => {
                    if s(&inputs, "scene").is_empty() || s(&inputs, "source").is_empty() {
                        return Ok(());
                    }
                    let id = call("GetSceneItemId", json!({"sceneName": inputs["scene"], "sourceName": inputs["source"]}))?["sceneItemId"].as_i64().unwrap_or(0);
                    SCENE_ITEM_IDS.lock().unwrap().get_or_insert_with(HashMap::new).insert(iid.clone(), id);
                    let enabled = call("GetSceneItemEnabled", json!({"sceneName": inputs["scene"], "sceneItemId": id}))?["sceneItemEnabled"].as_bool().unwrap_or(true);
                    rt.set_state(&iid, pick(enabled, "visible", "hidden"));
                }
                "audio-mixer" => {
                    if s(&inputs, "source").is_empty() {
                        return Ok(());
                    }
                    let muted = call("GetInputMute", json!({"inputName": inputs["source"]}))?["inputMuted"].as_bool().unwrap_or(false);
                    rt.set_state(&iid, pick(muted, "muted", "unmuted"));
                }
                "filter" => {
                    if s(&inputs, "source").is_empty() || s(&inputs, "filter").is_empty() {
                        return Ok(());
                    }
                    let on = call("GetSourceFilter", json!({"sourceName": inputs["source"], "filterName": inputs["filter"]}))?["filterEnabled"].as_bool().unwrap_or(true);
                    rt.set_state(&iid, pick(on, "enabled", "disabled"));
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(e) = r {
            log(format!("OBS: syncPerInstanceStates({iid}): {e}"));
        }
    }
}

fn on_closed() {
    *CACHE.lock().unwrap() = Cache::default();
    notify_globals();
}

fn on_event(name: &str, d: &Value) {
    let flag = |k: &str| d[k].as_bool().unwrap_or(false);
    match name {
        "RecordStateChanged" => {
            let mut c = CACHE.lock().unwrap();
            c.recording = flag("outputActive");
            c.record_paused = flag("outputPaused");
            drop(c);
            if let Some(rt) = RT.get() {
                rt.notify("record", pick(flag("outputActive"), "recording", "stopped"));
                rt.notify("record-pause", pick(flag("outputPaused"), "paused", "recording"));
            }
        }
        "StreamStateChanged" => {
            CACHE.lock().unwrap().streaming = flag("outputActive");
            RT.get().map(|rt| rt.notify("stream", pick(flag("outputActive"), "streaming", "stopped")));
        }
        "VirtualcamStateChanged" => {
            CACHE.lock().unwrap().vcam = flag("outputActive");
            RT.get().map(|rt| rt.notify("virtual-cam", pick(flag("outputActive"), "on", "off")));
        }
        "ReplayBufferStateChanged" => {
            CACHE.lock().unwrap().replay_buffer = flag("outputActive");
            RT.get().map(|rt| rt.notify("replay-buffer", pick(flag("outputActive"), "active", "stopped")));
        }
        "StudioModeStateChanged" => {
            CACHE.lock().unwrap().studio_mode = flag("studioModeEnabled");
            RT.get().map(|rt| rt.notify("studio-mode", pick(flag("studioModeEnabled"), "on", "off")));
        }
        "SceneItemEnableStateChanged" => {
            let Some(rt) = RT.get() else { return };
            let ids = SCENE_ITEM_IDS.lock().unwrap().clone().unwrap_or_default();
            for (iid, action, inputs) in instances() {
                if action == "source-visibility" && s(&inputs, "scene") == s(d, "sceneName") && ids.get(&iid) == d["sceneItemId"].as_i64().as_ref() {
                    rt.set_state(&iid, pick(flag("sceneItemEnabled"), "visible", "hidden"));
                }
            }
        }
        "InputMuteStateChanged" => {
            let Some(rt) = RT.get() else { return };
            for (iid, action, inputs) in instances() {
                if action == "audio-mixer" && s(&inputs, "source") == s(d, "inputName") {
                    rt.set_state(&iid, pick(flag("inputMuted"), "muted", "unmuted"));
                }
            }
        }
        "SourceFilterEnableStateChanged" => {
            let Some(rt) = RT.get() else { return };
            for (iid, action, inputs) in instances() {
                if action == "filter" && s(&inputs, "source") == s(d, "sourceName") && s(&inputs, "filter") == s(d, "filterName") {
                    rt.set_state(&iid, pick(flag("filterEnabled"), "enabled", "disabled"));
                }
            }
        }
        _ => {}
    }
}

fn require_obs(ctx: &Ctx) -> Result {
    if connected() {
        return Ok(());
    }
    ctx.fail("OBS is not connected. Is OBS running with WebSocket enabled?")
}

fn require_input(ctx: &Ctx, key: &str, msg: &str) -> Result<String> {
    match ctx.input_str(key) {
        v if v.is_empty() => ctx.fail(msg),
        v => Ok(v),
    }
}

/// Global on/off toggle: pressing calls `stop` when the current state is `active_state`, else `start`.
fn global_toggle(id: &str, label: &str, icon: &str, states: [(&str, &str, &str); 2], start: &'static str, stop: &'static str, initial: fn(&Cache) -> bool) -> Action {
    let mut a = Action::toggle(id, label).icon(icon);
    for (v, l, i) in states {
        a = a.state(v, l, i);
    }
    let (off, on) = (states[0].0.to_string(), states[1].0.to_string());
    let on2 = on.clone();
    a.initial_with(move |_| Some(if initial(&CACHE.lock().unwrap()) { on2.clone() } else { off.clone() })).on_press(move |ctx| {
        require_obs(ctx)?;
        // State is pushed by the corresponding *StateChanged event.
        call(if ctx.state.as_deref() == Some(&on) { stop } else { start }, json!({}))?;
        Ok(())
    })
}

/// A button taking one call with fixed params.
fn simple_button(id: &str, label: &str, icon: &str, request: &'static str) -> Action {
    Action::button(id, label).icon(icon).on_press(move |ctx| {
        require_obs(ctx)?;
        call(request, json!({})).map(|_| ())
    })
}

/// A button picking one named thing via a query-backed select, then applying it.
fn select_button(id: &str, label: &str, icon: &str, key: &'static str, query: &str, missing: &'static str, request: &'static str, param: &'static str) -> Action {
    Action::button(id, label).icon(icon).inputs(vec![input::query_select(key, label, query, &[])]).on_press(move |ctx| {
        require_obs(ctx)?;
        let v = require_input(ctx, key, missing)?;
        call(request, json!({ param: v })).map(|_| ())
    })
}

fn names(v: &Value, list: &str, key: &str) -> Vec<Value> {
    v[list].as_array().map(|a| a.iter().map(|i| json!({"value": i[key], "label": i[key]})).collect()).unwrap_or_default()
}

fn name_list(v: &Value, list: &str) -> Vec<Value> {
    v[list].as_array().map(|a| a.iter().map(|i| json!({"value": i, "label": i})).collect()).unwrap_or_default()
}

fn query(f: impl Fn(&knobdeck_sdk::QueryCtx) -> Result<Vec<Value>> + Send + Sync + 'static) -> impl Fn(&knobdeck_sdk::QueryCtx) -> Result<Value> + Send + Sync + 'static {
    move |ctx| {
        if !connected() {
            return Err(knobdeck_sdk::Error::msg("OBS is not connected"));
        }
        f(ctx).map(Value::Array)
    }
}

/// Scenes reversed so the top scene in OBS appears first.
fn scenes() -> Result<Vec<Value>> {
    let mut list = names(&call("GetSceneList", json!({}))?, "scenes", "sceneName");
    list.reverse();
    Ok(list)
}

fn main() {
    load_env();
    let (host, port, password) = (env_or("OBS_HOST", "localhost"), env_or("OBS_PORT", "4455"), env_or("OBS_PASSWORD", ""));

    // Kick off the first connection immediately so the cache may be warm by the first sync
    // (not under codegen, where Plugin::run() exits before anything is needed).
    if std::env::var("KNOBDECK_CODEGEN").as_deref() != Ok("1") {
        std::thread::spawn(move || obs_ws::run(host, port, password, sync_obs_state, on_event, on_closed));
    }

    Plugin::new(if cfg!(target_os = "macos") { "obs-macos" } else { "obs-windows" }, "OBS Studio")
        .version(env!("CARGO_PKG_VERSION"))
        .platforms(&[std::env::consts::OS]) // one package per OS
        .author("KnobDeck")
        .description("Control OBS Studio — record, stream, scenes, sources & more.")
        .icon("obs-logo.svg")
        .ack_timeout_ms(5000)
        .setup("obs-setup", Some("setup.html"))
        .on_ready(|rt| {
            let _ = RT.set(rt.clone());
            rt.log("OBS plugin ready");
            // Sync actual OBS states to all placed instances now that rt is available.
            sync_obs_state();
            Ok(())
        })
        // 1–2. Record, pause
        .action(global_toggle("record", "Record", "record.svg", [("stopped", "Record Off", "square"), ("recording", "Recording", "circle-dot")], "StartRecord", "StopRecord", |c| c.recording))
        .action(
            Action::toggle("record-pause", "Record Pause")
                .icon("record-pause.svg")
                .state("recording", "Recording", "play")
                .state("paused", "Paused", "pause")
                .initial_with(|_| Some(pick(CACHE.lock().unwrap().record_paused, "paused", "recording").into()))
                .on_press(|ctx| {
                    require_obs(ctx)?;
                    call(if ctx.state.as_deref() == Some("paused") { "ResumeRecord" } else { "PauseRecord" }, json!({}))?;
                    Ok(())
                }),
        )
        // 3. Chapter marker
        .action(
            Action::button("chapter-marker", "Chapter Marker")
                .icon("chapter.svg")
                .inputs(vec![input::text("chapterName", "Chapter Name").placeholder("Chapter 1")])
                .on_press(|ctx| {
                    require_obs(ctx)?;
                    call("CreateRecordChapter", json!({"chapterName": ctx.input_str("chapterName")})).map(|_| ())
                }),
        )
        // 4–6. Stream, virtual cam, replay buffer
        .action(global_toggle("stream", "Stream", "stream.svg", [("stopped", "Offline", "circle"), ("streaming", "Live", "circle-dot")], "StartStream", "StopStream", |c| c.streaming))
        .action(global_toggle("virtual-cam", "Virtual Camera", "virtual-cam.svg", [("off", "Virtual Cam Off", "circle"), ("on", "Virtual Cam On", "circle-dot")], "StartVirtualCam", "StopVirtualCam", |c| c.vcam))
        .action(global_toggle("replay-buffer", "Replay Buffer", "replay-buffer.svg", [("stopped", "Buffer Off", "circle"), ("active", "Buffer On", "circle-dot")], "StartReplayBuffer", "StopReplayBuffer", |c| c.replay_buffer))
        .action(simple_button("replay-buffer-save", "Save Replay", "replay-save.svg", "SaveReplayBuffer"))
        // 8–10. Collection, profile, scene
        .action(select_button("scene-collection", "Scene Collection", "scene-collection.svg", "collection", "get-scene-collections", "No scene collection selected", "SetCurrentSceneCollection", "sceneCollectionName"))
        .action(select_button("profile", "Profile", "profile.svg", "profile", "get-profiles", "No profile selected", "SetCurrentProfile", "profileName"))
        .action(select_button("scene", "Scene", "scene.svg", "scene", "get-scenes", "No scene selected", "SetCurrentProgramScene", "sceneName"))
        // 11. Source visibility
        .action(
            Action::toggle("source-visibility", "Source Visibility")
                .icon("source-visibility.svg")
                .inputs(vec![
                    input::query_select("scene", "Scene", "get-scenes", &[]),
                    input::query_select("source", "Source", "get-scene-items", &["scene"]).with("showIf", json!({"key": "scene", "truthy": true})),
                ])
                .state("visible", "Visible", "eye")
                .state("hidden", "Hidden", "eye-off")
                .initial_with(|ic| {
                    register(&ic.instance_id, "source-visibility", &ic.inputs);
                    Some("visible".into()) // actual state pushed by on_ready → sync_per_instance_states()
                })
                .on_press(|ctx| {
                    require_obs(ctx)?;
                    if ctx.input_str("scene").is_empty() || ctx.input_str("source").is_empty() {
                        return ctx.fail("Scene and Source must be configured");
                    }
                    register(&ctx.instance_id, "source-visibility", &ctx.inputs);
                    let id = call("GetSceneItemId", json!({"sceneName": ctx.inputs["scene"], "sourceName": ctx.inputs["source"]}))?["sceneItemId"].as_i64().unwrap_or(0);
                    SCENE_ITEM_IDS.lock().unwrap().get_or_insert_with(HashMap::new).insert(ctx.instance_id.clone(), id);
                    let enable = ctx.state.as_deref() != Some("visible");
                    call("SetSceneItemEnabled", json!({"sceneName": ctx.inputs["scene"], "sceneItemId": id, "sceneItemEnabled": enable}))?;
                    ctx.set_state(pick(enable, "visible", "hidden"));
                    Ok(())
                }),
        )
        // 12. Audio mixer
        .action(
            Action::toggle("audio-mixer", "Audio Mixer")
                .icon("audio-mixer.svg")
                .inputs(vec![input::query_select("source", "Audio Source", "get-audio-inputs", &[])])
                .state("unmuted", "Unmuted", "volume-2")
                .state("muted", "Muted", "volume-x")
                .initial_with(|ic| {
                    register(&ic.instance_id, "audio-mixer", &ic.inputs);
                    Some("unmuted".into())
                })
                .on_press(|ctx| {
                    require_obs(ctx)?;
                    let source = require_input(ctx, "source", "No audio source selected")?;
                    register(&ctx.instance_id, "audio-mixer", &ctx.inputs);
                    let muted = ctx.state.as_deref() != Some("muted");
                    call("SetInputMute", json!({"inputName": source, "inputMuted": muted}))?;
                    ctx.set_state(pick(muted, "muted", "unmuted"));
                    Ok(())
                }),
        )
        // 13. Media source control
        .action(
            Action::button("media-source-control", "Media Source Control")
                .icon("media-control.svg")
                .inputs(vec![
                    input::query_select("source", "Media Source", "get-media-sources", &[]),
                    input::select(
                        "action",
                        "Action",
                        &[
                            ("OBS_WEBSOCKET_MEDIA_INPUT_ACTION_PLAY", "Play"),
                            ("OBS_WEBSOCKET_MEDIA_INPUT_ACTION_PAUSE", "Pause"),
                            ("OBS_WEBSOCKET_MEDIA_INPUT_ACTION_STOP", "Stop"),
                            ("OBS_WEBSOCKET_MEDIA_INPUT_ACTION_RESTART", "Restart"),
                            ("OBS_WEBSOCKET_MEDIA_INPUT_ACTION_NEXT", "Next"),
                            ("OBS_WEBSOCKET_MEDIA_INPUT_ACTION_PREVIOUS", "Previous"),
                        ],
                    ),
                ])
                .on_press(|ctx| {
                    require_obs(ctx)?;
                    let source = require_input(ctx, "source", "No media source selected")?;
                    let action = require_input(ctx, "action", "No action selected")?;
                    call("TriggerMediaInputAction", json!({"inputName": source, "mediaAction": action})).map(|_| ())
                }),
        )
        // 14. Studio mode
        .action(
            Action::toggle("studio-mode", "Studio Mode")
                .icon("studio-mode.svg")
                .state("off", "Studio Off", "circle")
                .state("on", "Studio On", "circle-dot")
                .initial_with(|_| Some(pick(CACHE.lock().unwrap().studio_mode, "on", "off").into()))
                .on_press(|ctx| {
                    require_obs(ctx)?;
                    let enable = ctx.state.as_deref() != Some("on");
                    call("SetStudioModeEnabled", json!({"studioModeEnabled": enable}))?;
                    ctx.set_state(pick(enable, "on", "off"));
                    Ok(())
                }),
        )
        // 15. Transition
        .action(
            Action::button("transition", "Transition")
                .icon("transition.svg")
                .inputs(vec![
                    input::query_select("transition", "Transition", "get-transitions", &[]),
                    input::text("duration", "Duration (ms)").placeholder("300"),
                ])
                .on_press(|ctx| {
                    require_obs(ctx)?;
                    let t = require_input(ctx, "transition", "No transition selected")?;
                    call("SetCurrentSceneTransition", json!({"transitionName": t}))?;
                    if let Some(ms) = parse_int(&ctx.input_str("duration")) {
                        call("SetCurrentSceneTransitionDuration", json!({"transitionDuration": ms}))?;
                    }
                    Ok(())
                }),
        )
        // 16. Filter
        .action(
            Action::toggle("filter", "Filter")
                .icon("filter.svg")
                .inputs(vec![
                    input::query_select("source", "Source", "get-sources", &[]),
                    input::query_select("filter", "Filter", "get-filters", &["source"]).with("showIf", json!({"key": "source", "truthy": true})),
                ])
                .state("enabled", "Filter On", "check")
                .state("disabled", "Filter Off", "x")
                .initial_with(|ic| {
                    register(&ic.instance_id, "filter", &ic.inputs);
                    Some("enabled".into())
                })
                .on_press(|ctx| {
                    require_obs(ctx)?;
                    if ctx.input_str("source").is_empty() || ctx.input_str("filter").is_empty() {
                        return ctx.fail("Source and Filter must be configured");
                    }
                    register(&ctx.instance_id, "filter", &ctx.inputs);
                    let enable = ctx.state.as_deref() != Some("enabled");
                    call("SetSourceFilterEnabled", json!({"sourceName": ctx.inputs["source"], "filterName": ctx.inputs["filter"], "filterEnabled": enable}))?;
                    ctx.set_state(pick(enable, "enabled", "disabled"));
                    Ok(())
                }),
        )
        // 17. Screenshot
        .action(
            Action::button("screenshot", "Screenshot")
                .icon("screenshot.svg")
                .inputs(vec![
                    input::query_select("source", "Source", "get-sources", &[]),
                    input::select("imageFormat", "Format", &[("png", "PNG"), ("jpg", "JPEG"), ("bmp", "BMP")]),
                    input::text("imagePath", "Save Path").placeholder("C:\\screenshots\\shot.png"),
                    input::text("imageWidth", "Width (px)").placeholder("1920"),
                    input::text("imageHeight", "Height (px)").placeholder("1080"),
                ])
                .on_press(|ctx| {
                    require_obs(ctx)?;
                    let source = require_input(ctx, "source", "No source selected")?;
                    let format = match ctx.input_str("imageFormat") {
                        f if f.is_empty() => "png".to_string(),
                        f => f,
                    };
                    let mut params = json!({"sourceName": source, "imageFormat": format});
                    let path = ctx.input_str("imagePath");
                    if !path.is_empty() {
                        params["imageFilePath"] = path.into();
                    }
                    if let Some(w) = parse_int(&ctx.input_str("imageWidth")) {
                        params["imageWidth"] = w.into();
                    }
                    if let Some(h) = parse_int(&ctx.input_str("imageHeight")) {
                        params["imageHeight"] = h.into();
                    }
                    call("SaveSourceScreenshot", params).map(|_| ())
                }),
        )
        // 18. Studio-mode transition
        .action(simple_button("scene-transition", "Scene Transition", "scene-transition.svg", "TriggerStudioModeTransition"))
        // Queries
        .query("get-scenes", query(|_| scenes()))
        .query("get-scene-collections", query(|_| Ok(name_list(&call("GetSceneCollectionList", json!({}))?, "sceneCollections"))))
        .query("get-profiles", query(|_| Ok(name_list(&call("GetProfileList", json!({}))?, "profiles"))))
        .query("get-scene-items", query(|ctx| Ok(names(&call("GetSceneItemList", json!({"sceneName": ctx.inputs["scene"]}))?, "sceneItems", "sourceName"))))
        .query("get-sources", query(|_| {
            // All inputs + scenes (used by Filter and Screenshot).
            let mut out = names(&call("GetInputList", json!({}))?, "inputs", "inputName");
            let mut scenes = scenes()?;
            for s in &mut scenes {
                s["label"] = format!("[Scene] {}", s["value"].as_str().unwrap_or("")).into();
            }
            out.extend(scenes);
            Ok(out)
        }))
        .query("get-audio-inputs", query(|_| Ok(names(&call("GetInputList", json!({}))?, "inputs", "inputName"))))
        .query("get-media-sources", query(|_| {
            // Media file and VLC sources (used by Media Source Control).
            let mut out = vec![];
            for kind in ["ffmpeg_source", "vlc_source"] {
                if let Ok(r) = call("GetInputList", json!({"inputKind": kind})) {
                    out.extend(names(&r, "inputs", "inputName"));
                }
            }
            Ok(out)
        }))
        .query("get-transitions", query(|_| Ok(names(&call("GetSceneTransitionList", json!({}))?, "transitions", "transitionName"))))
        .query("get-filters", query(|ctx| Ok(names(&call("GetSourceFilterList", json!({"sourceName": ctx.inputs["source"]}))?, "filters", "filterName"))))
        .run()
}
