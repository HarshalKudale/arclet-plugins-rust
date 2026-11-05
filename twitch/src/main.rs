//! Twitch plugin for KnobDeck — chat, chat modes, ads, stream info, markers, clips, viewers and
//! a channel monitor through the Helix API with the user's sign-in.

mod helix;

use knobdeck_sdk::input::{self, InputExt};
use knobdeck_sdk::{json, load_env, open_url, Action, Plugin, Result, Value, Visual};
use std::time::Duration;

const MODES_POLL: Duration = Duration::from_secs(10);

/// `?broadcaster_id=…&moderator_id=…` for the user's own channel.
fn own() -> Result<String> {
    let me = helix::me()?;
    Ok(format!("broadcaster_id={me}&moderator_id={me}"))
}

fn chat_settings() -> Result<Value> {
    Ok(helix::get(&format!("/chat/settings?{}", own()?))?["data"][0].clone())
}

/// A chat-mode toggle over one boolean field of the chat settings.
fn chat_mode(id: &str, label: &str, icon: &str, field: &'static str) -> Action {
    let mut a = Action::toggle(id, label).icon(icon).state("off", "Off", "message-circle").state("on", "On", icon);
    if field == "slow_mode" {
        a = a.inputs(vec![input::text("seconds", "Seconds between messages").placeholder("30").default_value("30")]);
    }
    a.live(MODES_POLL, move |_| Ok(Visual::new().state(if chat_settings()?[field] == true { "on" } else { "off" })))
        .on_press(move |ctx| {
            let on = chat_settings()?[field] != true;
            let mut body = json!({ field: on });
            if field == "slow_mode" && on {
                body["slow_mode_wait_time"] = json!(ctx.input_f64("seconds").unwrap_or(30.0).clamp(3.0, 120.0) as i64);
            }
            helix::call("PATCH", &format!("/chat/settings?{}", own()?), Some(body))?;
            ctx.set_state(if on { "on" } else { "off" });
            Ok(())
        })
}

fn viewers_label() -> Result<String> {
    let s = helix::get(&format!("/streams?user_id={}", helix::me()?))?;
    Ok(match s["data"][0]["viewer_count"].as_i64() {
        Some(n) => format!("{n} watching"),
        None => "Offline".into(),
    })
}

