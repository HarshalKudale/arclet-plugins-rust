//! Discord desktop RPC over local IPC (named pipe `discord-ipc-N` / unix socket).
//! Frames: `u32 LE opcode, u32 LE length, JSON`. A port of the parts of
//! `discord-rpc` the plugin uses (handshake, READY, AUTHENTICATE/AUTHORIZE,
//! nonce-matched requests, event dispatch).
#![allow(dead_code)] // shared with the setup binary, which uses a subset

use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;

/// `code` is Some for errors Discord itself returned (e.g. a rejected token).
#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: Option<i64>,
    pub message: String,
}

impl RpcError {
    fn conn(m: impl Into<String>) -> Self {
        RpcError { code: None, message: m.into() }
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

type Reply = Result<Value, RpcError>;
type EventFn = Arc<dyn Fn(&str, &Value) + Send + Sync>;

// ── Transport ────────────────────────────────────────────────────────────────

#[cfg(windows)]
mod conn {
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::os::windows::io::AsRawHandle;

    extern "system" {
        fn PeekNamedPipe(h: *mut core::ffi::c_void, buf: *mut core::ffi::c_void, size: u32, read: *mut u32, avail: *mut u32, left: *mut u32) -> i32;
    }

    pub struct Conn(File);

    impl Conn {
        pub fn open(id: u32) -> io::Result<Conn> {
            OpenOptions::new().read(true).write(true).open(format!("\\\\.\\pipe\\discord-ipc-{id}")).map(Conn)
        }
        pub fn try_clone(&self) -> io::Result<Conn> {
            self.0.try_clone().map(Conn)
        }
        pub fn shutdown(&self) {}
    }

    impl Write for Conn {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.write(b)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    /// Synchronous pipe handles serialise I/O, so a blocked ReadFile would stall
    /// every write. Only read once PeekNamedPipe reports bytes are waiting;
    /// poll every 5 ms at first, backing off to 50 ms while the pipe is idle.
    impl Read for Conn {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let mut idle_polls = 0u32;
            loop {
                let mut avail = 0u32;
                let ok = unsafe { PeekNamedPipe(self.0.as_raw_handle() as *mut _, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut avail, std::ptr::null_mut()) };
                if ok == 0 {
                    return Ok(0); // broken pipe = closed
                }
                if avail > 0 {
                    let n = (avail as usize).min(buf.len());
                    return self.0.read(&mut buf[..n]);
                }
                idle_polls += 1;
                let ms = if idle_polls < 20 { 5 } else { 50 };
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
        }
    }
}

#[cfg(unix)]
mod conn {
    use std::io::{self, Read, Write};
    use std::os::unix::net::UnixStream;

    pub struct Conn(UnixStream);

    /// macOS per-user temp dir (`/var/folders/…/T/`), where Discord puts its
    /// socket. Read from the OS so it works when `TMPDIR` wasn't inherited
    /// (e.g. Arclet launched by launchd).
    #[cfg(target_os = "macos")]
    fn darwin_user_temp_dir() -> Option<String> {
        extern "C" {
            fn confstr(name: i32, buf: *mut std::ffi::c_char, len: usize) -> usize;
        }
        const CS_DARWIN_USER_TEMP_DIR: i32 = 65537;
        let mut buf = vec![0u8; 1024];
        let n = unsafe { confstr(CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr().cast(), buf.len()) };
        if n == 0 || n > buf.len() {
            return None;
        }
        buf.truncate(n - 1); // drop the NUL
        String::from_utf8(buf).ok()
    }

    /// Every directory Discord may have put `discord-ipc-N` in, most likely first.
    pub fn socket_dirs() -> Vec<String> {
        let mut dirs: Vec<String> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"].iter().filter_map(|k| std::env::var(k).ok()).filter(|d| !d.is_empty()).collect();
        #[cfg(target_os = "macos")]
        dirs.extend(darwin_user_temp_dir());
        dirs.push("/tmp".into());
        let mut seen = Vec::new();
        dirs.retain(|d| {
            let d = d.trim_end_matches('/').to_string();
            !seen.contains(&d) && {
                seen.push(d);
                true
            }
        });
        dirs
    }

    impl Conn {
        pub fn open(id: u32) -> io::Result<Conn> {
            let mut last = io::Error::new(io::ErrorKind::NotFound, "no Discord IPC socket");
            for dir in socket_dirs() {
                match UnixStream::connect(format!("{}/discord-ipc-{id}", dir.trim_end_matches('/'))) {
                    Ok(s) => return Ok(Conn(s)),
                    Err(e) => last = e,
                }
            }
            Err(last)
        }
        pub fn try_clone(&self) -> io::Result<Conn> {
            self.0.try_clone().map(Conn)
        }
        pub fn shutdown(&self) {
            let _ = self.0.shutdown(std::net::Shutdown::Both);
        }
    }

    impl Write for Conn {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.write(b)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    impl Read for Conn {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            self.0.read(b)
        }
    }
}

use conn::Conn;

fn frame(op: u32, data: &Value) -> Vec<u8> {
    let body = data.to_string().into_bytes();
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// Drop `null` object members (JS `JSON.stringify` omits `undefined`).
pub fn strip_nulls(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(m.into_iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k, strip_nulls(v))).collect::<Map<_, _>>()),
        other => other,
    }
}

// ── Client ───────────────────────────────────────────────────────────────────

pub struct Rpc {
    writer: Mutex<Conn>,
    pending: Mutex<HashMap<String, Sender<Reply>>>,
    ready: Mutex<Option<Sender<()>>>,
    on_event: Mutex<Option<EventFn>>,
    on_close: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    closed: AtomicBool,
    next: AtomicU64,
}

impl Rpc {
    /// Connect to the first available `discord-ipc-N` and wait for READY.
    pub fn connect(client_id: &str) -> Result<Arc<Rpc>, RpcError> {
        let conn = (0..10).find_map(|i| Conn::open(i).ok()).ok_or_else(|| RpcError::conn("Could not connect to IPC pipe"))?;
        let mut reader = conn.try_clone().map_err(|e| RpcError::conn(e.to_string()))?;
        let (ready_tx, ready_rx) = channel();
        let rpc = Arc::new(Rpc {
            writer: Mutex::new(conn),
            pending: Default::default(),
            ready: Mutex::new(Some(ready_tx)),
            on_event: Default::default(),
            on_close: Default::default(),
            closed: AtomicBool::new(false),
            next: AtomicU64::new(1),
        });
        rpc.write(OP_HANDSHAKE, &json!({"v": 1, "client_id": client_id}))?;

        let r = rpc.clone();
        std::thread::spawn(move || {
            r.read_loop(&mut reader);
            r.on_closed();
        });
        ready_rx.recv_timeout(Duration::from_secs(10)).map_err(|_| RpcError::conn("RPC_CONNECTION_TIMEOUT"))?;
        Ok(rpc)
    }

