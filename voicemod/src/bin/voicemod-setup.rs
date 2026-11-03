//! Voicemod setup: register with the running app (built-in or pasted client key), then save the key.

#[path = "../api.rs"]
#[allow(dead_code)]
mod api;

fn main() {
    knobdeck_sdk::run_setup(|ctx| {
        let typed = ctx.input_str("key").trim().to_string();
        let key = if typed.is_empty() { option_env!("VOICEMOD_CLIENT_KEY").unwrap_or("").to_string() } else { typed.clone() };
        if key.is_empty() {
            return ctx.fail("This build has no Voicemod client key; paste the one Voicemod gave you");
        }
        if let Err(e) = api::connect(&key) {
            let hint = if e.message().contains("client key") { "check the key" } else { "open Voicemod and run setup again" };
            return ctx.fail(format!("{} — {hint}.", e.message()));
        }
        ctx.log("[setup] Voicemod connected");
        // An empty value keeps using the built-in key.
        knobdeck_sdk::save_env(&[("VOICEMOD_CLIENT_KEY", &typed)])?;
        Ok(())
    })
}
