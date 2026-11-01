//! Read-only view of Claude Code's local state in `~/.claude` (or `CLAUDE_CONFIG_DIR`):
//! `sessions/<pid>.json` (live sessions, written by each running Claude Code),
//! `history.jsonl` (prompt history → recent sessions) and the transcripts under `projects/`.

use crate::os::{self, Window};
use arclet_sdk::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn claude_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return d.into();
    }
    let home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    PathBuf::from(std::env::var_os(home).unwrap_or_default()).join(".claude")
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Running,
    /// Waiting on the user (permission prompt, question…): `status: "waiting"`.
    Waiting,
    /// Can't continue on its own: `state: "blocked" | "failed"` (login, rate limit, API error…).
    Blocked,
    Done,
    Interrupted,
    Stopped,
}

impl Status {
    pub fn color(self) -> &'static str {
        match self {
            Status::Running => "#f59e0b",
            Status::Waiting => "#3b82f6",
            Status::Done => "#22c55e",
            Status::Interrupted | Status::Stopped | Status::Blocked => "#ef4444",
        }
    }
    pub fn word(self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Waiting => "needs you",
            Status::Blocked => "blocked",
            Status::Done => "done",
            Status::Interrupted => "interrupted",
            Status::Stopped => "stopped",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub cwd: String,
    pub status: Status,
    /// What it is waiting for / why it is blocked, as Claude Code reports it.
    pub detail: Option<String>,
    /// Remote Control session id (`session_…`) while Remote Control is on.
    pub bridge: Option<String>,
    pub updated: u64,
    pub window: Option<Window>,
    /// Another live session lives in the same window and can't be told apart
    /// (e.g. two Windows Terminal tabs, two VS Code terminals).
    pub shared_window: bool,
}

impl Session {
    pub fn alive(&self) -> bool {
        self.status != Status::Stopped
    }
}

/// Claude Code's project folder name: every non-alphanumeric char becomes '-'.
fn project_dir(cwd: &str) -> PathBuf {
    let enc: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    claude_dir().join("projects").join(enc)
}

/// Whether the last conversation message in `text` (a transcript tail) is the
/// interrupt marker Claude Code writes when the user stops a turn.
fn ends_interrupted(text: &str) -> bool {
    text.lines()
        .rev()
        .find(|l| l.contains("\"type\":\"user\"") || l.contains("\"type\":\"assistant\""))
        .map_or(false, |l| l.contains("[Request interrupted by user"))
}

fn transcript_interrupted(cwd: &str, id: &str) -> bool {
    let Ok(mut f) = std::fs::File::open(project_dir(cwd).join(format!("{id}.jsonl"))) else { return false };
    let len = f.metadata().map_or(0, |m| m.len());
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(64 * 1024)));
    let mut buf = vec![];
    let _ = f.read_to_end(&mut buf);
    ends_interrupted(&String::from_utf8_lossy(&buf))
}

/// Stale pid files of crashed sessions are shown as "stopped" for this long.
const STOPPED_FOR_MS: u64 = 6 * 3600 * 1000;

