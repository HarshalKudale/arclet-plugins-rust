//! OBS setup: save the connection details collected by the setup page to `.env`.

fn main() {
    knobdeck_sdk::run_setup(|ctx| {
        // JS `inputs.x ?? default`: only a missing/null field takes the default.
        let get = |key: &str, default: &str| match ctx.inputs.get(key) {
            None | Some(knobdeck_sdk::Value::Null) => default.to_string(),
            Some(_) => ctx.input_str(key).trim().to_string(),
        };
        let (host, port, password) = (get("host", "localhost"), get("port", "4455"), get("password", ""));

        if host.is_empty() {
            return ctx.fail("OBS host cannot be empty");
        }
        match port.parse::<u32>() {
            Ok(1..=65535) => {}
            _ => return ctx.fail("Invalid port number"),
        }

        std::fs::write(ctx.data_dir().join(".env"), format!("OBS_HOST={host}\nOBS_PORT={port}\nOBS_PASSWORD={password}\n"))?;
        ctx.log(format!("[setup] OBS connection configured: ws://{host}:{port}"));
        Ok(())
    })
}
