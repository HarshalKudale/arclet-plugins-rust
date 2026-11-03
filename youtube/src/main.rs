//! YouTube plugin for KnobDeck — live viewers and the Studio dashboard, through the YouTube Data
//! API with a read-only Google sign-in.

mod yt;

use knobdeck_sdk::input::{self, InputExt};
use knobdeck_sdk::{load_env, open_url, Action, Plugin, Result, Visual};
use std::time::Duration;

/// Each API read costs quota; a minute keeps a day of viewer polling far inside the free quota.
const VIEWERS_POLL: Duration = Duration::from_secs(60);

fn viewers_label() -> Result<String> {
    let Some(b) = yt::active_broadcast()? else { return Ok("Offline".into()) };
    let id = b["id"].as_str().unwrap_or("");
    let v = yt::get(&format!("/videos?part=liveStreamingDetails&id={id}"))?;
    // concurrentViewers is a decimal string.
    Ok(match v["items"][0]["liveStreamingDetails"]["concurrentViewers"].as_str() {
        Some(n) => format!("{n} watching"),
        None => "Live".into(),
    })
}

fn main() {
    load_env();
    Plugin::new(if cfg!(target_os = "macos") { "youtube-macos" } else { "youtube-windows" }, "YouTube")
        .version(env!("CARGO_PKG_VERSION"))
        .platforms(&[std::env::consts::OS]) // one package per OS
        .author("KnobDeck")
        .description("YouTube live viewers and the Studio live dashboard (read-only sign-in).")
        .icon("youtube")
        .ack_timeout_ms(5000)
        .setup("youtube-setup", None)
        .setup_inputs(vec![
            input::text("client_id", "Your Google app's client ID (optional)").placeholder("leave empty to use KnobDeck's"),
            input::text("client_secret", "Your Google app's client secret (optional)").with("help", "Only for your own Google Cloud app (type Desktop, YouTube Data API enabled). Kept only on this computer."),
        ])
        .action(
            Action::button("viewers", "Viewers")
                .icon("eye")
                .live(VIEWERS_POLL, |_| Ok(Visual::new().label(viewers_label().unwrap_or_else(|e| e.message().to_string()))))
                .on_press(|ctx| {
                    ctx.set_label(viewers_label()?);
                    Ok(())
                }),
        )
        // Write actions (need youtube.force-ssl, not requested for now for privacy; going live is
        // covered by the OBS plugin). Restore with the helpers in yt.rs and the scope in setup.
        //.action(Action::button("chat-message", "Chat Message").icon("message-square").inputs(vec![input::textarea("message", "Message").placeholder("Links: …")]).on_press(|ctx| {
        //    let message = ctx.input_str("message");
        //    if message.trim().is_empty() {
        //        return ctx.fail("Write the message first");
        //    }
        //    yt::chat(message.trim())
        //}))
        //.action(
        //    Action::button("play-ad", "Play Ad")
        //        .icon("megaphone")
        //        .inputs(vec![input::select("seconds", "Length", &[("30", "30 s"), ("60", "1 min"), ("90", "1 min 30 s"), ("120", "2 min")]).with("default", "30")])
        //        .on_press(|ctx| {
        //            let b = yt::live()?;
        //            let secs: i64 = ctx.input_str("seconds").parse().unwrap_or(30);
        //            yt::call("POST", &format!("/liveBroadcasts/cuepoint?id={}", b["id"].as_str().unwrap_or("")), Some(json!({"cueType": "cueTypeAd", "durationSecs": secs})))?;
        //            ctx.set_label(format!("Ad {secs}s"));
        //            Ok(())
        //        }),
        //)
        .action(Action::button("dashboard", "Live Dashboard").icon("layout-dashboard").on_press(|_| {
            let ch = yt::get("/channels?part=id&mine=true")?;
            let id = ch["items"][0]["id"].as_str().unwrap_or("");
            open_url(&format!("https://studio.youtube.com/channel/{id}/livestreaming"))
        }))
        //.action(
        //    Action::toggle("stream", "Go Live / End Stream")
        //        .icon("radio-tower")
        //        .state("offline", "Offline", "circle")
        //        .state("live", "Live", "circle-dot")
        //        .live(VIEWERS_POLL, |_| Ok(Visual::new().state(if yt::active_broadcast()?.is_some() { "live" } else { "offline" })))
        //        // Goes live with the next scheduled broadcast; ends the live one. YouTube needs the
        //        // encoder (OBS…) to be sending video before a broadcast can go live.
        //        .on_press(|ctx| {
        //            let (b, to) = match yt::active_broadcast()? {
        //                Some(b) => (b, "complete"),
        //                None => (yt::upcoming_broadcast()?.ok_or_else(|| knobdeck_sdk::Error::msg("No scheduled stream to go live with — create one in YouTube Studio"))?, "live"),
        //            };
        //            yt::call("POST", &format!("/liveBroadcasts/transition?broadcastStatus={to}&id={}&part=status", b["id"].as_str().unwrap_or("")), None)?;
        //            ctx.set_state(if to == "live" { "live" } else { "offline" });
        //            Ok(())
        //        }),
        //)
        .run()
}
