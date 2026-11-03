//! YouTube Data API v3 with the user's Google sign-in. Shared with the setup program.
//!
//! Google's token endpoint wants a client secret even for desktop apps using PKCE. No secret is
//! shipped in the plugin: with KnobDeck's app, code exchange and refresh go through the KnobDeck
//! website (KNOBDECK_GOOGLE_TOKEN_URL, baked in at build time), which adds the secret server-side,
//! as the Discord plugin does. A user with their own Google app can instead enter its id and
//! secret in setup; they are kept only in this computer's plugin data folder.

use knobdeck_sdk::oauth::OAuth;
use knobdeck_sdk::{env_or, Error, Result, Value};
use std::time::Duration;

pub const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// The user's own Google app (from setup's `.env`), if they entered one.
fn own_secret() -> String {
    env_or("GOOGLE_CLIENT_SECRET", "")
}

pub fn client_id() -> String {
    env_or("GOOGLE_CLIENT_ID", option_env!("GOOGLE_CLIENT_ID").unwrap_or(""))
}

/// Where tokens are exchanged: Google directly for the user's own app, else KnobDeck's proxy.
pub fn token_url() -> String {
    if !own_secret().is_empty() {
        return env_or("GOOGLE_TOKEN_URL", GOOGLE_TOKEN_URL);
    }
    env_or("KNOBDECK_GOOGLE_TOKEN_URL", option_env!("KNOBDECK_GOOGLE_TOKEN_URL").unwrap_or(""))
}

pub fn auth() -> OAuth {
    OAuth::new(&token_url(), &client_id()).with_secret(&own_secret())
}

fn base() -> String {
    env_or("YOUTUBE_API_BASE", "https://www.googleapis.com/youtube/v3")
}

fn api_error(code: u16, body: &Value) -> Error {
    let reason = body["error"]["errors"][0]["reason"].as_str().unwrap_or("");
    let message = body["error"]["message"].as_str().unwrap_or("");
    Error::msg(match (code, reason) {
        (403, "quotaExceeded") => "YouTube's daily API quota is used up; try again tomorrow".to_string(),
        (403, "liveChatDisabled") | (403, "liveChatEnded") => "Live chat is off for this stream".to_string(),
        (403, "insufficientLivePermissions") | (403, "livePermissionBlocked") => "This YouTube channel can't do that (live streaming or ads not enabled)".to_string(),
        (403, "redundantTransition") => "The stream is already in that state".to_string(),
        (403, "invalidTransition") => "YouTube can't start this stream yet — is the encoder (OBS…) sending video?".to_string(),
        _ => format!("YouTube said: {} (HTTP {code})", if message.is_empty() { reason } else { message }),
    })
}

/// One API call; a 401 refreshes the sign-in and retries once.
pub fn call(method: &str, path: &str, body: Option<Value>) -> Result<Value> {
    let auth = auth();
    for attempt in 0..2 {
        let token = if attempt == 0 { auth.access_token()? } else { auth.refresh()?["access_token"].as_str().unwrap_or("").to_string() };
        let req = ureq::request(method, &format!("{}{path}", base())).timeout(Duration::from_secs(10)).set("Authorization", &format!("Bearer {token}"));
        let res = match &body {
            Some(b) => req.send_json(b.clone()),
            None if method == "GET" => req.call(),
            None => req.send_bytes(&[]),
        };
        match res {
            Ok(r) => {
                let text = r.into_string().unwrap_or_default();
                return Ok(if text.trim().is_empty() { Value::Null } else { text.parse().unwrap_or(Value::Null) });
            }
            Err(ureq::Error::Status(401, _)) if attempt == 0 => continue,
            Err(ureq::Error::Status(code, r)) => return Err(api_error(code, &r.into_json().unwrap_or(Value::Null))),
            Err(e) => return Err(Error::msg(format!("YouTube is unreachable ({e})"))),
        }
    }
    Err(Error::msg("YouTube sign-in expired — run setup again"))
}

pub fn get(path: &str) -> Result<Value> {
    call("GET", path, None)
}

/// The broadcast that is live now (`None` when not live).
pub fn active_broadcast() -> Result<Option<Value>> {
    Ok(get("/liveBroadcasts?part=id,snippet,status&broadcastStatus=active&broadcastType=all")?["items"].as_array().and_then(|a| a.first().cloned()))
}

// Write access (youtube.force-ssl) is not requested for now, for privacy: the plugin signs in
// read-only. These helpers serve the write actions commented out in main.rs; restore them together
// with that scope in youtube-setup.rs.
//
// /// The next scheduled broadcast (to go live with).
// pub fn upcoming_broadcast() -> Result<Option<Value>> {
//     Ok(get("/liveBroadcasts?part=id,snippet,status&broadcastStatus=upcoming&broadcastType=all")?["items"].as_array().and_then(|a| a.first().cloned()))
// }
//
// pub fn live() -> Result<Value> {
//     active_broadcast()?.ok_or_else(|| Error::msg("You are not live on YouTube"))
// }
//
// pub fn chat(text: &str) -> Result<()> {
//     let b = live()?;
//     let chat_id = b["snippet"]["liveChatId"].as_str().ok_or_else(|| Error::msg("Live chat is off for this stream"))?;
//     call("POST", "/liveChat/messages?part=snippet", Some(json!({"snippet": {"liveChatId": chat_id, "type": "textMessageEvent", "textMessageDetails": {"messageText": text}}}))).map(|_| ())
// }

#[cfg(test)]
mod tests {
    use super::*;
    use knobdeck_sdk::json;

    #[test]
    fn errors_read_well() {
        let quota = json!({"error": {"message": "x", "errors": [{"reason": "quotaExceeded"}]}});
        assert_eq!(api_error(403, &quota).message(), "YouTube's daily API quota is used up; try again tomorrow");
        assert_eq!(api_error(400, &json!({"error": {"message": "Bad"}})).message(), "YouTube said: Bad (HTTP 400)");
    }
}
