//! Spotify plugin for KnobDeck. Play/pause, next, previous and now playing go through the OS
//! (no account or Premium needed). The Web API actions (likes, shuffle, repeat, volume, playlists)
//! are written but commented out until KnobDeck's Spotify app is set up; see web.rs.

mod os;
// mod web;

// The Web API code below also needs: knobdeck_sdk::input::{self, InputExt}, json, Result, Value,
// std::collections::HashMap and std::sync::Mutex.
use knobdeck_sdk::{load_env, Action, Plugin, Visual};
use std::time::Duration;

const POLL: Duration = Duration::from_secs(2);

// /// Repeat modes in the order a press cycles through them.
// const REPEAT: [&str; 3] = ["off", "context", "track"];

fn playing_label(n: &os::NowPlaying) -> String {
    match (n.title.is_empty(), n.artist.is_empty()) {
        (true, _) => "Spotify".into(),
        (false, true) => n.title.clone(),
        (false, false) => format!("{} — {}", n.title, n.artist),
    }
}

// /// Whether the current track is in Liked Songs.
// fn liked() -> Result<bool> {
//     let (id, _) = web::current_track()?;
//     Ok(web::get(&format!("/me/tracks/contains?ids={id}"))?[0] == true)
// }
//
// static WHEEL: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
//
// fn playlist_options() -> Result<Value> {
//     Ok(Value::Array(web::playlists()?.into_iter().map(|(uri, name)| json!({"value": uri, "label": name})).collect()))
// }
//
// fn play_context(uri: &str) -> Result<()> {
//     web::call("PUT", "/me/player/play", Some(json!({"context_uri": uri}))).map(|_| ())
// }

