//! Discord first-time setup: authorize through the Discord desktop client (native
//! popup, no browser), exchange the code for an access token, and save it to
//! `discord-rpc-token.json` so the plugin can connect without re-prompting.

#[path = "../common.rs"]
mod common;
#[path = "../rpc.rs"]
mod rpc;

use arclet_sdk::{json, Error, Value};
use std::time::Duration;

fn main() {
    arclet_sdk::run_setup(|ctx| {
        let (client_id, secret) = (common::client_id(), common::client_secret());
        if secret.is_empty() || secret == "your_client_secret_here" {
            return ctx.fail(
                "DISCORD_CLIENT_SECRET is not configured.\nEdit the plugin's .env and set your Discord application client secret,\nthen re-run setup.",
            );
        }

        ctx.log("[setup] Connecting to Discord RPC...");
        ctx.log("[setup] An authorization popup will appear inside the Discord client.");

        let rpc = rpc::Rpc::connect(&client_id).map_err(|e| Error::msg(e.message))?;
        let code = rpc
            .request("AUTHORIZE", json!({"scopes": common::RPC_SCOPES, "client_id": client_id}))
            .map_err(|e| Error::msg(e.message))?["code"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| Error::msg("Discord AUTHORIZE returned no code — did the user deny the request?"))?;

        let token: Value = ureq::post("https://discord.com/api/oauth2/token")
            .timeout(Duration::from_secs(15))
            .send_form(&[
                ("client_id", client_id.as_str()),
                ("client_secret", secret.as_str()),
                ("code", code.as_str()),
                ("grant_type", "authorization_code"),
                ("redirect_uri", "http://127.0.0.1:31416/callback"),
            ])
            .map_err(|e| Error::msg(format!("Discord token exchange failed: {e}")))?
            .into_json()?;
        let access = token["access_token"].as_str().ok_or_else(|| Error::msg("Discord token exchange returned no access_token"))?;

        rpc.authenticate(access).map_err(|e| Error::msg(e.message))?;
        std::fs::write(common::TOKEN_FILE, serde_json_pretty(&json!({"access_token": access})))?;
        ctx.log("[setup] Discord RPC authorized — token saved.");
        rpc.close();
        ctx.log("[setup] Setup complete! The Discord plugin will start now.");
        Ok(())
    })
}

fn serde_json_pretty(v: &Value) -> String {
    format!("{v:#}")
}
