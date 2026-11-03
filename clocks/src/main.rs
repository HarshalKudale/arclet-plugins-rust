//! Clocks plugin for KnobDeck — a clock for any time zone, a world-clock wheel, a countdown timer
//! and a stopwatch. Time zones come from the OS database (macOS) or a bundled one (Windows).

use knobdeck_sdk::input::{self, InputExt};
use knobdeck_sdk::{json, Action, Ctx, Error, Plugin, Result, Value, Visual};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const LOCAL: &str = "__local__";
const TICK: Duration = Duration::from_secs(1);

fn now_in(zone: &str) -> Result<jiff::Zoned> {
    let now = jiff::Zoned::now();
    if zone.is_empty() || zone == LOCAL {
        return Ok(now);
    }
    now.in_tz(zone).map_err(|_| Error::msg(format!("Unknown time zone \"{zone}\"")))
}

/// "21:05" or "9:05 PM", optionally with "Tue 3 Nov".
fn clock_text(t: &jiff::Zoned, twelve_hour: bool, date: bool) -> String {
    let time = if twelve_hour { t.strftime("%-I:%M %p").to_string() } else { t.strftime("%H:%M").to_string() };
    if date {
        format!("{time} · {}", t.strftime("%a %-d %b"))
    } else {
        time
    }
}

/// The city part of an IANA name: "America/New_York" → "New York".
fn city(zone: &str) -> String {
    zone.rsplit('/').next().unwrap_or(zone).replace('_', " ")
}

fn zone_input() -> Value {
    input::query_combobox("zone", "Time zone", "get-zones").with("default", LOCAL)
}

fn format_inputs() -> Vec<Value> {
    vec![input::select("format", "Format", &[("24", "24-hour"), ("12", "12-hour")]).with("default", "24"), input::checkbox("date", "Show the date")]
}

fn fmt(t: &jiff::Zoned, inputs: &Value) -> String {
    // Checkbox values arrive as a bool, or as "true" from older hosts.
    let date = inputs["date"] == true || inputs["date"] == "true";
    clock_text(t, inputs["format"] == "12", date)
}