    fn write(&self, op: u32, data: &Value) -> Result<(), RpcError> {
        self.writer.lock().unwrap().write_all(&frame(op, data)).map_err(|e| RpcError::conn(e.to_string()))
    }

    fn read_loop(&self, reader: &mut Conn) {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
            while buf.len() >= 8 {
                let op = u32::from_le_bytes(buf[0..4].try_into().unwrap());
                let len = u32::from_le_bytes(buf[4..8].try_into().unwrap()) as usize;
                if buf.len() < 8 + len {
                    break;
                }
                let raw: Vec<u8> = buf.drain(..8 + len).skip(8).collect();
                let Ok(data) = serde_json::from_slice::<Value>(&raw) else { continue };
                match op {
                    OP_PING => {
                        let _ = self.write(OP_PONG, &data);
                    }
                    OP_CLOSE => return,
                    OP_FRAME => self.on_message(data),
                    _ => {}
                }
            }
        }
    }

    fn on_message(&self, msg: Value) {
        if msg["cmd"] == "DISPATCH" && msg["evt"] == "READY" {
            if let Some(tx) = self.ready.lock().unwrap().take() {
                let _ = tx.send(());
            }
            return;
        }
        let nonce = msg["nonce"].as_str().unwrap_or("");
        if let Some(tx) = self.pending.lock().unwrap().remove(nonce) {
            let _ = tx.send(if msg["evt"] == "ERROR" {
                Err(RpcError { code: msg["data"]["code"].as_i64(), message: msg["data"]["message"].as_str().unwrap_or("RPC error").to_string() })
            } else {
                Ok(msg["data"].clone())
            });
            return;
        }
        if let (Some(evt), Some(f)) = (msg["evt"].as_str(), self.on_event.lock().unwrap().clone()) {
            f(evt, &msg["data"]);
        }
    }

