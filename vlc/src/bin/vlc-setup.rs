//! VLC setup: verify the HTTP interface is reachable, then save `.env`.

use base64::Engine;
use std::time::Duration;

fn main() {
    arclet_sdk::run_setup(|ctx| {
        // JS `inputs.x ?? default`: only a missing/null field takes the default.
        let get = |key: &str, default: &str| match ctx.inputs.get(key) {
            None | Some(arclet_sdk::Value::Null) => default.to_string(),
            Some(_) => ctx.input_str(key).trim().to_string(),
        };
        let (host, port, password) = (get("host", "127.0.0.1"), get("port", "8080"), get("password", ""));

        if host.is_empty() {
            return ctx.fail("VLC host cannot be empty");
        }
        match port.parse::<u32>() {
            Ok(1..=65535) => {}
            _ => return ctx.fail("Enter a valid VLC HTTP port"),
        }

        let base = format!("http://{host}:{port}");
        let mut req = ureq::get(&format!("{base}/requests/status.json")).timeout(Duration::from_secs(10));
        if !password.is_empty() {
            req = req.set("Authorization", &format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!(":{password}"))));
        }
        match req.call() {
            Ok(r) => {
                if let Err(e) = r.into_json::<arclet_sdk::Value>() {
                    return ctx.fail(format!("VLC responded with invalid JSON: {e}"));
                }
            }
            Err(ureq::Error::Status(401, _)) => return ctx.fail("VLC rejected the password. Check the Lua HTTP password in VLC settings."),
            Err(ureq::Error::Status(code, _)) => {
                return ctx.fail(format!("VLC HTTP interface returned {code}. Make sure the Web interface is enabled in VLC."))
            }
            Err(e) => {
                return ctx.fail(format!(
                    "Could not reach VLC at {base}. Make sure VLC is open and its Web interface is enabled. ({e})"
                ))
            }
        }

        std::fs::write(ctx.data_dir().join(".env"), format!("VLC_HOST={host}\nVLC_PORT={port}\nVLC_PASSWORD={password}\n"))?;
        ctx.log(format!("[setup] VLC HTTP configured: {base}"));
        Ok(())
    })
}
