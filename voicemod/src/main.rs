//! Voicemod plugin for KnobDeck — voices, voice changer, hear-myself, background effects, mic mute
//! and the soundboard through Voicemod's local Control API.

mod api;

use knobdeck_sdk::input::{self, InputExt};
use knobdeck_sdk::{env_or, json, load_env, Action, Plugin, Result, Value, Visual};
use std::time::Duration;

const POLL: Duration = Duration::from_secs(2);

/// Voicemod issues client keys per app: KnobDeck's is baked in at build time (CI variable
/// VOICEMOD_CLIENT_KEY); setup can override it with the user's own.
fn key() -> String {
    env_or("VOICEMOD_CLIENT_KEY", option_env!("VOICEMOD_CLIENT_KEY").unwrap_or(""))
}

fn ask(action: &str, payload: Value) -> Result<Value> {
    api::ask(&key(), action, payload)
}

fn tell(action: &str, payload: Value) -> Result<()> {
    api::tell(&key(), action, payload)
}

/// An on/off setting: `get…Status` answers `{value: bool}`, `toggle…` flips it and answers the same.
fn setting(id: &str, label: &str, icon: &str, status: &'static str, toggle: &'static str, states: [(&str, &str, &str); 2]) -> Action {
    let (off, on) = (states[0].0.to_string(), states[1].0.to_string());
    let (off2, on2) = (off.clone(), on.clone());
    let mut a = Action::toggle(id, label).icon(icon);
    for (v, l, i) in states {
        a = a.state(v, l, i);
    }
    a.live(POLL, move |_| Ok(Visual::new().state(if ask(status, json!({}))?["value"] == true { &on } else { &off })))
        .on_press(move |ctx| {
            let now_on = ask(toggle, json!({}))?["value"] == true;
            ctx.set_state(if now_on { &on2 } else { &off2 });
            Ok(())
        })
}

/// `[{id, friendlyName, favorited…}]`
fn voices() -> Result<Vec<Value>> {
    Ok(ask("getVoices", json!({}))?["voices"].as_array().cloned().unwrap_or_default())
}

fn current_voice() -> Result<String> {
    Ok(ask("getCurrentVoice", json!({}))?["voiceID"].as_str().unwrap_or("").to_string())
}

/// Soundboard sounds, `[{FileName, Name}]` (older Voicemod builds use camelCase names).
fn sounds() -> Result<Vec<(String, String)>> {
    let memes = ask("getMemes", json!({}))?;
    let list = memes["listOfMemes"].as_array().cloned().unwrap_or_default();
    Ok(list
        .iter()
        .filter_map(|m| {
            let file = m["FileName"].as_str().or(m["fileName"].as_str())?.to_string();
            let name = m["Name"].as_str().or(m["name"].as_str()).unwrap_or(&file).to_string();
            Some((file, name))
        })
        .collect())
}