fn main() {
    load_env();
    Plugin::new(&format!("spotify-{}", std::env::consts::OS), "Spotify")
        .version(env!("CARGO_PKG_VERSION"))
        .platforms(&[std::env::consts::OS]) // one package per OS
        .author("KnobDeck")
        .description("Play, pause, skip and now playing for the Spotify desktop app.")
        .icon("music")
        .ack_timeout_ms(5000)
        // .setup("spotify-setup", None)
        // .setup_inputs(vec![input::text("client_id", "Spotify app client ID (optional)").placeholder("leave empty to use KnobDeck's").with("help", "Only needed if you made your own app at developer.spotify.com (redirect URI http://127.0.0.1:47811/callback).")])
        // .query("get-playlists", |_| playlist_options())
        // ── Through the OS: no account or Premium ──
        .action(
            Action::toggle("play-pause", "Play / Pause")
                .icon("play")
                .state("paused", "Paused", "play")
                .state("playing", "Playing", "pause")
                .live(POLL, |_| Ok(match os::now_playing() {
                    Ok(n) => Visual::new().state(if n.playing { "playing" } else { "paused" }),
                    Err(_) => Visual::new().state("paused"),
                }))
                .on_press(|ctx| {
                    os::play_pause()?;
                    ctx.set_state(if ctx.state.as_deref() == Some("playing") { "paused" } else { "playing" });
                    Ok(())
                }),
        )
        .action(Action::button("next", "Next Track").icon("skip-forward").on_press(|_| os::next()))
        .action(Action::button("previous", "Previous Track").icon("skip-back").on_press(|_| os::previous()))
        .action(
            Action::button("now-playing", "Now Playing")
                .icon("music")
                .live(POLL, |_| Ok(match os::now_playing() {
                    Ok(n) => Visual::new().label(playing_label(&n)).icon(if n.playing { "music" } else { "pause" }),
                    Err(e) => Visual::new().label(e.message().to_string()).icon("music"),
                }))
                .on_press(|_| os::play_pause()),
        )
        // ── Web API (sign-in) — off until KnobDeck's Spotify app has Premium and SPOTIFY_CLIENT_ID is set.
        // Restore these actions, the setup and get-playlists query above, `mod web` and the helpers. ──
        // .action(
            // Action::toggle("like", "Like Song")
                // .icon("heart")
                // .state("not-liked", "Not Liked", "heart-plus")
                // .state("liked", "Liked", "heart")
                // .live(Duration::from_secs(5), |_| Ok(Visual::new().state(if liked().unwrap_or(false) { "liked" } else { "not-liked" })))
                // .on_press(|ctx| {
                    // let (id, _) = web::current_track()?;
                    // let was = web::get(&format!("/me/tracks/contains?ids={id}"))?[0] == true;
                    // web::call(if was { "DELETE" } else { "PUT" }, &format!("/me/tracks?ids={id}"), None)?;
                    // ctx.set_state(if was { "not-liked" } else { "liked" });
                    // Ok(())
                // }),
        // )
        // .action(
            // Action::toggle("shuffle", "Shuffle")
                // .icon("shuffle")
                // .state("off", "Shuffle Off", "arrow-right")
                // .state("on", "Shuffle On", "shuffle")
                // .live(Duration::from_secs(5), |_| Ok(Visual::new().state(if web::player()?["shuffle_state"] == true { "on" } else { "off" })))
                // .on_press(|ctx| {
                    // let on = web::player()?["shuffle_state"] != true;
                    // web::call("PUT", &format!("/me/player/shuffle?state={on}"), None)?;
                    // ctx.set_state(if on { "on" } else { "off" });
                    // Ok(())
                // }),
        // )
        // .action(
            // Action::toggle("repeat", "Repeat")
                // .icon("repeat")
                // .state("off", "Repeat Off", "arrow-right")
                // .state("context", "Repeat All", "repeat")
                // .state("track", "Repeat One", "repeat-1")
                // .live(Duration::from_secs(5), |_| Ok(Visual::new().state(web::player()?["repeat_state"].as_str().unwrap_or("off"))))
                // .on_press(|ctx| {
                    // let now = web::player()?["repeat_state"].as_str().unwrap_or("off").to_string();
                    // let next = REPEAT[(REPEAT.iter().position(|m| *m == now).unwrap_or(0) + 1) % REPEAT.len()];
                    // web::call("PUT", &format!("/me/player/repeat?state={next}"), None)?;
                    // ctx.set_state(next);
                    // Ok(())
                // }),
        // )
        // .action(
            // Action::slider("volume", "Spotify Volume", 0.0, 100.0)
                // .icon("volume-2")
                // .unit("%")
                // .inputs(vec![input::text("step", "Step (%)").placeholder("5").default_value("5")])
                // .live(Duration::from_secs(5), |_| Ok(match web::player()?["device"]["volume_percent"].as_i64() {
                    // Some(v) => Visual::new().state(v),
                    // None => Visual::new(),
                // }))
                // .on_adjust(|ctx| {
                    // let target = match ctx.value {
                        // Some(v) => v,
                        // None => web::player()?["device"]["volume_percent"].as_f64().unwrap_or(50.0) + ctx.input_f64("step").unwrap_or(5.0) * ctx.delta.unwrap_or(0.0),
                    // }
                    // .round()
                    // .clamp(0.0, 100.0);
                    // web::call("PUT", &format!("/me/player/volume?volume_percent={target}"), None)?;
                    // ctx.set_state(target);
                    // Ok(())
                // }),
        // )
        // .action(Action::button("play-playlist", "Play Playlist").icon("list-music").inputs(vec![input::query_combobox("playlist", "Playlist", "get-playlists")]).on_press(|ctx| {
            // let uri = ctx.input_str("playlist");
            // if uri.is_empty() {
                // return ctx.fail("Choose a playlist first");
            // }
            // play_context(&uri)
        // }))
        // .action(Action::button("add-to-playlist", "Add Song to Playlist").icon("list-plus").inputs(vec![input::query_combobox("playlist", "Playlist", "get-playlists")]).on_press(|ctx| {
            // let uri = ctx.input_str("playlist");
            // let Some(id) = uri.strip_prefix("spotify:playlist:") else {
                // return ctx.fail("Choose one of your playlists (Liked Songs has its own Like key)");
            // };
            // let (_, track) = web::current_track()?;
            // web::call("POST", &format!("/playlists/{id}/tracks"), Some(json!({"uris": [track]}))).map(|_| ())
        // }))
        // .action(
            // Action::wheel("playlists-wheel", "Playlists")
                // .icon("list-music")
                // .on_open(|ctx| {
                    // let list = web::playlists().or_else(|e| ctx.fail(e.message().to_string()))?;
                    // let mut map = HashMap::new();
                    // let slices = list
                        // .into_iter()
                        // .take(16)
                        // .enumerate()
                        // .map(|(i, (uri, name))| {
                            // let id = format!("p{i}");
                            // map.insert(id.clone(), uri);
                            // json!({"id": id, "label": name, "icon": if i == 0 { "heart" } else { "list-music" }})
                        // })
                        // .collect();
                    // *WHEEL.lock().unwrap() = Some(map);
                    // Ok(slices)
                // })
                // .on_child_press(|ctx| {
                    // let uri = WHEEL.lock().unwrap().as_ref().and_then(|m| m.get(ctx.child_id.as_deref().unwrap_or("")).cloned());
                    // match uri {
                        // Some(u) => play_context(&u),
                        // None => ctx.fail("That playlist is no longer available"),
                    // }
                // }),
        // )
        .run()
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn media_session_manager_answers() {
        // Read-only: asks Windows for its media sessions; Spotify itself may be closed.
        eprintln!("media sessions: {}", super::os::session_count());
        assert!(matches!(super::os::now_playing(), Ok(_) | Err(_)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mpris_answers() {
        // Read-only: lists MPRIS players; Spotify itself may be closed.
        eprintln!("mpris players: {}", super::os::player_count());
        assert!(matches!(super::os::now_playing(), Ok(_) | Err(_)));
        if let Some((name, n)) = super::os::any_player_now_playing() {
            let n = n.unwrap();
            eprintln!("{name}: {:?} — {:?} playing={}", n.title, n.artist, n.playing);
        }
    }

    #[test]
    fn labels() {
        let n = |t: &str, a: &str| super::os::NowPlaying { title: t.into(), artist: a.into(), playing: true };
        assert_eq!(super::playing_label(&n("Song", "Band")), "Song — Band");
        assert_eq!(super::playing_label(&n("Song", "")), "Song");
        assert_eq!(super::playing_label(&n("", "")), "Spotify");
    }
}
