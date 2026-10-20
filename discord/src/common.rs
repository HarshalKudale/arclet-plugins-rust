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
/// Relative to the plugin folder (the host's cwd).
pub const TOKEN_FILE: &str = "discord-rpc-token.json";

/// Runtime env var, else the value inlined from `.env` at build time.
pub fn client_id() -> String {
    std::env::var("DISCORD_CLIENT_ID")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| option_env!("DISCORD_CLIENT_ID").map(String::from))
        .unwrap_or_else(|| APP_ID.into())
}

pub fn client_secret() -> String {
    std::env::var("DISCORD_CLIENT_SECRET")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| option_env!("DISCORD_CLIENT_SECRET").map(String::from))
        .unwrap_or_default()
}

pub fn load_token() -> Option<String> {
    let v: arclet_sdk::Value = std::fs::read_to_string(TOKEN_FILE).ok()?.parse().ok()?;
    v["access_token"].as_str().filter(|t| !t.is_empty()).map(String::from)
}

pub fn clear_token() {
    let _ = std::fs::remove_file(TOKEN_FILE);
}
