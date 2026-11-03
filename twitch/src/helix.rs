//! Twitch Helix API with the user's sign-in (device-code flow: Twitch has no PKCE for desktop
//! apps, and a public client needs no secret). Shared with the setup program.

use knobdeck_sdk::oauth::OAuth;
use knobdeck_sdk::{env_or, Error, Result, Value};
use std::sync::OnceLock;
use std::time::Duration;

pub const TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";

/// KnobDeck's Twitch app (CI variable TWITCH_CLIENT_ID), or the user's own from setup.
pub fn client_id() -> String {
    env_or("TWITCH_CLIENT_ID", option_env!("TWITCH_CLIENT_ID").unwrap_or(""))
}

pub fn auth() -> OAuth {
    OAuth::new(&env_or("TWITCH_TOKEN_URL", TOKEN_URL), &client_id())
}

fn base() -> String {
    env_or("TWITCH_API_BASE", "https://api.twitch.tv/helix")
}

fn api_error(code: u16, body: &Value) -> Error {
    let message = body["message"].as_str().unwrap_or("");
    Error::msg(match code {
        401 if message.contains("scope") => "Twitch sign-in is missing a permission — run setup again".to_string(),
        403 => format!("Twitch refused: {message}"),
        429 => "Twitch is rate-limiting requests; try again in a moment".to_string(),
        _ => format!("Twitch said: {} (HTTP {code})", if message.is_empty() { "error" } else { message }),
    })
}

/// One Helix call; a 401 refreshes the sign-in and retries once. Empty replies are `Null`.
pub fn call(method: &str, path: &str, body: Option<Value>) -> Result<Value> {
    let auth = auth();
    for attempt in 0..2 {
        let token = if attempt == 0 { auth.access_token()? } else { auth.refresh()?["access_token"].as_str().unwrap_or("").to_string() };
        let req = ureq::request(method, &format!("{}{path}", base())).timeout(Duration::from_secs(10)).set("Authorization", &format!("Bearer {token}")).set("Client-Id", &client_id());
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
            Err(ureq::Error::Status(401, r)) if attempt == 0 => {
                let body: Value = r.into_json().unwrap_or(Value::Null);
                // A missing scope will not be fixed by a refresh.
                if body["message"].as_str().is_some_and(|m| m.contains("scope")) {
                    return Err(api_error(401, &body));
                }
            }
            Err(ureq::Error::Status(code, r)) => return Err(api_error(code, &r.into_json().unwrap_or(Value::Null))),
            Err(e) => return Err(Error::msg(format!("Twitch is unreachable ({e})"))),
        }
    }
    Err(Error::msg("Twitch sign-in expired — run setup again"))
}

pub fn get(path: &str) -> Result<Value> {
    call("GET", path, None)
}

static ME: OnceLock<String> = OnceLock::new();

/// The signed-in user's id (also the broadcaster and moderator id for their own channel).
pub fn me() -> Result<String> {
    if let Some(id) = ME.get() {
        return Ok(id.clone());
    }
    let id = get("/users")?["data"][0]["id"].as_str().ok_or_else(|| Error::msg("Could not read your Twitch account"))?.to_string();
    Ok(ME.get_or_init(|| id).clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use knobdeck_sdk::json;

    #[test]
    fn errors_read_well() {
        assert_eq!(api_error(401, &json!({"message": "Missing scope: clips:edit"})).message(), "Twitch sign-in is missing a permission — run setup again");
        assert_eq!(api_error(400, &json!({"message": "length is invalid"})).message(), "Twitch said: length is invalid (HTTP 400)");
    }
}
