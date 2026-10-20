//! Minimal obs-websocket v5 client: one connection thread owns the socket
//! (read with a short timeout, drain an outgoing queue between reads), callers
//! block on a per-request channel. Reconnects with the same back-off as the JS plugin.

use arclet_sdk::{json, log, Error, Result, Value};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;
use std::time::Duration;
use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket};

type Reply = std::result::Result<Value, String>;
type Socket = WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>;

const RECONNECT_DELAYS_MS: [u64; 6] = [1000, 2000, 4000, 8000, 16000, 30000];
/// EventSubscription.All (General|Config|Scenes|Inputs|Transitions|Filters|Outputs|SceneItems|MediaInputs|Vendors|Ui).
const EVENT_SUBSCRIPTIONS: u32 = 2047;

static CONNECTED: AtomicBool = AtomicBool::new(false);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static OUTBOX: Mutex<Option<Sender<String>>> = Mutex::new(None);
static PENDING: Mutex<Option<HashMap<String, Sender<Reply>>>> = Mutex::new(None);

pub fn connected() -> bool {
    CONNECTED.load(Ordering::SeqCst)
}

/// Send a request and wait for its response data.
pub fn call(request_type: &str, data: Value) -> Result<Value> {
    let outbox = OUTBOX.lock().unwrap().clone().ok_or_else(|| Error::msg("OBS is not connected"))?;
    let id = format!("req-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let (tx, rx) = channel();
    PENDING.lock().unwrap().get_or_insert_with(HashMap::new).insert(id.clone(), tx);
    let msg = json!({"op": 6, "d": {"requestType": request_type, "requestId": id, "requestData": data}});
    if outbox.send(msg.to_string()).is_err() {
        PENDING.lock().unwrap().as_mut().map(|p| p.remove(&id));
        return Err(Error::msg("OBS is not connected"));
    }
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(Error::Other(e)),
        Err(_) => {
            PENDING.lock().unwrap().as_mut().map(|p| p.remove(&id));
            Err(Error::msg(format!("OBS request {request_type} timed out")))
        }
    }
}

fn b64_sha256(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(Sha256::digest(data))
}

fn open(host: &str, port: &str, password: &str) -> std::result::Result<Socket, String> {
    let addr = format!("{host}:{port}").to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("no address")?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|e| e.to_string())?;
    let mut req = format!("ws://{host}:{port}").into_client_request().map_err(|e| e.to_string())?;
    req.headers_mut().insert("Sec-WebSocket-Protocol", "obswebsocket.json".parse().unwrap());
    let (mut ws, _) = tungstenite::client(req, tungstenite::stream::MaybeTlsStream::Plain(stream)).map_err(|e| e.to_string())?;

    // Handshake is still blocking here: Hello -> Identify -> Identified.
    let hello: Value = match ws.read().map_err(|e| e.to_string())? {
        Message::Text(t) => serde_json_from(&t)?,
        _ => return Err("unexpected OBS hello".into()),
    };
    let mut identify = json!({"rpcVersion": 1, "eventSubscriptions": EVENT_SUBSCRIPTIONS});
    if let Some(auth) = hello["d"].get("authentication").filter(|a| a.is_object()) {
        if password.is_empty() {
            return Err("OBS requires a password".into());
        }
        let secret = b64_sha256(format!("{password}{}", auth["salt"].as_str().unwrap_or("")).as_bytes());
        identify["authentication"] = b64_sha256(format!("{secret}{}", auth["challenge"].as_str().unwrap_or("")).as_bytes()).into();
    }
    ws.send(Message::text(json!({"op": 1, "d": identify}).to_string())).map_err(|e| e.to_string())?;
    match ws.read().map_err(|e| e.to_string())? {
        Message::Text(t) if serde_json_from(&t)?["op"] == 2 => {}
        Message::Close(_) => return Err("OBS rejected the connection (check the password)".into()),
        other => return Err(format!("unexpected OBS reply: {other:?}")),
    }
    if let tungstenite::stream::MaybeTlsStream::Plain(s) = ws.get_mut() {
        let _ = s.set_read_timeout(Some(Duration::from_millis(25)));
    }
    Ok(ws)
}

fn serde_json_from(t: &str) -> std::result::Result<Value, String> {
    t.parse::<Value>().map_err(|e| e.to_string())
}

