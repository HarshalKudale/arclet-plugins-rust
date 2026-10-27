//! Discord first-time setup: authorize through the Discord desktop client (native
//! popup, no browser), exchange the code for tokens through the Arclet website
//! (which holds the client secret), and save them to
//! `discord-rpc-token.json` in the plugin data folder.

#[path = "../common.rs"]
mod common;
#[path = "../rpc.rs"]
mod rpc;

use arclet_sdk::{json, Error};

fn main() {
    arclet_sdk::run_setup(|ctx| {
        let client_id = common::client_id();
        if common::token_url().is_none() {
            return ctx.fail("ARCLET_DISCORD_TOKEN_URL is not configured for this build of the Discord plugin.");
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

        let access = common::exchange(json!({"code": code})).map_err(Error::msg)?;

        rpc.authenticate(&access).map_err(|e| Error::msg(e.message))?;
        ctx.log("[setup] Discord RPC authorized — token saved.");
        rpc.close();
        ctx.log("[setup] Setup complete! The Discord plugin will start now.");
        Ok(())
    })
}
