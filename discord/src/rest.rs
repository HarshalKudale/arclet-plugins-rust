//! Discord REST helpers (soundboard). Bearer token from the stored RPC token; 30 s cache.

use crate::common::load_token;
use arclet_sdk::{Error, Result, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const API_BASE: &str = "https://discord.com/api/v10";
const CACHE_TTL: Duration = Duration::from_secs(30);

static CACHE: Mutex<Option<HashMap<String, (Value, Instant)>>> = Mutex::new(None);

fn get(endpoint: &str, no_auth: bool) -> Result<Value> {
    if let Some((data, at)) = CACHE.lock().unwrap().as_ref().and_then(|c| c.get(endpoint)) {
        if at.elapsed() < CACHE_TTL {
            return Ok(data.clone());
        }
    }
    let token = if no_auth {
        None
    } else {
        Some(load_token().ok_or_else(|| Error::msg("Discord setup is required before REST queries can run."))?)
    };
    let parse = |r: ureq::Response| r.into_json::<Value>().map_err(|e| Error::msg(format!("REST parse error: {e}")));
    let data = loop {
        let mut req = ureq::get(&format!("{API_BASE}{endpoint}")).set("User-Agent", "ArcletDiscordPlugin/1.0").timeout(Duration::from_secs(10));
        if let Some(t) = &token {
            req = req.set("Authorization", &format!("Bearer {t}"));
        }
        // Like the JS client, error statuses are parsed as data (Discord error bodies are JSON).
        match req.call() {
            Ok(r) => break parse(r)?,
            Err(ureq::Error::Status(429, r)) => {
                let secs: f64 = r.header("retry-after").and_then(|v| v.parse().ok()).unwrap_or(1.0);
                std::thread::sleep(Duration::from_secs_f64(secs));
            }
            Err(ureq::Error::Status(_, r)) => break parse(r)?,
            Err(e) => return Err(Error::msg(e.to_string())),
        }
    };
    CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(endpoint.into(), (data.clone(), Instant::now()));
    Ok(data)
}

/// Sounds for a guild; any failure yields an empty list.
pub fn soundboard(guild_id: &str) -> Vec<Value> {
    match get(&format!("/guilds/{guild_id}/soundboard-sounds"), false) {
        Ok(Value::Array(a)) => a,
        Ok(v) => v["items"].as_array().cloned().unwrap_or_default(),
        Err(_) => vec![],
    }
}

/// Default sounds available to every user.
pub fn default_sounds() -> Vec<Value> {
    match get("/soundboard-default-sounds", true) {
        Ok(Value::Array(a)) => a,
        _ => vec![],
    }
}
