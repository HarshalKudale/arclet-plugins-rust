//! Weather plugin for KnobDeck — current conditions, forecast and air quality from Open-Meteo
//! (free, no account or API key).

use knobdeck_sdk::input::{self, InputExt};
use knobdeck_sdk::{json, Action, Error, Plugin, Result, Value, Visual};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_secs(600);

fn get(url: &str, query: &[(&str, &str)]) -> Result<Value> {
    let mut req = ureq::get(url).timeout(Duration::from_secs(10));
    for (k, v) in query {
        req = req.query(k, v);
    }
    req.call().map_err(|e| Error::msg(format!("Weather service unavailable ({e})")))?.into_json().map_err(|e| Error::msg(format!("Weather service sent an invalid reply: {e}")))
}

#[derive(Clone)]
struct Place {
    lat: f64,
    lon: f64,
    name: String,
}

static PLACES: Mutex<Option<HashMap<String, Place>>> = Mutex::new(None);

/// "Pune", "Pune, India" or "18.52, 73.86" → coordinates (city lookups are cached).
fn place(text: &str) -> Result<Place> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::msg("Enter a location"));
    }
    if let Some((a, b)) = text.split_once(',') {
        if let (Ok(lat), Ok(lon)) = (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
            return Ok(Place { lat, lon, name: format!("{lat:.2}, {lon:.2}") });
        }
    }
    if let Some(p) = PLACES.lock().unwrap().as_ref().and_then(|m| m.get(text)).cloned() {
        return Ok(p);
    }
    // "City, Country": search the city, prefer a result in that country or region.
    let (city, hint) = text.split_once(',').map_or((text, ""), |(c, h)| (c.trim(), h.trim()));
    let found = get("https://geocoding-api.open-meteo.com/v1/search", &[("name", city), ("count", "10")])?;
    let results = found["results"].as_array().cloned().unwrap_or_default();
    let matches = |r: &Value| ["country", "country_code", "admin1"].iter().any(|k| r[*k].as_str().is_some_and(|v| v.eq_ignore_ascii_case(hint)));
    let r = results.iter().find(|r| !hint.is_empty() && matches(r)).or(results.first()).ok_or_else(|| Error::msg(format!("No place called \"{text}\"")))?;
    let p = Place { lat: r["latitude"].as_f64().unwrap_or(0.0), lon: r["longitude"].as_f64().unwrap_or(0.0), name: r["name"].as_str().unwrap_or(city).to_string() };
    PLACES.lock().unwrap().get_or_insert_with(HashMap::new).insert(text.to_string(), p.clone());
    Ok(p)
}

/// Forecast replies are shared by every key for the same place and units for 10 minutes.
static FORECASTS: Mutex<Option<HashMap<String, (Instant, Value)>>> = Mutex::new(None);

fn forecast(p: &Place, fahrenheit: bool) -> Result<Value> {
    let key = format!("{:.3},{:.3},{fahrenheit}", p.lat, p.lon);
    if let Some((at, v)) = FORECASTS.lock().unwrap().as_ref().and_then(|m| m.get(&key)).cloned() {
        if at.elapsed() < REFRESH {
            return Ok(v);
        }
    }
    let (lat, lon) = (p.lat.to_string(), p.lon.to_string());
    let v = get(
        "https://api.open-meteo.com/v1/forecast",
        &[
            ("latitude", &lat),
            ("longitude", &lon),
            ("current", "temperature_2m,apparent_temperature,weather_code,is_day,relative_humidity_2m,wind_speed_10m"),
            ("daily", "weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max"),
            ("hourly", "temperature_2m,weather_code,is_day"),
            ("forecast_hours", "24"),
            ("forecast_days", "7"),
            ("timezone", "auto"),
            ("temperature_unit", if fahrenheit { "fahrenheit" } else { "celsius" }),
        ],
    )?;
    FORECASTS.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, (Instant::now(), v.clone()));
    Ok(v)
}

