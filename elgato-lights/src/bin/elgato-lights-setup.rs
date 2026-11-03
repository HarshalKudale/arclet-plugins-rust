//! Elgato Lights setup: find lights (mDNS plus any typed addresses), check they answer, save the list.

// Shared with the plugin; setup uses only discovery and names.
#[allow(dead_code)]
#[path = "../lights.rs"]
mod lights;

use std::time::Duration;

fn main() {
    knobdeck_sdk::run_setup(|ctx| {
        let typed: Vec<String> = ctx.input_str("addresses").split([',', ' ', '\n']).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
        for a in &typed {
            if let Err(e) = lights::name(a) {
                return ctx.fail(format!("{a}: {e}. Check the address in Control Center (light settings) and that it is on."));
            }
        }
        let mut all = typed.clone();
        for a in lights::discover(Duration::from_secs(3)) {
            if !all.contains(&a) {
                all.push(a);
            }
        }
        let named: Vec<String> = all.iter().filter_map(|a| lights::name(a).ok().map(|n| format!("{n} ({a})"))).collect();
        if named.is_empty() {
            return ctx.fail("No Elgato light found. Make sure the light is on and on the same network as this computer, or type its address.");
        }
        ctx.log(format!("[setup] lights: {}", named.join(", ")));
        // Only typed addresses are saved: discovered lights are found again at every start, so a
        // light that gets a new address from the router keeps working.
        knobdeck_sdk::save_env(&[("ELGATO_LIGHTS", &typed.join(","))])?;
        Ok(())
    })
}