/// Run forever: connect, pump, and on close report it and reconnect with back-off.
/// `on_connected` runs on a fresh thread (it makes blocking `call`s); `on_event`
/// runs on the pump thread and must not block on `call`.
pub fn run(host: String, port: String, password: String, on_connected: fn(), on_event: fn(&str, &Value), on_closed: fn()) {
    let mut attempt = 0usize;
    loop {
        log(format!("OBS: connecting to ws://{host}:{port}"));
        match open(&host, &port, &password) {
            Ok(mut ws) => {
                attempt = 0;
                let (tx, rx) = channel::<String>();
                *OUTBOX.lock().unwrap() = Some(tx);
                CONNECTED.store(true, Ordering::SeqCst);
                log("OBS: connected");
                std::thread::spawn(on_connected);
                pump(&mut ws, &rx, on_event);
                CONNECTED.store(false, Ordering::SeqCst);
                *OUTBOX.lock().unwrap() = None;
                for (_, tx) in PENDING.lock().unwrap().get_or_insert_with(HashMap::new).drain() {
                    let _ = tx.send(Err("OBS disconnected".into()));
                }
                log("OBS: disconnected");
                on_closed();
            }
            Err(e) if e.contains("password") => {
                // Retrying can't fix a bad password; the host stops us and asks for setup.
                log(format!("OBS: {e}"));
                arclet_sdk::mark_setup_required(e);
                return;
            }
            Err(e) => log(format!("OBS: connect error: {e}")),
        }
        let delay = RECONNECT_DELAYS_MS[attempt.min(RECONNECT_DELAYS_MS.len() - 1)];
        attempt += 1;
        log(format!("OBS: reconnect in {delay}ms"));
        std::thread::sleep(Duration::from_millis(delay));
    }
}

fn pump(ws: &mut Socket, rx: &std::sync::mpsc::Receiver<String>, on_event: fn(&str, &Value)) {
    loop {
        while let Ok(out) = rx.try_recv() {
            if ws.send(Message::text(out)).is_err() {
                return;
            }
        }
        match ws.read() {
            Ok(Message::Text(t)) => {
                let Ok(msg) = serde_json_from(&t) else { continue };
                match msg["op"].as_u64() {
                    Some(5) => on_event(msg["d"]["eventType"].as_str().unwrap_or(""), &msg["d"]["eventData"]),
                    Some(7) => {
                        let d = &msg["d"];
                        let Some(id) = d["requestId"].as_str() else { continue };
                        let tx = PENDING.lock().unwrap().as_mut().and_then(|p| p.remove(id));
                        if let Some(tx) = tx {
                            let _ = tx.send(if d["requestStatus"]["result"] == true {
                                Ok(d.get("responseData").cloned().unwrap_or(json!({})))
                            } else {
                                Err(d["requestStatus"]["comment"].as_str().unwrap_or("OBS request failed").to_string())
                            });
                        }
                    }
                    _ => {}
                }
            }
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;
    use tungstenite::handshake::server::{Request, Response};

    static EVENTS: AtomicUsize = AtomicUsize::new(0);
    fn noop() {}
    fn count_event(name: &str, _: &Value) {
        if name == "StreamStateChanged" {
            EVENTS.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Mock obs-websocket v5 server with the auth hash cross-checked against obs-websocket-js (sha256(sha256(pw+salt)+challenge)).
    #[test]
    fn auth_request_response_and_events() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port().to_string();
        std::thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept_hdr(sock, |_: &Request, mut r: Response| {
                r.headers_mut().insert("Sec-WebSocket-Protocol", "obswebsocket.json".parse().unwrap());
                Ok(r)
            })
            .unwrap();
            ws.send(Message::text(json!({"op": 0, "d": {"obsWebSocketVersion": "5.1.0", "rpcVersion": 1, "authentication": {
                "challenge": "+IxH4CnCiqpX1rZtV1uvLhV9BJLvIBpl7S2Rq0m3OeI=", "salt": "lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI="}}}).to_string())).unwrap();
            let Message::Text(t) = ws.read().unwrap() else { panic!() };
            let identify: Value = t.parse().unwrap();
            assert_eq!(identify["d"]["authentication"], "PpBcQRYZ293hkbo603BhA2OZpNewCjFc5mRBoruOb2Q=");
            ws.send(Message::text(json!({"op": 2, "d": {"negotiatedRpcVersion": 1}}).to_string())).unwrap();
            loop {
                let Ok(Message::Text(t)) = ws.read() else { return };
                let m: Value = t.parse().unwrap();
                let d = &m["d"];
                let (status, data) = match d["requestType"].as_str().unwrap() {
                    "GetRecordStatus" => (json!({"result": true, "code": 100}), json!({"outputActive": true})),
                    _ => (json!({"result": false, "code": 600, "comment": "No such thing"}), json!({})),
                };
                ws.send(Message::text(json!({"op": 7, "d": {"requestType": d["requestType"], "requestId": d["requestId"], "requestStatus": status, "responseData": data}}).to_string())).unwrap();
                ws.send(Message::text(json!({"op": 5, "d": {"eventType": "StreamStateChanged", "eventData": {"outputActive": true}}}).to_string())).unwrap();
            }
        });

        std::thread::spawn(move || run("127.0.0.1".into(), port, "supersecretpassword".into(), noop, count_event, noop));
        for _ in 0..100 {
            if connected() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(connected(), "never connected");
        assert_eq!(call("GetRecordStatus", json!({})).unwrap()["outputActive"], true);
        assert_eq!(call("Nope", json!({})).unwrap_err().to_string(), "No such thing");
        std::thread::sleep(Duration::from_millis(100));
        assert!(EVENTS.load(Ordering::SeqCst) >= 1);
    }
}
