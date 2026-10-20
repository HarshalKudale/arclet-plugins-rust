//! IntelliJ setup: verify the companion is reachable, then save `.env`.

use std::time::Duration;

fn main() {
    arclet_sdk::run_setup(|ctx| {
        // JS `inputs.x ?? default`: only a missing/null field takes the default.
        let get = |key: &str, default: &str| match ctx.inputs.get(key) {
            None | Some(arclet_sdk::Value::Null) => default.to_string(),
            Some(_) => ctx.input_str(key).trim().to_string(),
        };
        let (host, port, password) = (get("host", "127.0.0.1"), get("port", "21420"), get("password", ""));

        if host.is_empty() {
            return ctx.fail("IntelliJ host cannot be empty");
        }
        match port.parse::<u32>() {
            Ok(1..=65535) => {}
            _ => return ctx.fail("Enter a valid IntelliJ action-service port"),
        }

        let base = format!("http://{host}:{port}");
        let mut req = ureq::get(&format!("{base}/api/action/streamdeck.show.action.browser")).timeout(Duration::from_secs(10));
        if !password.is_empty() {
            req = req.set("Authorization", &password);
        }
        match req.call() {
            Ok(_) => {}
            Err(ureq::Error::Status(403 | 500, _)) => return ctx.fail("The IntelliJ companion plugin rejected the configured password."),
            Err(ureq::Error::Status(code, _)) => return ctx.fail(format!("The IntelliJ companion plugin returned HTTP {code}.")),
            Err(e) => {
                return ctx.fail(format!(
                    "Could not reach the IntelliJ companion plugin at {base}. Install the Arclet IntelliJ plugin and enable its action service. ({e})"
                ))
            }
        }

        std::fs::write(".env", format!("IDEA_HOST={host}
IDEA_PORT={port}
IDEA_PASSWORD={password}
"))?;
        ctx.log(format!("[setup] IntelliJ companion plugin configured: {base}"));
        Ok(())
    })
}
