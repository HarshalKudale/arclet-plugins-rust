//! Streamlabs Desktop plugin for KnobDeck — go live, record, replay buffer, scenes, sources and
//! audio through the app's local API (Settings ▸ Remote Control).

mod api;

use knobdeck_sdk::input::{self, InputExt};
use knobdeck_sdk::{env_or, json, load_env, Action, Ctx, Error, Plugin, Result, Value, Visual};
use std::time::Duration;

const POLL: Duration = Duration::from_secs(2);

fn cfg() -> api::Config {
    api::Config { host: env_or("STREAMLABS_HOST", "127.0.0.1"), port: env_or("STREAMLABS_PORT", "59650"), token: env_or("STREAMLABS_TOKEN", "") }
}

fn call(resource: &str, method: &str, args: Value) -> Result<Value> {
    api::call(&cfg(), resource, method, args)
}

fn streaming_model() -> Result<Value> {
    call("StreamingService", "getModel", json!([]))
}

/// A toggle over one StreamingService status: `on` when the status is one of `active`.
fn output_toggle(id: &str, label: &str, icon: &str, field: &'static str, active: &'static [&'static str], toggle: &'static str, states: [(&str, &str, &str); 2]) -> Action {
    let on_of = move |m: &Value| active.contains(&m[field].as_str().unwrap_or(""));
    let (off, on) = (states[0].0.to_string(), states[1].0.to_string());
    let mut a = Action::toggle(id, label).icon(icon);
    for (v, l, i) in states {
        a = a.state(v, l, i);
    }
    let (off2, on2) = (off.clone(), on.clone());
    a.live(POLL, move |_| Ok(Visual::new().state(if on_of(&streaming_model()?) { &on } else { &off })))
        .on_press(move |ctx| {
            let (method, args) = match toggle {
                // The replay buffer has start/stop calls rather than a toggle.
                "replay" => (if on_of(&streaming_model()?) { "stopReplayBuffer" } else { "startReplayBuffer" }, json!([])),
                t => (t, json!([])),
            };
            call("StreamingService", method, args)?;
            // Streamlabs reports "starting"/"stopping" first; the poll shows the final state.
            ctx.set_state(if ctx.state.as_deref() == Some(on2.as_str()) { &off2 } else { &on2 });
            Ok(())
        })
}

fn require(ctx: &Ctx, key: &str, what: &str) -> Result<String> {
    match ctx.input_str(key) {
        v if v.is_empty() => ctx.fail(format!("Choose {what} first")),
        v => Ok(v),
    }
}

/// Audio sources: `[{sourceId, name, muted, fader: {deflection}}]`.
fn audio_sources() -> Result<Vec<Value>> {
    Ok(call("AudioService", "getSources", json!([]))?.as_array().cloned().unwrap_or_default())
}

fn audio_source(id: &str) -> Result<Value> {
    audio_sources()?.into_iter().find(|s| s["sourceId"] == id).ok_or_else(|| Error::msg("That audio source no longer exists"))
}

/// Items of a scene: `[{sceneItemId, name, visible, resourceId…}]`.
fn scene_items(scene: &str) -> Result<Vec<Value>> {
    Ok(call(&format!("Scene[\"{scene}\"]"), "getItems", json!([]))?.as_array().cloned().unwrap_or_default())
}

fn scene_item(scene: &str, item: &str) -> Result<Value> {
    scene_items(scene)?.into_iter().find(|i| i["sceneItemId"] == item).ok_or_else(|| Error::msg("That source is no longer in the scene"))
}

fn options(list: Vec<Value>, value: &str, label: &str) -> Value {
    Value::Array(list.iter().map(|v| json!({"value": v[value], "label": v[label]})).collect())
}

fn main() {
    load_env();
    Plugin::new(if cfg!(target_os = "macos") { "streamlabs-macos" } else { "streamlabs-windows" }, "Streamlabs Desktop")
        .version(env!("CARGO_PKG_VERSION"))
        .platforms(&[std::env::consts::OS]) // one package per OS
        .author("KnobDeck")
        .description("Go live, record, replay buffer, scenes, sources and audio in Streamlabs Desktop.")
        .icon("radio-tower")
        .ack_timeout_ms(5000)
        .setup("streamlabs-setup", None)
        .setup_inputs(vec![
            input::text("token", "API token").placeholder("from Settings ▸ Remote Control ▸ Show details").with("required", true),
            input::text("port", "Port").placeholder("59650").default_value("59650"),
        ])
        .query("get-scenes", |_| Ok(options(call("ScenesService", "getScenes", json!([]))?.as_array().cloned().unwrap_or_default(), "id", "name")))
        .query("get-scene-items", |ctx| Ok(options(scene_items(&ctx.input_str("scene"))?, "sceneItemId", "name")))
        .query("get-audio-sources", |_| Ok(options(audio_sources()?, "sourceId", "name")))
        .action(output_toggle("stream", "Go Live", "radio-tower", "streamingStatus", &["live", "starting", "reconnecting"], "toggleStreaming", [("offline", "Offline", "circle"), ("live", "Live", "circle-dot")]))
        .action(output_toggle("record", "Record", "circle", "recordingStatus", &["recording", "starting"], "toggleRecording", [("offline", "Not Recording", "square"), ("recording", "Recording", "circle-dot")]))
        .action(output_toggle("replay-buffer", "Replay Buffer", "history", "replayBufferStatus", &["running", "saving"], "replay", [("offline", "Buffer Off", "circle"), ("running", "Buffer On", "circle-dot")]))
        .action(Action::button("save-replay", "Save Replay").icon("save").on_press(|_| call("StreamingService", "saveReplay", json!([])).map(|_| ())))
        .action(
            Action::toggle("scene", "Scene")
                .icon("layout-grid")
                .inputs(vec![input::query_select("scene", "Scene", "get-scenes", &[])])
                .state("inactive", "Not Live", "layout-grid")
                .state("active", "Live", "circle-check")
                .live(POLL, |ic| Ok(Visual::new().state(if call("ScenesService", "activeSceneId", json!([]))? == ic.input_str("scene").as_str() { "active" } else { "inactive" })))
                .on_press(|ctx| {
                    let scene = require(ctx, "scene", "a scene")?;
                    call("ScenesService", "makeSceneActive", json!([scene]))?;
                    ctx.set_state("active");
                    Ok(())
                }),
        )
        .action(
            Action::toggle("source-visibility", "Source Visibility")
                .icon("eye")
                .inputs(vec![input::query_select("scene", "Scene", "get-scenes", &[]), input::query_select("item", "Source", "get-scene-items", &["scene"]).with("showIf", json!({"key": "scene", "truthy": true}))])
                .state("visible", "Visible", "eye")
                .state("hidden", "Hidden", "eye-off")
                .live(POLL, |ic| {
                    let (scene, item) = (ic.input_str("scene"), ic.input_str("item"));
                    if scene.is_empty() || item.is_empty() {
                        return Ok(Visual::new());
                    }
                    Ok(Visual::new().state(if scene_item(&scene, &item)?["visible"] == true { "visible" } else { "hidden" }))
                })
                .on_press(|ctx| {
                    let (scene, item) = (require(ctx, "scene", "a scene")?, require(ctx, "item", "a source")?);
                    let it = scene_item(&scene, &item)?;
                    let show = it["visible"] != true;
                    let resource = it["resourceId"].as_str().map(String::from).unwrap_or_else(|| format!("SceneItem[\"{scene}\",\"{item}\",\"{}\"]", it["sourceId"].as_str().unwrap_or("")));
                    call(&resource, "setVisibility", json!([show]))?;
                    ctx.set_state(if show { "visible" } else { "hidden" });
                    Ok(())
                }),
        )
        .action(
            Action::toggle("audio-mute", "Mute Audio Source")
                .icon("volume-x")
                .inputs(vec![input::query_select("source", "Audio source", "get-audio-sources", &[])])
                .state("unmuted", "On", "volume-2")
                .state("muted", "Muted", "volume-x")
                .live(POLL, |ic| {
                    let id = ic.input_str("source");
                    if id.is_empty() {
                        return Ok(Visual::new());
                    }
                    Ok(Visual::new().state(if audio_source(&id)?["muted"] == true { "muted" } else { "unmuted" }))
                })
                .on_press(|ctx| {
                    let id = require(ctx, "source", "an audio source")?;
                    let mute = audio_source(&id)?["muted"] != true;
                    call("SourcesService", "setMuted", json!([id, mute]))?;
                    ctx.set_state(if mute { "muted" } else { "unmuted" });
                    Ok(())
                }),
        )
        .action(
            Action::slider("audio-volume", "Audio Source Volume", 0.0, 100.0)
                .icon("volume-2")
                .unit("%")
                .inputs(vec![input::query_select("source", "Audio source", "get-audio-sources", &[]), input::text("step", "Step (%)").placeholder("5").default_value("5")])
                // Streamlabs' fader "deflection" (0–1) is the mixer slider position.
                .live(POLL, |ic| {
                    let id = ic.input_str("source");
                    if id.is_empty() {
                        return Ok(Visual::new());
                    }
                    Ok(Visual::new().state((audio_source(&id)?["fader"]["deflection"].as_f64().unwrap_or(0.0) * 100.0).round()))
                })
                .on_adjust(|ctx| {
                    let id = require(ctx, "source", "an audio source")?;
                    let target = match ctx.value {
                        Some(v) => v,
                        None => audio_source(&id)?["fader"]["deflection"].as_f64().unwrap_or(0.0) * 100.0 + ctx.input_f64("step").unwrap_or(5.0) * ctx.delta.unwrap_or(0.0),
                    }
                    .round()
                    .clamp(0.0, 100.0);
                    call(&format!("AudioSource[\"{id}\"]"), "setDeflection", json!([target / 100.0]))?;
                    ctx.set_state(target);
                    Ok(())
                }),
        )
        .run()
}
