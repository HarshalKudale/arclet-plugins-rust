//! VS Code setup: verify the companion is reachable, then save `.env`.

use std::time::Duration;

fn main() {
    knobdeck_sdk::run_setup(|ctx| {
        // JS `inputs.x ?? default`: only a missing/null field takes the default.
        let get = |key: &str, default: &str| match ctx.inputs.get(key) {
            None | Some(knobdeck_sdk::Value::Null) => default.to_string(),
            Some(_) => ctx.input_str(key).trim().to_string(),
        };
        let (host, port, password) = (get("host", "127.0.0.1"), get("port", "32145"), get("password", ""));

        if host.is_empty() {
            return ctx.fail("VS Code host cannot be empty");
        }
        match port.parse::<u32>() {
            Ok(1..=65535) => {}
            _ => return ctx.fail("Enter a valid VS Code companion port"),
        }

        let base = format!("http://{host}:{port}");
        let mut req = ureq::get(&format!("{base}/health")).timeout(Duration::from_secs(10));
        if !password.is_empty() {
            req = req.set("Authorization", &password);
        }
        match req.call() {
            Ok(_) => {}
            Err(ureq::Error::Status(401 | 403, _)) => return ctx.fail("The VS Code companion rejected the configured password."),
            Err(ureq::Error::Status(code, _)) => return ctx.fail(format!("The VS Code companion returned HTTP {code}.")),
            Err(e) => {
                return ctx.fail(format!(
                    "Could not reach the VS Code companion at {base}. Install the KnobDeck VS Code companion extension and enable its action server. ({e})"
                ))
            }
        }

        std::fs::write(ctx.data_dir().join(".env"), format!("VSCODE_HOST={host}
VSCODE_PORT={port}
VSCODE_PASSWORD={password}
"))?;
        ctx.log(format!("[setup] VS Code companion configured: {base}"));
        Ok(())
    })
}
