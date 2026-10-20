//! Discord first-time setup: authorize through the Discord desktop client (native
//! popup, no browser), exchange the code for an access token through the Arclet
//! website (which holds the client secret), and save it to
//! `discord-rpc-token.json` in the plugin data folder.

#[path = "../common.rs"]
mod common;
#[path = "../rpc.rs"]
mod rpc;

use arclet_sdk::{json, Error, Value};
use std::time::Duration;

fn main() {
    arclet_sdk::run_setup(|ctx| {
        let client_id = common::client_id();
        let Some(token_url) = common::token_url() else {
            return ctx.fail("ARCLET_DISCORD_TOKEN_URL is not configured for this build of the Discord plugin.");
        };

        ctx.log("[setup] Connecting to Discord RPC...");
        ctx.log("[setup] An authorization popup will appear inside the Discord client.");

        let rpc = rpc::Rpc::connect(&client_id).map_err(|e| Error::msg(e.message))?;
        let code = rpc
            .request("AUTHORIZE", json!({"scopes": common::RPC_SCOPES, "client_id": client_id}))
            .map_err(|e| Error::msg(e.message))?["code"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| Error::msg("Discord AUTHORIZE returned no code — did the user deny the request?"))?;

        let token: Value = ureq::post(&token_url)
            .timeout(Duration::from_secs(15))
            .send_json(json!({"code": code}))
            .map_err(|e| Error::msg(format!("Discord token exchange failed: {e}")))?
            .into_json()?;
        let access = token["access_token"].as_str().ok_or_else(|| Error::msg("Discord token exchange returned no access_token"))?;

        rpc.authenticate(access).map_err(|e| Error::msg(e.message))?;
        std::fs::write(common::token_path(), serde_json_pretty(&json!({"access_token": access})))?;
        ctx.log("[setup] Discord RPC authorized — token saved.");
        rpc.close();
        ctx.log("[setup] Setup complete! The Discord plugin will start now.");
        Ok(())
    })
}

fn serde_json_pretty(v: &Value) -> String {
    format!("{v:#}")
}