/// mm:ss, or h:mm:ss from an hour up.
fn duration_text(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// "5m", "90s", "1h30m", "10:00", "1:30:00", or a bare number of minutes.
fn parse_duration(text: &str) -> Option<Duration> {
    let t = text.trim().to_ascii_lowercase();
    if t.is_empty() {
        return None;
    }
    if t.contains(':') {
        let parts: Vec<u64> = t.split(':').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
        let secs = parts.iter().fold(0, |acc, p| acc * 60 + p);
        return (parts.len() <= 3 && secs > 0).then(|| Duration::from_secs(secs));
    }
    if let Ok(mins) = t.parse::<f64>() {
        return (mins > 0.0).then(|| Duration::from_secs_f64(mins * 60.0));
    }
    let (mut secs, mut num) = (0u64, String::new());
    for c in t.chars().filter(|c| !c.is_whitespace()) {
        match c {
            '0'..='9' => num.push(c),
            'h' | 'm' | 's' => {
                let n: u64 = num.parse().ok()?;
                secs += n * match c {
                    'h' => 3600,
                    'm' => 60,
                    _ => 1,
                };
                num.clear();
            }
            _ => return None,
        }
    }
    (num.is_empty() && secs > 0).then(|| Duration::from_secs(secs))
}

/// A running or paused countdown / stopwatch, per placed key.
#[derive(Clone, Copy, Default)]
struct Watch {
    /// Time counted before the current run.
    banked: Duration,
    /// When the current run started (None = paused / not started).
    since: Option<Instant>,
}

impl Watch {
    fn elapsed(&self) -> Duration {
        self.banked + self.since.map_or(Duration::ZERO, |s| s.elapsed())
    }
    fn running(&self) -> bool {
        self.since.is_some()
    }
    fn toggle(&mut self) {
        match self.since.take() {
            Some(s) => self.banked += s.elapsed(),
            None => self.since = Some(Instant::now()),
        }
    }
}

static WATCHES: Mutex<Option<HashMap<String, Watch>>> = Mutex::new(None);

fn watch(iid: &str) -> Watch {
    WATCHES.lock().unwrap().as_ref().and_then(|m| m.get(iid)).copied().unwrap_or_default()
}

fn set_watch(iid: &str, w: Watch) {
    WATCHES.lock().unwrap().get_or_insert_with(HashMap::new).insert(iid.into(), w);
}

fn timer_length(inputs: &Value) -> Result<Duration> {
    parse_duration(inputs["length"].as_str().unwrap_or("")).ok_or_else(|| Error::msg("Set a length like 5m, 90s or 1:30:00"))
}

/// Whether a countdown has run out.
fn timer_done(iid: &str, inputs: &Value) -> Result<bool> {
    Ok(watch(iid).elapsed() >= timer_length(inputs)?)
}

/// Countdown key face: remaining time, or "Time's up" when it ran out.
fn timer_visual(iid: &str, inputs: &Value) -> Result<Visual> {
    let total = timer_length(inputs)?;
    let w = watch(iid);
    let left = total.saturating_sub(w.elapsed());
    Ok(if left.is_zero() {
        Visual::new().label("Time's up").icon("bell-ring")
    } else if w.running() {
        // Round up so it shows 00:01 during its last second, not 00:00.
        Visual::new().label(duration_text(left + Duration::from_millis(999))).icon("timer")
    } else if w.elapsed().is_zero() {
        Visual::new().label(duration_text(total)).icon("timer")
    } else {
        Visual::new().label(format!("{} ⏸", duration_text(left + Duration::from_millis(999)))).icon("timer-off")
    })
}

fn stopwatch_visual(iid: &str) -> Visual {
    let w = watch(iid);
    let text = duration_text(w.elapsed());
    match (w.running(), w.elapsed().is_zero()) {
        (true, _) => Visual::new().label(text).icon("timer"),
        (false, true) => Visual::new().label("00:00").icon("timer-reset"),
        (false, false) => Visual::new().label(format!("{text} ⏸")).icon("timer-off"),
    }
}

fn show(ctx: &Ctx, v: Visual) {
    ctx.set_visual(v);
}

fn main() {
    Plugin::new(if cfg!(target_os = "macos") { "clocks-macos" } else { "clocks-windows" }, "Clocks")
        .version(env!("CARGO_PKG_VERSION"))
        .platforms(&[std::env::consts::OS]) // one package per OS
        .author("KnobDeck")
        .description("Clocks for any time zone, a world clock wheel, a countdown timer and a stopwatch.")
        .icon("clock")
        .query("get-zones", |_| {
            let mut zones: Vec<String> = jiff::tz::db().available().map(|n| n.to_string()).filter(|n| n.contains('/') && !n.starts_with("Etc/")).collect();
            zones.sort();
            let mut opts = vec![json!({"value": LOCAL, "label": "This computer's time zone"})];
            opts.extend(zones.into_iter().map(|z| json!({"value": z, "label": z.replace('_', " ")})));
            Ok(Value::Array(opts))
        })
        .action(
            Action::button("clock", "Clock")
                .icon("clock")
                .inputs([vec![zone_input(), input::text("name", "Name on the key (optional)").placeholder("Tokyo")], format_inputs()].concat())
                // Pushed only when the text changes, so once a minute.
                .live(TICK, |ic| {
                    let zone = ic.input_str("zone");
                    let t = match now_in(&zone) {
                        Ok(t) => t,
                        Err(e) => return Ok(Visual::new().label(e.message().to_string())),
                    };
                    let name = ic.input_str("name");
                    let name = if !name.is_empty() { name } else if zone.is_empty() || zone == LOCAL { String::new() } else { city(&zone) };
                    let text = fmt(&t, &ic.inputs);
                    Ok(Visual::new().label(if name.is_empty() { text } else { format!("{name} {text}") }))
                }),
        )
        .action(
            Action::wheel("world-clock-wheel", "World Clock")
                .icon("globe")
                .inputs([vec![input::textarea("zones", "Time zones, one per line").placeholder("America/New_York\nEurope/London\nAsia/Tokyo")], format_inputs()].concat())
                .on_open(|ctx| {
                    let zones: Vec<String> = ctx.input_str("zones").lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
                    if zones.is_empty() {
                        return ctx.fail("Add some time zones, one per line (e.g. Asia/Tokyo)");
                    }
                    let local_day = jiff::Zoned::now().date();
                    let mut slices = vec![];
                    for (i, z) in zones.iter().take(16).enumerate() {
                        let t = now_in(z).or_else(|e| ctx.fail(e.message().to_string()))?;
                        // Mark another day relative to here: "+1" tomorrow, "-1" yesterday.
                        let day = match t.date().cmp(&local_day) {
                            std::cmp::Ordering::Greater => " +1",
                            std::cmp::Ordering::Less => " -1",
                            _ => "",
                        };
                        let night = !(6..18).contains(&t.hour());
                        slices.push(json!({"id": format!("z{i}"), "label": format!("{} {}{day}", city(z), fmt(&t, &ctx.inputs)), "icon": if night { "moon" } else { "sun" }}));
                    }
                    Ok(slices)
                })
                .on_child_press(|_| Ok(())),
        )
        .action(
            Action::button("timer", "Countdown Timer")
                .icon("timer")
                .inputs(vec![input::text("length", "Length").placeholder("5m, 90s, 1h30m or 10:00").default_value("5m")])
                .live(TICK, |ic| timer_visual(&ic.instance_id, &ic.inputs).or_else(|e| Ok(Visual::new().label(e.message().to_string()))))
                // Press: start / pause; once it has run out, press resets it.
                .on_press(|ctx| {
                    timer_length(&ctx.inputs)?;
                    let mut w = watch(&ctx.instance_id);
                    if timer_done(&ctx.instance_id, &ctx.inputs)? {
                        w = Watch::default();
                    } else {
                        w.toggle();
                    }
                    set_watch(&ctx.instance_id, w);
                    show(ctx, timer_visual(&ctx.instance_id, &ctx.inputs)?);
                    Ok(())
                }),
        )
        .action(
            Action::button("stopwatch", "Stopwatch")
                .icon("timer-reset")
                .live(TICK, |ic| Ok(stopwatch_visual(&ic.instance_id)))
                // Press: start → stop → reset.
                .on_press(|ctx| {
                    let mut w = watch(&ctx.instance_id);
                    if !w.running() && !w.elapsed().is_zero() {
                        w = Watch::default();
                    } else {
                        w.toggle();
                    }
                    set_watch(&ctx.instance_id, w);
                    show(ctx, stopwatch_visual(&ctx.instance_id));
                    Ok(())
                }),
        )
        .run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_duration("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("1h30m"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_duration("1h 30m"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_duration("10:00"), Some(Duration::from_secs(600)));
        assert_eq!(parse_duration("1:30:00"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_duration("2.5"), Some(Duration::from_secs(150)));
        for bad in ["", "0", "abc", "5x", "1:2:3:4", "5m3", "0:00"] {
            assert_eq!(parse_duration(bad), None, "{bad}");
        }
        assert_eq!(duration_text(Duration::from_secs(65)), "01:05");
        assert_eq!(duration_text(Duration::from_secs(3725)), "1:02:05");
    }

    #[test]
    fn zones_and_formats() {
        let t = jiff::civil::date(2026, 11, 3).at(21, 5, 0, 0).in_tz("Asia/Kolkata").unwrap();
        assert_eq!(clock_text(&t, false, false), "21:05");
        assert_eq!(clock_text(&t, true, true), "9:05 PM · Tue 3 Nov");
        assert_eq!(city("America/New_York"), "New York");
        assert!(now_in("Not/AZone").is_err());
        // DST: New York is UTC-4 in July and UTC-5 in January.
        let ny = |m| jiff::civil::date(2026, m, 1).at(12, 0, 0, 0).in_tz("America/New_York").unwrap().offset().seconds() / 3600;
        assert_eq!((ny(7), ny(1)), (-4, -5));
    }

    #[test]
    fn watch_pauses_and_resumes() {
        let mut w = Watch::default();
        assert!(!w.running() && w.elapsed().is_zero());
        w.toggle();
        std::thread::sleep(Duration::from_millis(30));
        w.toggle();
        let paused = w.elapsed();
        assert!(paused >= Duration::from_millis(30) && !w.running());
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(w.elapsed(), paused, "a paused watch does not count");
    }
}
