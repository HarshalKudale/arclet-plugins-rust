//! Voicemod's local Control API: a websocket at `ws://127.0.0.1:<port>/v1` on one of a fixed list of
//! ports. Requests are `{id, action, payload}`; replies carry `actionId` (our id), `actionType` and
//! `actionObject`. The connection is registered with a client key issued by Voicemod.

use knobdeck_sdk::{json, Error, Result, Value};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

/// Voicemod listens on the first free port of this list.
const PORTS: [u16; 11] = [59129, 20000, 39273, 42152, 43782, 46667, 35679, 37170, 38501, 33952, 30546];
const REPLY_TIMEOUT: Duration = Duration::from_secs(4);

static CONN: Mutex<Option<Socket>> = Mutex::new(None);
static NEXT: AtomicU64 = AtomicU64::new(1);

fn next_id() -> String {
    format!("kd-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

fn ports() -> Vec<u16> {
    // A fixed port (tests, unusual setups) instead of scanning.
    match std::env::var("VOICEMOD_PORT").ok().and_then(|p| p.parse().ok()) {
        Some(p) => vec![p],
        None => PORTS.to_vec(),
    }
}

fn send(ws: &mut Socket, id: &str, action: &str, payload: Value) -> Result<()> {
    ws.send(Message::text(json!({"id": id, "action": action, "payload": payload}).to_string())).map_err(|e| Error::msg(format!("Voicemod connection lost: {e}")))
}

/// Read until the reply to `id` (events and other replies are skipped).
fn reply(ws: &mut Socket, id: &str, action: &str) -> Result<Value> {
    let deadline = Instant::now() + REPLY_TIMEOUT;
    while Instant::now() < deadline {
        let msg = match ws.read() {
            Ok(m) => m,
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(e) => return Err(Error::msg(format!("Voicemod connection lost: {e}"))),
        };
        let Message::Text(t) = msg else { continue };
        let Ok(v) = t.parse::<Value>() else { continue };
        // registerClient answers with its action name and a status; everything else with actionId.
        if v["actionId"] == id || (action == "registerClient" && v["action"] == "registerClient") {
            return Ok(v);
        }
    }
    Err(Error::msg(format!("Voicemod did not answer {action}")))
}

/// Connect on the first port that answers and register with `key`.
pub fn connect(key: &str) -> Result<Socket> {
    let mut last = Error::msg("Voicemod is not running");
    for port in ports() {
        let Ok(stream) = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(300)) else { continue };
        stream.set_read_timeout(Some(Duration::from_millis(500))).ok();
        let mut ws = match tungstenite::client(format!("ws://127.0.0.1:{port}/v1"), MaybeTlsStream::Plain(stream)) {
            Ok((ws, _)) => ws,
            Err(e) => {
                last = Error::msg(format!("Port {port} is not Voicemod ({e})"));
                continue;
            }
        };
        let id = next_id();
        send(&mut ws, &id, "registerClient", json!({"clientKey": key}))?;
        let r = reply(&mut ws, &id, "registerClient")?;
        if r["payload"]["status"]["code"].to_string().trim_matches('"') != "200" {
            return Err(Error::msg("Voicemod refused the client key"));
        }
        return Ok(ws);
    }
    Err(last)
}

fn with_conn<T>(key: &str, f: impl FnOnce(&mut Socket) -> Result<T>) -> Result<T> {
    let mut guard = CONN.lock().unwrap();
    if guard.is_none() {
        *guard = Some(connect(key).map_err(|e| {
            if e.message().contains("client key") {
                knobdeck_sdk::mark_setup_required("Voicemod refused the client key");
            }
            e
        })?);
    }
    let out = f(guard.as_mut().unwrap());
    if out.is_err() {
        *guard = None; // reconnect next time
    }
    out
}

/// An action that answers: returns its `actionObject`.
pub fn ask(key: &str, action: &str, payload: Value) -> Result<Value> {
    with_conn(key, |ws| {
        let id = next_id();
        send(ws, &id, action, payload)?;
        Ok(reply(ws, &id, action)?["actionObject"].clone())
    })
}

/// An action Voicemod does not answer (loadVoice, playMeme…).
pub fn tell(key: &str, action: &str, payload: Value) -> Result<()> {
    with_conn(key, |ws| send(ws, &next_id(), action, payload))
}
