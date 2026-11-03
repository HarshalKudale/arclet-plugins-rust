//! Streamlabs Desktop's local API: JSON-RPC 2.0 over the SockJS server's raw websocket
//! (`ws://127.0.0.1:59650/api/websocket`), authorised with the token from Settings ▸ Remote Control.
//! One connection is kept and reopened after any error.

use knobdeck_sdk::{json, Error, Result, Value};
use std::net::TcpStream;
use std::sync::Mutex;
use std::time::Duration;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

static CONN: Mutex<Option<(Socket, u64)>> = Mutex::new(None);

pub struct Config {
    pub host: String,
    pub port: String,
    pub token: String,
}

pub fn open(c: &Config) -> Result<Socket> {
    let addr = format!("{}:{}", c.host, c.port);
    let stream = TcpStream::connect_timeout(&addr.parse().map_err(|_| Error::msg(format!("Bad address {addr}")))?, Duration::from_secs(3))
        .map_err(|_| Error::msg("Streamlabs Desktop is not running (or Remote Control is off)"))?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let (ws, _) = tungstenite::client(format!("ws://{addr}/api/websocket"), MaybeTlsStream::Plain(stream)).map_err(|e| Error::msg(format!("Streamlabs refused the connection: {e}")))?;
    Ok(ws)
}

pub fn exchange(ws: &mut Socket, id: u64, resource: &str, method: &str, args: Value) -> Result<Value> {
    let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": {"resource": resource, "args": args}});
    ws.send(Message::text(req.to_string())).map_err(|e| Error::msg(format!("Streamlabs connection lost: {e}")))?;
    loop {
        let msg = ws.read().map_err(|e| Error::msg(format!("Streamlabs connection lost: {e}")))?;
        let Message::Text(t) = msg else { continue };
        let Ok(v) = t.parse::<Value>() else { continue };
        if v["id"].as_u64() != Some(id) {
            continue; // an event or a stale reply
        }
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
            return Err(Error::msg(e["message"].as_str().unwrap_or("Streamlabs rejected the request").to_string()));
        }
        return Ok(v["result"].clone());
    }
}

/// Call `resource.method(args…)`, connecting and authenticating first if needed.
pub fn call(c: &Config, resource: &str, method: &str, args: Value) -> Result<Value> {
    let mut guard = CONN.lock().unwrap();
    if guard.is_none() {
        let mut ws = open(c)?;
        exchange(&mut ws, 1, "TcpServerService", "auth", json!([c.token])).map_err(|e| {
            knobdeck_sdk::mark_setup_required("Streamlabs rejected the token");
            Error::msg(format!("Streamlabs rejected the token ({}). Copy it again from Settings ▸ Remote Control.", e.message()))
        })?;
        *guard = Some((ws, 1));
    }
    let (ws, next) = guard.as_mut().unwrap();
    *next += 1;
    let id = *next;
    let out = exchange(ws, id, resource, method, args);
    if out.is_err() {
        *guard = None; // reconnect next time
    }
    out
}
