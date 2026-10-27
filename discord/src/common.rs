//! Constants and token storage shared by the plugin and its setup binary.
#![allow(dead_code)]

pub const APP_ID: &str = "1527677329524916424";
pub const RPC_SCOPES: [&str; 11] = [
    "identify",
    "guilds",
    "messages.read",
    "rpc",
    "rpc.notifications.read",
    "rpc.screenshare.read",
    "rpc.screenshare.write",
    "rpc.video.read",
    "rpc.video.write",
    "rpc.voice.read",
    "rpc.voice.write",
];
/// Stored in the plugin data folder (survives updates/reinstalls).
pub const TOKEN_FILE: &str = "discord-rpc-token.json";

/// Runtime env var, else the value inlined from `.env` at build time.
pub fn client_id() -> String {
    std::env::var("DISCORD_CLIENT_ID")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| option_env!("DISCORD_CLIENT_ID").map(String::from))
        .unwrap_or_else(|| APP_ID.into())
}

/// The Arclet website endpoint that swaps an authorization code or refresh
/// token for an access token (it holds the client secret, like Elgato's
/// discord-plugin-auth server). Runtime env var, else the build-time value.
pub fn token_url() -> Option<String> {
    std::env::var("ARCLET_DISCORD_TOKEN_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| option_env!("ARCLET_DISCORD_TOKEN_URL").map(String::from))
        .filter(|v| !v.is_empty())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// POST `body` (`{"code"}` or `{"refresh_token"}`) to the token worker and
/// store the returned tokens. Returns the new access token.
pub fn exchange(body: arclet_sdk::Value) -> Result<String, String> {
    let url = token_url().ok_or("ARCLET_DISCORD_TOKEN_URL is not configured for this build of the Discord plugin.")?;
    let reply: arclet_sdk::Value = match ureq::post(&url).timeout(std::time::Duration::from_secs(15)).send_json(body) {
        Ok(r) => r.into_json().map_err(|e| format!("Discord token exchange returned invalid JSON: {e}"))?,
        Err(ureq::Error::Status(_, r)) => {
            let v: arclet_sdk::Value = r.into_json().unwrap_or_default();
            return Err(format!("Discord token exchange failed: {}", v["error"].as_str().unwrap_or("unknown error")));
        }
        Err(e) => return Err(format!("Discord token exchange failed: {e}")),
    };
    let access = reply["access_token"].as_str().filter(|t| !t.is_empty()).ok_or("Discord token exchange returned no access_token")?;
    let saved = serde_json::json!({
        "access_token": access,
        "refresh_token": reply["refresh_token"],
        "expires_at": now_secs() + reply["expires_in"].as_u64().unwrap_or(0),
    });
    std::fs::write(token_path(), format!("{saved:#}")).map_err(|e| format!("failed to save Discord token: {e}"))?;
    Ok(access.to_string())
}

fn stored() -> Option<arclet_sdk::Value> {
    std::fs::read_to_string(token_path()).ok()?.parse().ok()
}

/// Renew the access token with the stored refresh token. None if there is no
/// refresh token or the worker refused it (the user must run setup again).
pub fn refresh_token() -> Option<String> {
    let refresh = stored()?["refresh_token"].as_str()?.to_string();
    exchange(serde_json::json!({"refresh_token": refresh}))
        .map_err(|e| arclet_sdk::log(format!("token refresh failed: {e}")))
        .ok()
}

pub fn token_path() -> std::path::PathBuf {
    arclet_sdk::data_file(TOKEN_FILE)
}

/// The stored access token, renewed first when it is within a day of expiry.
/// Tokens saved by older builds have no `expires_at` and are used as is.
pub fn load_token() -> Option<String> {
    let v = stored()?;
    let token = v["access_token"].as_str().filter(|t| !t.is_empty())?.to_string();
    match v["expires_at"].as_u64() {
        Some(at) if at < now_secs() + 86_400 => refresh_token().or(Some(token)),
        _ => Some(token),
    }
}

pub fn clear_token() {
    let _ = std::fs::remove_file(token_path());
}
