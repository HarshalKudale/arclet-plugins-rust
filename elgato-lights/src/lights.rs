//! Elgato Key Light / Ring Light / Light Strip local API (`http://<light>:9123/elgato/…`) and
//! discovery over mDNS (`_elg._tcp`). Shared by the plugin and its setup program.

use knobdeck_sdk::{json, Error, Result, Value};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

pub const PORT: u16 = 9123;

/// `_elg._tcp.local` as DNS labels.
const SERVICE: &[u8] = b"\x04_elg\x04_tcp\x05local\x00";

/// Lights answering an mDNS query within `wait`. A query sent from a port other than 5353 is
/// answered by unicast to that port (RFC 6762 §6.7), so the reply's source address is the light.
pub fn discover(wait: Duration) -> Vec<String> {
    let Ok(sock) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else { return vec![] };
    // Header: id 0, flags 0, one question; question: PTR (12), class IN with the unicast-response bit.
    let mut q = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    q.extend_from_slice(SERVICE);
    q.extend_from_slice(&[0, 12, 0x80, 1]);
    if sock.send_to(&q, (Ipv4Addr::new(224, 0, 0, 251), 5353)).is_err() {
        return vec![];
    }
    let mut found = vec![];
    let end = Instant::now() + wait;
    let mut buf = [0u8; 1500];
    while let Some(left) = end.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) {
        let _ = sock.set_read_timeout(Some(left));
        let Ok((n, SocketAddr::V4(from))) = sock.recv_from(&mut buf) else { break };
        // Only replies that name the Elgato service (anything else on the network is ignored).
        let ip = from.ip().to_string();
        if buf[..n].windows(SERVICE.len()).any(|w| w == SERVICE) && !found.contains(&ip) {
            found.push(ip);
        }
    }
    found
}

fn url(addr: &str, path: &str) -> String {
    // An address may carry its own port ("192.168.1.20:9123").
    if addr.contains(':') {
        format!("http://{addr}/elgato/{path}")
    } else {
        format!("http://{addr}:{PORT}/elgato/{path}")
    }
}

fn get(addr: &str, path: &str) -> Result<Value> {
    ureq::get(&url(addr, path)).timeout(Duration::from_secs(3)).call().map_err(|e| unreachable(addr, e))?.into_json().map_err(|e| Error::msg(format!("{addr} sent an invalid reply: {e}")))
}

fn unreachable(addr: &str, e: ureq::Error) -> Error {
    Error::msg(format!("Light {addr} did not answer ({e})"))
}

/// The light's name as set in Control Center (falls back to its product name).
pub fn name(addr: &str) -> Result<String> {
    let info = get(addr, "accessory-info")?;
    Ok(info["displayName"].as_str().filter(|s| !s.is_empty()).or(info["productName"].as_str()).unwrap_or(addr).to_string())
}

/// Current state of the light: `{"on": 0|1, "brightness": 0-100, "temperature": 143-344}`
/// (Light Strip: `hue` 0-360 and `saturation` 0-100 instead of temperature).
pub fn state(addr: &str) -> Result<Value> {
    Ok(get(addr, "lights")?["lights"][0].clone())
}

/// Change some fields of the light (`{"on": 1}`, `{"brightness": 40}`…); returns the new state.
pub fn set(addr: &str, fields: Value) -> Result<Value> {
    let body = json!({"numberOfLights": 1, "lights": [fields]});
    let reply: Value = ureq::put(&url(addr, "lights")).timeout(Duration::from_secs(3)).send_json(body).map_err(|e| unreachable(addr, e))?.into_json().map_err(|e| Error::msg(format!("{addr} sent an invalid reply: {e}")))?;
    Ok(reply["lights"][0].clone())
}

/// Battery percent of a Key Light Mini (`None` for lights without a battery).
pub fn battery(addr: &str) -> Result<Option<f64>> {
    match ureq::get(&url(addr, "battery-info")).timeout(Duration::from_secs(3)).call() {
        Ok(r) => Ok(r.into_json::<Value>().ok().and_then(|v| v["level"].as_f64())),
        Err(ureq::Error::Status(_, _)) => Ok(None),
        Err(e) => Err(unreachable(addr, e)),
    }
}

/// The API's colour temperature is in mireds (143–344); people think in Kelvin (7000–2900 K).
pub fn kelvin_to_api(k: f64) -> i64 {
    (1_000_000.0 / k.clamp(2900.0, 7000.0)).round().clamp(143.0, 344.0) as i64
}

pub fn api_to_kelvin(v: i64) -> i64 {
    // Round to 50 K, as Control Center shows it.
    ((1_000_000.0 / v.clamp(143, 344) as f64) / 50.0).round() as i64 * 50
}

/// `#RRGGBB` → (hue 0-360, saturation 0-100, brightness 0-100) for a Light Strip.
pub fn hex_to_hsb(hex: &str) -> Option<(f64, f64, f64)> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|v| v as f64 / 255.0);
    let (r, g, b) = (c(0)?, c(2)?, c(4)?);
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    let hue = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let sat = if max == 0.0 { 0.0 } else { d / max * 100.0 };
    Some((hue.round(), sat.round(), (max * 100.0).round()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temperature_units() {
        assert_eq!(kelvin_to_api(7000.0), 143);
        assert_eq!(kelvin_to_api(2900.0), 344); // 344.8 rounds past the API's top value
        assert_eq!(kelvin_to_api(99999.0), 143);
        assert_eq!(api_to_kelvin(143), 7000);
        assert_eq!(api_to_kelvin(344), 2900);
        assert_eq!(api_to_kelvin(213), 4700);
        assert_eq!(api_to_kelvin(kelvin_to_api(5000.0)), 5000);
    }

    #[test]
    fn colors() {
        assert_eq!(hex_to_hsb("#FF0000"), Some((0.0, 100.0, 100.0)));
        assert_eq!(hex_to_hsb("00ff00"), Some((120.0, 100.0, 100.0)));
        assert_eq!(hex_to_hsb("#0000FF"), Some((240.0, 100.0, 100.0)));
        assert_eq!(hex_to_hsb("#808080"), Some((0.0, 0.0, 50.0)));
        assert_eq!(hex_to_hsb("#12"), None);
        assert_eq!(hex_to_hsb("#GG0000"), None);
    }

    #[test]
    fn urls() {
        assert_eq!(url("10.0.0.5", "lights"), "http://10.0.0.5:9123/elgato/lights");
        assert_eq!(url("127.0.0.1:9200", "lights"), "http://127.0.0.1:9200/elgato/lights");
    }
}