/// Live sessions (plus recently crashed ones), most recently active first.
pub fn sessions() -> Vec<Session> {
    let Ok(dir) = std::fs::read_dir(claude_dir().join("sessions")) else { return vec![] };
    let procs = os::snapshot();
    let now = now_ms();
    let mut out: Vec<Session> = dir
        .flatten()
        .filter(|e| e.path().extension().map_or(false, |x| x == "json"))
        .filter_map(|e| {
            let v: Value = std::fs::read_to_string(e.path()).ok()?.parse().ok()?;
            let pid = v["pid"].as_u64()? as u32;
            let id = v["sessionId"].as_str()?.to_string();
            let cwd = v["cwd"].as_str().unwrap_or("").to_string();
            let updated = v["updatedAt"].as_u64().or(v["startedAt"].as_u64()).unwrap_or(0);
            // A pid alone can be reused; the creation time pins it to this session.
            let alive = os::alive(pid, &v["procStart"]);
            let text = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(String::from);
            let blocked = matches!(v["state"].as_str(), Some("blocked" | "failed"));
            let status = match (alive, v["status"].as_str()) {
                (false, _) => Status::Stopped,
                (true, _) if blocked => Status::Blocked,
                (true, Some("waiting")) => Status::Waiting,
                (true, Some("idle")) if transcript_interrupted(&cwd, &id) => Status::Interrupted,
                (true, Some("idle")) => Status::Done,
                _ => Status::Running,
            };
            let detail = match status {
                Status::Blocked => text("needs").or_else(|| text("detail")),
                Status::Waiting => text("waitingFor"),
                _ => None,
            };
            if !alive && now.saturating_sub(updated) > STOPPED_FOR_MS {
                return None;
            }
            let name = v["name"].as_str().map(String::from).unwrap_or_else(|| folder_name(&cwd));
            let bridge = v["bridgeSessionId"].as_str().filter(|b| b.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')).map(String::from);
            let window = alive.then(|| os::host_window(pid, &procs)).flatten();
            Some(Session { id, name, cwd, status, detail, bridge, updated, window, shared_window: false })
        })
        .collect();
    let windows: Vec<Window> = out.iter().filter_map(|s| s.window.clone()).collect();
    for s in &mut out {
        s.shared_window = s.window.as_ref().map_or(false, |w| windows.iter().filter(|x| *x == w).count() > 1);
    }
    out.sort_by(|a, b| b.updated.cmp(&a.updated));
    out
}

/// The session the user is looking at (its window is in front), else the most recently active one.
pub fn focused_or_latest(list: &[Session]) -> Option<&Session> {
    let front = os::front();
    list.iter().filter(|s| s.alive()).find(|s| s.window.as_ref().is_some_and(|w| front.shows(w))).or_else(|| list.iter().find(|s| s.alive()))
}

pub fn folder_name(path: &str) -> String {
    path.rsplit(['\\', '/']).find(|s| !s.is_empty()).unwrap_or(path).to_string()
}

#[derive(Clone, Debug)]
pub struct HistoryItem {
    pub id: String,
    pub project: String,
    pub label: String,
}

/// The `n` most recent distinct sessions from `history.jsonl`, labeled by their latest non-command prompt.
pub fn recent(n: usize) -> Vec<HistoryItem> {
    let text = std::fs::read_to_string(claude_dir().join("history.jsonl")).unwrap_or_default();
    parse_recent(&text, n)
}

fn parse_recent(text: &str, n: usize) -> Vec<HistoryItem> {
    let mut out: Vec<HistoryItem> = vec![];
    for line in text.lines().rev() {
        let Ok(v) = line.parse::<Value>() else { continue };
        let (Some(id), Some(display)) = (v["sessionId"].as_str(), v["display"].as_str()) else { continue };
        let label = display.split_whitespace().collect::<Vec<_>>().join(" ");
        match out.iter().position(|h| h.id == id) {
            Some(i) if out[i].label.starts_with('/') && !label.starts_with('/') => out[i].label = label,
            Some(_) => {}
            None if out.len() < n => out.push(HistoryItem { id: id.into(), project: v["project"].as_str().unwrap_or("").into(), label }),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupt_marker_only_counts_when_last() {
        let a = r#"{"type":"assistant","x":1}"#;
        let i = r#"{"type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#;
        let meta = r#"{"type":"system","subtype":"turn_duration"}"#;
        assert!(ends_interrupted(&format!("{a}\n{i}\n{meta}\n")));
        assert!(!ends_interrupted(&format!("{i}\n{a}\n{meta}\n")));
    }

    #[test]
    fn recent_dedupes_and_prefers_real_prompts() {
        let h = [
            r#"{"display":"old one","sessionId":"s1","project":"C:\\p1"}"#,
            r#"{"display":"fix  the\nbug","sessionId":"s2","project":"C:\\p2"}"#,
            r#"{"display":"/rc","sessionId":"s2","project":"C:\\p2"}"#,
            r#"{"display":"third","sessionId":"s3","project":"C:\\p3"}"#,
        ]
        .join("\n");
        let r = parse_recent(&h, 2);
        assert_eq!(r.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["s3", "s2"]);
        assert_eq!(r[1].label, "fix the bug");
    }

    /// Against this machine's ~/.claude: `cargo test -p claude -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_sessions_smoke() {
        for s in sessions() {
            println!("{} [{}] {:?} window={:?} shared={}", s.name, s.status.word(), s.detail, s.window, s.shared_window);
        }
        if let Some(s) = focused_or_latest(&sessions()) {
            println!("focused or latest: {}", s.name);
        }
    }

    #[test]
    fn project_dir_encoding() {
        assert!(project_dir("I:\\workspace\\Arclet-workspace").ends_with("I--workspace-Arclet-workspace"));
    }
}