fn main() {
    load_env();
    Plugin::new(if cfg!(target_os = "macos") { "voicemod-macos" } else { "voicemod-windows" }, "Voicemod")
        .version(env!("CARGO_PKG_VERSION"))
        .platforms(&[std::env::consts::OS]) // one package per OS
        .author("KnobDeck")
        .description("Voicemod voices, voice changer, hear myself, background effects, mic mute and soundboard.")
        .icon("audio-waveform")
        .ack_timeout_ms(5000)
        .setup("voicemod-setup", None)
        .setup_inputs(vec![input::text("key", "Client key (optional)").placeholder("leave empty to use KnobDeck's").with("help", "Only needed if Voicemod gave you your own Control API key.")])
        .query("get-voices", |_| Ok(Value::Array(voices()?.iter().map(|v| json!({"value": v["id"], "label": v["friendlyName"]})).collect())))
        .query("get-sounds", |_| Ok(Value::Array(sounds()?.into_iter().map(|(f, n)| json!({"value": f, "label": n})).collect())))
        .action(setting("voice-changer", "Voice Changer", "audio-waveform", "getVoiceChangerStatus", "toggleVoiceChanger", [("off", "Voice Changer Off", "audio-lines"), ("on", "Voice Changer On", "audio-waveform")]))
        .action(setting("hear-myself", "Hear Myself", "ear", "getHearMyselfStatus", "toggleHearMyVoice", [("off", "Not Hearing", "ear-off"), ("on", "Hearing Myself", "ear")]))
        .action(setting("background-effects", "Background Effects", "sparkles", "getBackgroundEffectStatus", "toggleBackground", [("off", "Effects Off", "circle"), ("on", "Effects On", "sparkles")]))
        .action(setting("mute-mic", "Mute Microphone", "mic", "getMuteMicStatus", "toggleMuteMic", [("unmuted", "Mic On", "mic"), ("muted", "Mic Muted", "mic-off")]))
        .action(setting("mute-sounds-for-me", "Mute Sounds for Me", "volume-x", "getMuteMemeForMeStatus", "toggleMuteMemeForMe", [("off", "I Hear Sounds", "volume-2"), ("on", "Sounds Muted for Me", "volume-x")]))
        .action(
            Action::toggle("voice", "Select Voice")
                .icon("user-round")
                .inputs(vec![input::query_combobox("voice", "Voice", "get-voices")])
                .state("inactive", "Not Selected", "user-round")
                .state("active", "Selected", "circle-check")
                .live(POLL, |ic| Ok(Visual::new().state(if current_voice()? == ic.input_str("voice") { "active" } else { "inactive" })))
                .on_press(|ctx| {
                    let voice = ctx.input_str("voice");
                    if voice.is_empty() {
                        return ctx.fail("Choose a voice first");
                    }
                    tell("loadVoice", json!({"voiceID": voice}))?;
                    ctx.set_state("active");
                    Ok(())
                }),
        )
        .action(
            Action::button("random-voice", "Random Voice")
                .icon("shuffle")
                .inputs(vec![input::select("mode", "From", &[("AllVoices", "All voices"), ("FreeVoices", "Free voices"), ("FavoriteVoices", "Favorite voices"), ("CustomVoices", "Custom voices")]).with("default", "AllVoices")])
                .on_press(|ctx| tell("selectRandomVoice", json!({"mode": ctx.input_str("mode")}))),
        )
        .action(
            Action::wheel("favorite-voices-wheel", "Favorite Voices")
                .icon("star")
                .on_open(|ctx| {
                    let list = voices().or_else(|e| ctx.fail(e.message().to_string()))?;
                    let current = current_voice().unwrap_or_default();
                    let favs: Vec<Value> = list
                        .iter()
                        .filter(|v| v["favorited"] == true)
                        .take(16)
                        .map(|v| json!({"id": v["id"], "label": v["friendlyName"], "icon": if v["id"] == current.as_str() { "circle-check" } else { "user-round" }}))
                        .collect();
                    if favs.is_empty() {
                        return ctx.fail("No favorite voices — star some in Voicemod");
                    }
                    Ok(favs)
                })
                // The slice id is the voice id.
                .on_child_press(|ctx| tell("loadVoice", json!({"voiceID": ctx.child_id.clone().unwrap_or_default()}))),
        )
        .action(Action::button("play-sound", "Play Sound").icon("play").inputs(vec![input::query_combobox("sound", "Sound", "get-sounds")]).on_press(|ctx| {
            let file = ctx.input_str("sound");
            if file.is_empty() {
                return ctx.fail("Choose a sound first");
            }
            tell("playMeme", json!({"FileName": file, "IsKeyDown": true}))
        }))
        .action(Action::button("stop-sounds", "Stop All Sounds").icon("square").on_press(|_| tell("stopAllMemeSounds", json!({}))))
        // Hold to bleep (censor) yourself.
        .action(
            Action::hold("bleep", "Bleep")
                .icon("message-square-off")
                .released("Bleep", "message-square-off")
                .held("Bleeping", "megaphone")
                .on_hold_start(|_| tell("setBeepSound", json!({"badLanguage": 1})))
                .on_hold_end(|_| tell("setBeepSound", json!({"badLanguage": 0}))),
        )
        .run()
}
