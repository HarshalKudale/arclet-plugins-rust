//! Streamlabs Desktop setup: check the token against the running app, then save it.

#[path = "../api.rs"]
#[allow(dead_code)]
mod api;

fn main() {
    knobdeck_sdk::run_setup(|ctx| {
        let token = ctx.input_str("token").trim().to_string();
        let port = match ctx.input_str("port").trim() {
            "" => "59650".to_string(),
            p => p.to_string(),
        };
        if token.is_empty() {
            return ctx.fail("Paste the API token from Streamlabs Desktop: Settings ▸ Remote Control ▸ click the QR code ▸ Show details");
        }
        if !matches!(port.parse::<u32>(), Ok(1..=65535)) {
            return ctx.fail("Enter a valid port (default 59650)");
        }
        let c = api::Config { host: "127.0.0.1".into(), port: port.clone(), token: token.clone() };
        match check(&c) {
            Ok(n) => ctx.log(format!("[setup] Streamlabs connected, {n} scene(s)")),
            Err(e) => return ctx.fail(format!("{} — keep Streamlabs Desktop open while running setup.", e.message())),
        }
        knobdeck_sdk::save_env(&[("STREAMLABS_TOKEN", &token), ("STREAMLABS_PORT", &port)])?;
        Ok(())
    })
}

/// Connect, authenticate and read the scenes once.
fn check(c: &api::Config) -> knobdeck_sdk::Result<usize> {
    let mut ws = api::open(c)?;
    api::exchange(&mut ws, 1, "TcpServerService", "auth", knobdeck_sdk::json!([c.token])).map_err(|e| knobdeck_sdk::Error::msg(format!("Streamlabs rejected the token ({})", e.message())))?;
    Ok(api::exchange(&mut ws, 2, "ScenesService", "getScenes", knobdeck_sdk::json!([]))?.as_array().map_or(0, Vec::len))
}