    fn on_closed(&self) {
        self.closed.store(true, Ordering::SeqCst);
        for (_, tx) in self.pending.lock().unwrap().drain() {
            let _ = tx.send(Err(RpcError::conn("connection closed")));
        }
        // A close during connect() unblocks the ready wait by dropping its sender.
        self.ready.lock().unwrap().take();
        if let Some(f) = self.on_close.lock().unwrap().take() {
            f();
        }
    }

    /// Run `f` once when the connection closes (immediately if it already has).
    pub fn set_on_close(&self, f: impl FnOnce() + Send + 'static) {
        if self.closed.load(Ordering::SeqCst) {
            return f();
        }
        *self.on_close.lock().unwrap() = Some(Box::new(f));
        if self.closed.load(Ordering::SeqCst) {
            if let Some(f) = self.on_close.lock().unwrap().take() {
                f();
            }
        }
    }

    pub fn set_on_event(&self, f: impl Fn(&str, &Value) + Send + Sync + 'static) {
        *self.on_event.lock().unwrap() = Some(Arc::new(f));
    }

    pub fn close(&self) {
        let _ = self.write(OP_CLOSE, &json!({}));
        self.writer.lock().unwrap().shutdown();
    }

    /// Send a command and wait for its reply. `Value::Null` args are omitted.
    pub fn request(&self, cmd: &str, args: Value) -> Reply {
        self.request_evt(cmd, args, None)
    }

    pub fn request_evt(&self, cmd: &str, args: Value, evt: Option<&str>) -> Reply {
        if self.closed.load(Ordering::SeqCst) {
            return Err(RpcError::conn("connection closed"));
        }
        let nonce = format!("{:016x}-{:08x}", self.next.fetch_add(1, Ordering::Relaxed), std::process::id());
        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(nonce.clone(), tx);
        let mut m = json!({"cmd": cmd, "nonce": nonce});
        if !args.is_null() {
            m["args"] = args;
        }
        if let Some(e) = evt {
            m["evt"] = e.into();
        }
        if let Err(e) = self.write(OP_FRAME, &m) {
            self.pending.lock().unwrap().remove(&nonce);
            return Err(e);
        }
        match rx.recv_timeout(Duration::from_secs(120)) {
            Ok(r) => r,
            Err(_) => {
                self.pending.lock().unwrap().remove(&nonce);
                Err(RpcError::conn(format!("{cmd} timed out")))
            }
        }
    }

    pub fn authenticate(&self, token: &str) -> Reply {
        self.request("AUTHENTICATE", json!({"access_token": token}))
    }

    pub fn subscribe(&self, event: &str) -> Reply {
        self.request_evt("SUBSCRIBE", json!({}), Some(event))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a running Discord desktop client: `cargo test -p discord -- --ignored`.
    #[test]
    #[ignore]
    fn live_handshake_and_error_reply() {
        let rpc = Rpc::connect("1527677329524916424").expect("handshake + READY");
        // Not authenticated, so Discord must answer with an ERROR frame carrying a numeric code.
        let err = rpc.request("GET_VOICE_SETTINGS", Value::Null).unwrap_err();
        assert!(err.code.is_some(), "expected a Discord error code, got {err}");
        rpc.close();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_socket_dirs_include_user_temp_dir() {
        let dirs = conn::socket_dirs();
        assert!(dirs.iter().any(|d| d.starts_with("/var/folders/") || d.starts_with("/private/var/folders/")), "{dirs:?}");
        assert_eq!(dirs.last().map(String::as_str), Some("/tmp"));
    }
}
