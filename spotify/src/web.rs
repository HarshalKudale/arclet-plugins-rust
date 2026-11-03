//! Spotify Web API with the user's sign-in (PKCE, no client secret). Shared with the setup program.

use knobdeck_sdk::oauth::OAuth;
use knobdeck_sdk::{env_or, Error, Result, Value};
use std::time::Duration;

pub const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";

/// KnobDeck's Spotify app (CI variable SPOTIFY_CLIENT_ID), or the user's own from setup.
pub fn client_id() -> String {
    env_or("SPOTIFY_CLIENT_ID", option_env!("SPOTIFY_CLIENT_ID").unwrap_or(""))
}

pub fn auth() -> OAuth {
    OAuth::new(&env_or("SPOTIFY_TOKEN_URL", TOKEN_URL), &client_id())
}

fn base() -> String {
    env_or("SPOTIFY_API_BASE", "https://api.spotify.com/v1")
}

/// Turn Spotify's error reply into words a user can act on.
fn api_error(code: u16, body: &Value) -> Error {
    let reason = body["error"]["reason"].as_str().unwrap_or("");
    let message = body["error"]["message"].as_str().unwrap_or("");
    Error::msg(match (code, reason) {
        (403, _) if reason == "PREMIUM_REQUIRED" || message.to_lowercase().contains("premium") => "This needs Spotify Premium".to_string(),
        (404, _) | (_, "NO_ACTIVE_DEVICE") => "Start playing Spotify on a device first".to_string(),
        (429, _) => "Spotify is rate-limiting requests; try again in a moment".to_string(),
        _ => format!("Spotify said: {} (HTTP {code})", if message.is_empty() { reason } else { message }),
    })
}

/// One API call; a 401 refreshes the sign-in and retries once. Empty replies (204) are `Null`.
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
            Err(e) => return Err(Error::msg(format!("Spotify is unreachable ({e})"))),
        }
    }
    Err(Error::msg("Spotify sign-in expired — run setup again"))
}

pub fn get(path: &str) -> Result<Value> {
    call("GET", path, None)
}

/// The playback state (`Null` when nothing is playing anywhere).
pub fn player() -> Result<Value> {
    get("/me/player")
}

/// Id and URI of the current track.
pub fn current_track() -> Result<(String, String)> {
    let p = get("/me/player/currently-playing")?;
    let item = &p["item"];
    match (item["id"].as_str(), item["uri"].as_str()) {
        (Some(id), Some(uri)) => Ok((id.to_string(), uri.to_string())),
        _ => Err(Error::msg("Nothing is playing")),
    }
}

/// The user's playlists (first 50), `(uri, name)`, plus Liked Songs first.
pub fn playlists() -> Result<Vec<(String, String)>> {
    let me = get("/me")?;
    let mut out = vec![(format!("{}:collection", me["uri"].as_str().unwrap_or("spotify:user:me")), "Liked Songs".to_string())];
    let page = get("/me/playlists?limit=50")?;
    for p in page["items"].as_array().cloned().unwrap_or_default() {
        if let (Some(uri), Some(name)) = (p["uri"].as_str(), p["name"].as_str()) {
            out.push((uri.to_string(), name.to_string()));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use knobdeck_sdk::json;

    #[test]
    fn errors_read_well() {
        let premium = json!({"error": {"status": 403, "message": "Player command failed: Premium required", "reason": "PREMIUM_REQUIRED"}});
        assert_eq!(api_error(403, &premium).message(), "This needs Spotify Premium");
        assert_eq!(api_error(404, &json!({"error": {"reason": "NO_ACTIVE_DEVICE"}})).message(), "Start playing Spotify on a device first");
        assert_eq!(api_error(400, &json!({"error": {"message": "Bad thing"}})).message(), "Spotify said: Bad thing (HTTP 400)");
    }
}
