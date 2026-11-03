//! Twitch setup: device-code sign-in (opens twitch.tv/activate), then save the optional client id.

#[path = "../helix.rs"]
#[allow(dead_code)]
mod helix;

const DEVICE_URL: &str = "https://id.twitch.tv/oauth2/device";
const SCOPES: &str = "user:write:chat moderator:manage:chat_settings moderator:manage:chat_messages channel:edit:commercial channel:manage:broadcast clips:edit";

fn main() {
    knobdeck_sdk::run_setup(|ctx| {
        let typed = ctx.input_str("client_id").trim().to_string();
        if !typed.is_empty() {
            std::env::set_var("TWITCH_CLIENT_ID", &typed);
        }
        if helix::client_id().is_empty() {
            return ctx.fail("This build has no Twitch app; register one at dev.twitch.tv (Public client) and paste its client ID");
        }
        helix::auth().device_flow(DEVICE_URL, &[("scopes", SCOPES)])?;
        let me = helix::get("/users")?;
        ctx.log(format!("[setup] signed in to Twitch as {}", me["data"][0]["login"].as_str().unwrap_or("?")));
        // An empty value keeps using KnobDeck's app.
        knobdeck_sdk::save_env(&[("TWITCH_CLIENT_ID", &typed)])?;
        Ok(())
    })
}