/// WMO weather code → (words, Lucide icon).
fn condition(code: i64, day: bool) -> (&'static str, &'static str) {
    match code {
        0 => ("Clear", if day { "sun" } else { "moon" }),
        1 | 2 => ("Partly cloudy", if day { "cloud-sun" } else { "cloud-moon" }),
        3 => ("Cloudy", "cloud"),
        45 | 48 => ("Fog", "cloud-fog"),
        51..=57 => ("Drizzle", "cloud-drizzle"),
        61..=67 | 80..=82 => ("Rain", "cloud-rain"),
        71..=77 | 85 | 86 => ("Snow", "cloud-snow"),
        95..=99 => ("Thunderstorm", "cloud-lightning"),
        _ => ("—", "cloud"),
    }
}

/// "2026-11-03" → "Tue" (days-from-civil; 1970-01-01 was a Thursday).
fn weekday(date: &str) -> &'static str {
    let mut it = date.split('-').filter_map(|s| s.parse::<i64>().ok());
    let (Some(y), Some(m), Some(d)) = (it.next(), it.next(), it.next()) else { return "" };
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][days.rem_euclid(7) as usize]
}

fn deg(v: &Value) -> String {
    v.as_f64().map_or("—".into(), |t| format!("{t:.0}°"))
}

/// US AQI or European AQI → its band's name.
fn aqi_band(v: i64, us: bool) -> &'static str {
    let bands: &[(i64, &str)] = if us {
        &[(50, "Good"), (100, "Moderate"), (150, "Unhealthy for some"), (200, "Unhealthy"), (300, "Very unhealthy"), (i64::MAX, "Hazardous")]
    } else {
        &[(20, "Good"), (40, "Fair"), (60, "Moderate"), (80, "Poor"), (100, "Very poor"), (i64::MAX, "Extremely poor")]
    };
    bands.iter().find(|(top, _)| v <= *top).map_or("", |(_, n)| n)
}

/// Show a problem (unknown place, no network) on the key itself instead of only in the log.
fn on_key(r: Result<Visual>) -> Result<Visual> {
    Ok(r.unwrap_or_else(|e| Visual::new().label(e.message().to_string()).icon("cloud-off")))
}

fn place_inputs() -> Vec<Value> {
    vec![
        input::text("location", "Location").placeholder("Pune, India  or  18.52, 73.86"),
        input::select("units", "Units", &[("c", "Celsius"), ("f", "Fahrenheit")]).with("default", "c"),
    ]
}

fn read(inputs: &Value) -> Result<(Place, Value)> {
    let p = place(inputs["location"].as_str().unwrap_or(""))?;
    let f = forecast(&p, inputs["units"] == "f")?;
    Ok((p, f))
}

