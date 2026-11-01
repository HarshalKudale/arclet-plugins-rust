//! Shared in-memory Discord voice state, updated by RPC event handlers and read
//! by action callbacks.

use knobdeck_sdk::Value;
use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex, MutexGuard};

pub struct State {
    pub muted: bool,
    pub deafened: bool,
    pub voice_mode: String,
    pub current_channel_id: Option<String>,
    pub current_guild_id: Option<String>,
    pub video_enabled: bool,
    pub stream_enabled: bool,
    pub input_volume: f64,
    pub output_volume: f64,
    pub input_devices: Vec<Value>,
    pub output_devices: Vec<Value>,
    /// userId → (volume, mute)
    pub user_volumes: BTreeMap<String, (f64, bool)>,
    pub notification_count: usize,
    /// (channel_id, guild_id), newest first, max 50
    pub recent_notifications: Vec<(Option<String>, Option<String>)>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
    Mutex::new(State {
        muted: false,
        deafened: false,
        voice_mode: "VOICE_ACTIVITY".into(),
        current_channel_id: None,
        current_guild_id: None,
        video_enabled: false,
        stream_enabled: false,
        input_volume: 100.0,
        output_volume: 100.0,
        input_devices: vec![],
        output_devices: vec![],
        user_volumes: BTreeMap::new(),
        notification_count: 0,
        recent_notifications: vec![],
    })
});

pub fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap()
}

fn nonempty(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(String::from)
}

impl State {
    /// Accepts both `GET_VOICE_SETTINGS` (snake_case) and `VOICE_SETTINGS_UPDATE` payloads.
    pub fn apply_voice_settings(&mut self, d: &Value) {
        if let Some(b) = d["mute"].as_bool() {
            self.muted = b;
        }
        if let Some(b) = d["deaf"].as_bool() {
            self.deafened = b;
        }
        if let Some(t) = nonempty(&d["mode"]["type"]) {
            self.voice_mode = t;
        }
        if let Some(v) = d["input"]["volume"].as_f64() {
            self.input_volume = v;
        }
        if let Some(v) = d["output"]["volume"].as_f64() {
            self.output_volume = v;
        }
        let devices = |side: &Value| {
            side.get("availableDevices").filter(|v| !v.is_null()).or_else(|| side.get("available_devices")).and_then(Value::as_array).cloned()
        };
        if let Some(v) = devices(&d["input"]) {
            self.input_devices = v;
        }
        if let Some(v) = devices(&d["output"]) {
            self.output_devices = v;
        }
    }

    pub fn apply_channel_select(&mut self, d: &Value) {
        self.current_channel_id = nonempty(&d["channel_id"]);
        self.current_guild_id = nonempty(&d["guild_id"]);
    }

    pub fn add_notification(&mut self, d: &Value) {
        self.notification_count += 1;
        self.recent_notifications.insert(0, (nonempty(&d["channel_id"]), nonempty(&d["guild_id"])));
        self.recent_notifications.truncate(50);
    }

    pub fn clear_notifications(&mut self) {
        self.notification_count = 0;
        self.recent_notifications.clear();
    }

    pub fn user_volume(&self, id: &str) -> (f64, bool) {
        self.user_volumes.get(id).copied().unwrap_or((100.0, false))
    }
}
