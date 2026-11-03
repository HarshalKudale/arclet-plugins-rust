//! Spotify setup: sign in through the browser (PKCE), then save the optional client id.

#[path = "../web.rs"]
#[allow(dead_code)]
mod web;

const AUTH_URL: &str = "https://accounts.spotify.com/authorize";
/// Must be registered in the Spotify app as `http://127.0.0.1:47811/callback`.
const REDIRECT_PORT: u16 = 47811;
const SCOPES: &str = "user-read-playback-state user-modify-playback-state user-read-currently-playing user-library-read user-library-modify playlist-read-private playlist-read-collaborative playlist-modify-private playlist-modify-public";

fn main() {
    knobdeck_sdk::run_setup(|ctx| {
        let typed = ctx.input_str("client_id").trim().to_string();
        if !typed.is_empty() {
            // The sign-in below reads SPOTIFY_CLIENT_ID, so use the typed one for it too.
            std::env::set_var("SPOTIFY_CLIENT_ID", &typed);
        }
        if web::client_id().is_empty() {
            return ctx.fail("This build has no Spotify app; create one at developer.spotify.com and paste its client ID");
        }
        web::auth().authorize_pkce(AUTH_URL, SCOPES, REDIRECT_PORT, &[])?;
        let me = web::get("/me")?;
        ctx.log(format!("[setup] signed in to Spotify as {}", me["display_name"].as_str().or(me["id"].as_str()).unwrap_or("?")));
        // An empty value keeps using KnobDeck's app.
        knobdeck_sdk::save_env(&[("SPOTIFY_CLIENT_ID", &typed)])?;
        Ok(())
    })
}
