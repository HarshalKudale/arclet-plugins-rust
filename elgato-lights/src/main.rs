//! Elgato Lights plugin for KnobDeck — Key Light, Key Light Air/Mini, Ring Light and Light Strip
//! over their local HTTP API. Lights are found by mDNS; setup also accepts addresses by hand.

mod lights;

use knobdeck_sdk::input::{self, InputExt};
use knobdeck_sdk::{env_or, json, load_env, Action, Ctx, Error, Plugin, Result, Value, Visual};
use std::sync::Mutex;
use std::time::Duration;

const ALL: &str = "__all__";
const POLL: Duration = Duration::from_secs(3);

/// Known lights: (address, name). Filled from setup's list plus discovery.
static LIGHTS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

fn configured() -> Vec<String> {
    env_or("ELGATO_LIGHTS", "").split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// Re-scan: configured addresses plus whatever answers mDNS now.
fn refresh_lights() -> Vec<(String, String)> {
    let mut addrs = configured();
    for a in lights::discover(Duration::from_millis(1500)) {
        if !addrs.contains(&a) {
            addrs.push(a);
        }
    }
    let found: Vec<(String, String)> = addrs.into_iter().filter_map(|a| lights::name(&a).ok().map(|n| (a, n))).collect();
    *LIGHTS.lock().unwrap() = found.clone();
    found
}

fn known() -> Vec<(String, String)> {
    let cached = LIGHTS.lock().unwrap().clone();
    if cached.is_empty() {
        refresh_lights()
    } else {
        cached
    }
}

/// The addresses an action targets: one light, or every known light.
fn targets(inputs: &Value) -> Result<Vec<String>> {
    let pick = inputs["light"].as_str().unwrap_or(ALL);
    let list: Vec<String> = if pick.is_empty() || pick == ALL { known().into_iter().map(|(a, _)| a).collect() } else { vec![pick.to_string()] };
    if list.is_empty() {
        return Err(Error::msg("No Elgato light found — make sure it is on and on the same network, or add its address in setup"));
    }
    Ok(list)
}

/// State of the first targeted light.
fn first_state(inputs: &Value) -> Result<Value> {
    lights::state(&targets(inputs)?[0])
}

fn set_all(inputs: &Value, fields: Value) -> Result<Value> {
    let mut last = Value::Null;
    for a in targets(inputs)? {
        last = lights::set(&a, fields.clone())?;
    }
    Ok(last)
}

fn light_input() -> Value {
    input::query_select("light", "Light", "get-lights", &[]).with("default", ALL)
}

fn step_of(ctx: &Ctx, default: f64) -> f64 {
    ctx.input_f64("step").filter(|s| *s > 0.0).unwrap_or(default)
}

fn on_label(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

fn main() {
    load_env();
    if std::env::var("KNOBDECK_CODEGEN").as_deref() != Ok("1") {
        // Warm the list so the first press does not wait for discovery.
        std::thread::spawn(refresh_lights);
    }
    Plugin::new(&format!("elgato-lights-{}", std::env::consts::OS), "Elgato Lights")
        .version(env!("CARGO_PKG_VERSION"))
        .platforms(&[std::env::consts::OS]) // one package per OS
        .author("KnobDeck")
        .description("Key Light, Ring Light and Light Strip: power, brightness, temperature and color.")
        .icon("lamp")
        .setup("elgato-lights-setup", None)
        .setup_inputs(vec![input::text("addresses", "Light addresses (optional)").placeholder("192.168.1.20, 192.168.1.21").with("help", "Lights on this network are found automatically. Add addresses only for lights that are not found.")])
        .query("get-lights", |_| {
            let mut opts = vec![json!({"value": ALL, "label": "All lights"})];
            opts.extend(refresh_lights().into_iter().map(|(a, n)| json!({"value": a, "label": format!("{n} ({a})")})));
            Ok(Value::Array(opts))
        })
        .action(
            Action::toggle("power", "Power")
                .icon("lightbulb")
                .inputs(vec![light_input()])
                .state("off", "Off", "lightbulb-off")
                .state("on", "On", "lightbulb")
                .live(POLL, |ic| Ok(Visual::new().state(on_label(first_state(&ic.inputs)?["on"] == 1))))
                .on_press(|ctx| {
                    let on = first_state(&ctx.inputs)?["on"] != 1;
                    set_all(&ctx.inputs, json!({"on": on as u8}))?;
                    ctx.set_state(on_label(on));
                    Ok(())
                }),
        )
        .action(
            Action::slider("brightness", "Brightness", 0.0, 100.0)
                .icon("sun")
                .unit("%")
                .inputs(vec![light_input(), input::text("step", "Step (%)").placeholder("5").default_value("5")])
                .live(POLL, |ic| Ok(Visual::new().state(first_state(&ic.inputs)?["brightness"].as_i64().unwrap_or(0))))
                .on_adjust(|ctx| {
                    let target = match ctx.value {
                        Some(v) => v,
                        None => first_state(&ctx.inputs)?["brightness"].as_f64().unwrap_or(0.0) + step_of(ctx, 5.0) * ctx.delta.unwrap_or(0.0),
                    }
                    .round()
                    .clamp(3.0, 100.0); // the lights' minimum is 3%
                    set_all(&ctx.inputs, json!({"on": 1, "brightness": target as i64}))?;
                    ctx.set_state(target);
                    Ok(())
                }),
        )
        .action(Action::button("set-brightness", "Set Brightness").icon("sun").inputs(vec![light_input(), input::text("value", "Brightness (%)").placeholder("50").default_value("50")]).on_press(|ctx| {
            let Some(v) = ctx.input_f64("value").filter(|v| (0.0..=100.0).contains(v)) else {
                return ctx.fail("Brightness must be 0–100");
            };
            set_all(&ctx.inputs, json!({"on": 1, "brightness": v.round().max(3.0) as i64})).map(|_| ())
        }))
        .action(
            Action::slider("temperature", "Temperature", 2900.0, 7000.0)
                .icon("thermometer")
                .unit("K")
                .step(50.0)
                .inputs(vec![light_input(), input::text("step", "Step (K)").placeholder("100").default_value("100")])
                .live(POLL, |ic| {
                    let s = first_state(&ic.inputs)?;
                    // A Light Strip has no white temperature; leave its key alone.
                    Ok(match s["temperature"].as_i64() {
                        Some(t) => Visual::new().state(lights::api_to_kelvin(t)),
                        None => Visual::new().label("No temperature"),
                    })
                })
                .on_adjust(|ctx| {
                    let target = match ctx.value {
                        Some(v) => v,
                        None => lights::api_to_kelvin(first_state(&ctx.inputs)?["temperature"].as_i64().unwrap_or(213)) as f64 + step_of(ctx, 100.0) * ctx.delta.unwrap_or(0.0),
                    }
                    .clamp(2900.0, 7000.0);
                    let s = set_all(&ctx.inputs, json!({"on": 1, "temperature": lights::kelvin_to_api(target)}))?;
                    ctx.set_state(s["temperature"].as_i64().map_or(target as i64, lights::api_to_kelvin));
                    Ok(())
                }),
        )
        .action(Action::button("set-temperature", "Set Temperature").icon("thermometer").inputs(vec![light_input(), input::text("value", "Temperature (K, 2900–7000)").placeholder("5000").default_value("5000")]).on_press(|ctx| {
            let Some(k) = ctx.input_f64("value").filter(|k| (2900.0..=7000.0).contains(k)) else {
                return ctx.fail("Temperature must be 2900–7000 K");
            };
            set_all(&ctx.inputs, json!({"on": 1, "temperature": lights::kelvin_to_api(k)})).map(|_| ())
        }))
        .action(Action::button("set-color", "Set Color (Light Strip)").icon("palette").inputs(vec![light_input(), input::text("color", "Color (#RRGGBB)").placeholder("#FF8800").default_value("#FF8800")]).on_press(|ctx| {
            let Some((hue, saturation, brightness)) = lights::hex_to_hsb(&ctx.input_str("color")) else {
                return ctx.fail("Color must look like #FF8800");
            };
            set_all(&ctx.inputs, json!({"on": 1, "hue": hue, "saturation": saturation, "brightness": brightness.max(3.0)})).map(|_| ())
        }))
        .action(
            Action::button("battery", "Battery (Key Light Mini)")
                .icon("battery")
                .inputs(vec![input::query_select("light", "Light", "get-lights", &[])])
                .live(Duration::from_secs(30), |ic| {
                    Ok(Visual::new().label(match lights::battery(&targets(&ic.inputs)?[0])? {
                        Some(level) => format!("{level:.0}%"),
                        None => "No battery".into(),
                    }))
                }),
        )
        .run()
}
