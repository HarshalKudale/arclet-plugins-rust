//! YouTube setup: Google sign-in through the browser (PKCE on a loopback port).

#[path = "../yt.rs"]
#[allow(dead_code)]
mod yt;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// Desktop clients may use any loopback port.
const REDIRECT_PORT: u16 = 47812;
/// Read-only for now (privacy): enough for viewers and the dashboard. The commented-out write
/// actions (chat, ads, go live) need "https://www.googleapis.com/auth/youtube.force-ssl" instead.
const SCOPE: &str = "https://www.googleapis.com/auth/youtube.readonly";

fn main() {
    knobdeck_sdk::run_setup(|ctx| {
        let (id, secret) = (ctx.input_str("client_id").trim().to_string(), ctx.input_str("client_secret").trim().to_string());
        if id.is_empty() != secret.is_empty() {
            return ctx.fail("Enter both the client ID and the client secret of your Google app, or neither");
        }
        // The sign-in below reads these, so a typed app applies to it too.
        std::env::set_var("GOOGLE_CLIENT_ID", &id);
        std::env::set_var("GOOGLE_CLIENT_SECRET", &secret);
        if yt::client_id().is_empty() || yt::token_url().is_empty() {
            return ctx.fail("This build has no Google app for YouTube; enter your own Google app's client ID and secret");
        }
        // offline + consent: Google returns a refresh token only when asked for it this way.
        yt::auth().authorize_pkce(AUTH_URL, SCOPE, REDIRECT_PORT, &[("access_type", "offline"), ("prompt", "consent")])?;
        let ch = yt::get("/channels?part=snippet&mine=true")?;
        match ch["items"][0]["snippet"]["title"].as_str() {
            Some(t) => ctx.log(format!("[setup] signed in to YouTube channel {t}")),
            None => return ctx.fail("This Google account has no YouTube channel"),
        }
        // Empty values keep using KnobDeck's app.
        knobdeck_sdk::save_env(&[("GOOGLE_CLIENT_ID", &id), ("GOOGLE_CLIENT_SECRET", &secret)])?;
        Ok(())
    })
}