fn main() {
    load_env();
    Plugin::new(&format!("twitch-{}", std::env::consts::OS), "Twitch")
        .version(env!("CARGO_PKG_VERSION"))
        .platforms(&[std::env::consts::OS]) // one package per OS
        .author("KnobDeck")
        .description("Chat, chat modes, ads, stream info, markers, clips, viewers and live channels on Twitch.")
        .icon("twitch")
        .ack_timeout_ms(5000)
        .setup("twitch-setup", None)
        .setup_inputs(vec![input::text("client_id", "Twitch app client ID (optional)").placeholder("leave empty to use KnobDeck's").with("help", "Only needed if you registered your own app at dev.twitch.tv (Public client type).")])
        .action(Action::button("chat-message", "Chat Message").icon("message-square").inputs(vec![input::textarea("message", "Message").placeholder("Follow me on …")]).on_press(|ctx| {
            let message = ctx.input_str("message");
            if message.trim().is_empty() {
                return ctx.fail("Write the message first");
            }
            let me = helix::me()?;
            let r = helix::call("POST", "/chat/messages", Some(json!({"broadcaster_id": me, "sender_id": me, "message": message})))?;
            if r["data"][0]["is_sent"] == false {
                return ctx.fail(format!("Twitch dropped the message: {}", r["data"][0]["drop_reason"]["message"].as_str().unwrap_or("unknown reason")));
            }
            Ok(())
        }))
        .action(Action::button("clear-chat", "Clear Chat").icon("eraser").on_press(|_| helix::call("DELETE", &format!("/moderation/chat?{}", own()?), None).map(|_| ())))
        .action(chat_mode("emote-only", "Emote-Only Chat", "smile", "emote_mode"))
        .action(chat_mode("follower-only", "Follower-Only Chat", "user-check", "follower_mode"))
        .action(chat_mode("sub-only", "Subscriber-Only Chat", "star", "subscriber_mode"))
        .action(chat_mode("slow-mode", "Slow Mode", "hourglass", "slow_mode"))
        .action(
            Action::button("ad-break", "Run Ad")
                .icon("megaphone")
                .inputs(vec![input::select("length", "Length", &[("30", "30 s"), ("60", "1 min"), ("90", "1 min 30 s"), ("120", "2 min"), ("150", "2 min 30 s"), ("180", "3 min")]).with("default", "60")])
                .on_press(|ctx| {
                    let length: i64 = ctx.input_str("length").parse().unwrap_or(60);
                    let r = helix::call("POST", "/channels/commercial", Some(json!({"broadcaster_id": helix::me()?, "length": length})))?;
                    ctx.set_label(format!("Ad {}s", r["data"][0]["length"].as_i64().unwrap_or(length)));
                    Ok(())
                }),
        )
        .action(
            Action::button("stream-info", "Stream Info")
                .icon("pencil")
                .inputs(vec![input::text("title", "Title (empty keeps it)"), input::text("category", "Category (empty keeps it)").placeholder("Just Chatting")])
                .on_press(|ctx| {
                    let (title, category) = (ctx.input_str("title"), ctx.input_str("category"));
                    let mut body = json!({});
                    if !title.trim().is_empty() {
                        body["title"] = json!(title.trim());
                    }
                    if !category.trim().is_empty() {
                        let games = helix::get(&format!("/games?name={}", knobdeck_sdk::oauth::encode(category.trim())))?;
                        let Some(id) = games["data"][0]["id"].as_str() else {
                            return ctx.fail(format!("No Twitch category called \"{}\" (use its exact name)", category.trim()));
                        };
                        body["game_id"] = json!(id);
                    }
                    if body.as_object().is_some_and(|o| o.is_empty()) {
                        return ctx.fail("Set a title or a category");
                    }
                    helix::call("PATCH", &format!("/channels?broadcaster_id={}", helix::me()?), Some(body)).map(|_| ())
                }),
        )
        .action(Action::button("marker", "Stream Marker").icon("bookmark-plus").inputs(vec![input::text("description", "Description (optional)")]).on_press(|ctx| {
            let mut body = json!({"user_id": helix::me()?});
            let d = ctx.input_str("description");
            if !d.trim().is_empty() {
                body["description"] = json!(d.trim());
            }
            helix::call("POST", "/streams/markers", Some(body)).map(|_| ())
        }))
        .action(
            Action::button("create-clip", "Create Clip")
                .icon("clapperboard")
                .inputs(vec![input::checkbox("open", "Open the clip editor")])
                .on_press(|ctx| {
                    let r = helix::call("POST", &format!("/clips?broadcaster_id={}", helix::me()?), None)?;
                    let clip = &r["data"][0];
                    if ctx.input_bool("open") {
                        if let Some(url) = clip["edit_url"].as_str() {
                            open_url(url)?;
                        }
                    }
                    ctx.set_label("Clipped");
                    Ok(())
                }),
        )
        .action(
            Action::button("viewers", "Viewer Count")
                .icon("eye")
                .live(Duration::from_secs(30), |_| Ok(Visual::new().label(viewers_label().unwrap_or_else(|e| e.message().to_string()))))
                .on_press(|ctx| {
                    ctx.set_label(viewers_label()?);
                    Ok(())
                }),
        )
        .action(
            Action::toggle("channel-live", "Channel Monitor")
                .icon("radio")
                .inputs(vec![input::text("channel", "Channel").placeholder("streamer name")])
                .state("offline", "Offline", "radio")
                .state("live", "Live", "circle-dot")
                .live(Duration::from_secs(60), |ic| {
                    let channel = ic.input_str("channel").trim().to_lowercase();
                    if channel.is_empty() {
                        return Ok(Visual::new());
                    }
                    let s = helix::get(&format!("/streams?user_login={}", knobdeck_sdk::oauth::encode(&channel)))?;
                    Ok(match s["data"][0]["viewer_count"].as_i64() {
                        Some(n) => Visual::new().state("live").label(format!("{channel} · {n}")),
                        None => Visual::new().state("offline").label(channel),
                    })
                })
                // Press opens the channel.
                .on_press(|ctx| {
                    let channel = ctx.input_str("channel").trim().to_lowercase();
                    if channel.is_empty() {
                        return ctx.fail("Enter a channel name");
                    }
                    open_url(&format!("https://www.twitch.tv/{channel}"))
                }),
        )
        .run()
}