fn main() {
    Plugin::new(if cfg!(target_os = "macos") { "weather-macos" } else { "weather-windows" }, "Weather")
        .version(env!("CARGO_PKG_VERSION"))
        .platforms(&[std::env::consts::OS]) // one package per OS
        .author("KnobDeck")
        .description("Current weather, forecast and air quality for any place (Open-Meteo, no account).")
        .icon("cloud-sun")
        .action(
            Action::button("weather", "Weather")
                .icon("cloud-sun")
                .inputs(place_inputs())
                .live(REFRESH, |ic| {
                    on_key(read(&ic.inputs).map(|(p, f)| {
                        let c = &f["current"];
                        let (words, icon) = condition(c["weather_code"].as_i64().unwrap_or(-1), c["is_day"] == 1);
                        Visual::new().label(format!("{} {} · {words}", p.name, deg(&c["temperature_2m"]))).icon(icon)
                    }))
                })
                // Pressing shows today's high/low and feels-like; the next refresh puts the current weather back.
                .on_press(|ctx| {
                    let (_, f) = read(&ctx.inputs)?;
                    let d = &f["daily"];
                    ctx.set_label(format!("H {} L {} · feels {}", deg(&d["temperature_2m_max"][0]), deg(&d["temperature_2m_min"][0]), deg(&f["current"]["apparent_temperature"])));
                    Ok(())
                }),
        )
        .action(Action::wheel("forecast-wheel", "Forecast (7 days)").icon("calendar-days").inputs(place_inputs()).on_open(|ctx| {
            let (_, f) = read(&ctx.inputs).or_else(|e| ctx.fail(e.message().to_string()))?;
            let d = &f["daily"];
            let days = d["time"].as_array().cloned().unwrap_or_default();
            Ok(days
                .iter()
                .enumerate()
                .map(|(i, date)| {
                    let (_, icon) = condition(d["weather_code"][i].as_i64().unwrap_or(-1), true);
                    let rain = d["precipitation_probability_max"][i].as_i64().map_or(String::new(), |r| format!(" · {r}%"));
                    let day = if i == 0 { "Today" } else { weekday(date.as_str().unwrap_or("")) };
                    json!({"id": format!("d{i}"), "label": format!("{day} {}/{}{rain}", deg(&d["temperature_2m_max"][i]), deg(&d["temperature_2m_min"][i])), "icon": icon})
                })
                .collect())
        }).on_child_press(|_| Ok(())))
        .action(Action::wheel("hourly-wheel", "Next 24 hours").icon("clock").inputs(place_inputs()).on_open(|ctx| {
            let (_, f) = read(&ctx.inputs).or_else(|e| ctx.fail(e.message().to_string()))?;
            let h = &f["hourly"];
            let times = h["time"].as_array().cloned().unwrap_or_default();
            // Every 3 hours: 8 slices cover the day.
            Ok(times
                .iter()
                .enumerate()
                .step_by(3)
                .map(|(i, t)| {
                    let (_, icon) = condition(h["weather_code"][i].as_i64().unwrap_or(-1), h["is_day"][i] == 1);
                    let hour = t.as_str().and_then(|s| s.get(11..16)).unwrap_or("");
                    json!({"id": format!("h{i}"), "label": format!("{hour} {}", deg(&h["temperature_2m"][i])), "icon": icon})
                })
                .collect())
        }).on_child_press(|_| Ok(())))
        .action(
            Action::button("air-quality", "Air Quality")
                .icon("wind")
                .inputs(vec![input::text("location", "Location").placeholder("Pune, India  or  18.52, 73.86"), input::select("scale", "Scale", &[("us", "US AQI"), ("eu", "European AQI")]).with("default", "us")])
                .live(Duration::from_secs(1800), |ic| {
                    let us = ic.input_str("scale") != "eu";
                    let field = if us { "us_aqi" } else { "european_aqi" };
                    on_key(place(&ic.input_str("location")).and_then(|p| get("https://air-quality-api.open-meteo.com/v1/air-quality", &[("latitude", &p.lat.to_string()), ("longitude", &p.lon.to_string()), ("current", field)])).map(|v| match v["current"][field].as_i64() {
                        Some(a) => Visual::new().label(format!("AQI {a} · {}", aqi_band(a, us))),
                        None => Visual::new().label("AQI n/a"),
                    }))
                }),
        )
        .run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekdays() {
        assert_eq!(weekday("1970-01-01"), "Thu");
        assert_eq!(weekday("2026-11-03"), "Tue");
        assert_eq!(weekday("2024-02-29"), "Thu");
        assert_eq!(weekday("2000-03-01"), "Wed");
        assert_eq!(weekday("bad"), "");
    }

    #[test]
    fn bands_and_conditions() {
        assert_eq!(aqi_band(42, true), "Good");
        assert_eq!(aqi_band(89, true), "Moderate");
        assert_eq!(aqi_band(51, false), "Moderate");
        assert_eq!(condition(0, false), ("Clear", "moon"));
        assert_eq!(condition(63, true).0, "Rain");
    }

    #[test]
    fn coordinates_need_no_lookup() {
        let p = place(" 18.52 , 73.86 ").unwrap();
        assert_eq!((p.lat, p.lon), (18.52, 73.86));
        assert!(place("").is_err());
    }
}
