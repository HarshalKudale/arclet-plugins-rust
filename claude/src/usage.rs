//! Token usage from local Claude Code transcripts (`projects/**/*.jsonl`), summed over
//! rolling 5-hour / 7-day / 30-day windows. Files are read incrementally (only bytes
//! appended since the last pass), so the 1 GB+ history is parsed once, not every refresh.

use crate::sessions::{claude_dir, now_ms};
use arclet_sdk::Value;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

pub const WINDOWS: [(&str, u64); 3] = [("5h", 5 * 3600_000), ("7d", 7 * 86400_000), ("30d", 30 * 86400_000)];

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Totals {
    pub tokens: u64,
    pub messages: u64,
}

#[derive(Default)]
struct FileScan {
    offset: u64,
    /// message id → (timestamp ms, tokens). Claude Code writes one line per content
    /// block, each repeating the message's usage, so last-wins per id.
    msgs: HashMap<String, (u64, u64)>,
}

static FILES: Mutex<Option<HashMap<PathBuf, FileScan>>> = Mutex::new(None);
static SNAPSHOT: Mutex<Option<[Totals; 3]>> = Mutex::new(None);

/// Latest computed totals, one per entry of [`WINDOWS`]; `None` until the first scan finishes.
pub fn snapshot() -> Option<[Totals; 3]> {
    *SNAPSHOT.lock().unwrap()
}

fn walk(dir: &Path, depth: u8, cutoff: SystemTime, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(md) = e.metadata() else { continue };
        if md.is_dir() && depth > 0 {
            walk(&p, depth - 1, cutoff, out);
        } else if p.extension().map_or(false, |x| x == "jsonl") && md.modified().map_or(true, |m| m >= cutoff) {
            out.push(p);
        }
    }
}

/// Rescan transcripts and update the snapshot. Run off the UI path (it can take seconds the first time).
pub fn refresh() {
    let horizon = WINDOWS[2].1;
    let cutoff = SystemTime::now() - Duration::from_millis(horizon);
    let mut paths = vec![];
    // projects/<project>/<session>.jsonl and projects/<project>/<session>/subagents/*.jsonl
    walk(&claude_dir().join("projects"), 3, cutoff, &mut paths);

    let mut guard = FILES.lock().unwrap();
    let files = guard.get_or_insert_with(HashMap::new);
    files.retain(|p, _| paths.contains(p));
    for p in paths {
        let scan = files.entry(p.clone()).or_default();
        read_new(&p, scan);
    }

    let now = now_ms();
    let mut totals = [Totals::default(); 3];
    for scan in files.values_mut() {
        scan.msgs.retain(|_, (ts, _)| now.saturating_sub(*ts) <= horizon);
        for &(ts, tokens) in scan.msgs.values() {
            for (i, (_, span)) in WINDOWS.iter().enumerate() {
                if now.saturating_sub(ts) <= *span {
                    totals[i].tokens += tokens;
                    totals[i].messages += 1;
                }
            }
        }
    }
    *SNAPSHOT.lock().unwrap() = Some(totals);
}

fn read_new(path: &Path, scan: &mut FileScan) {
    let Ok(mut f) = std::fs::File::open(path) else { return };
    let len = f.metadata().map_or(0, |m| m.len());
    if len < scan.offset {
        *scan = FileScan::default(); // rewritten / truncated
    }
    if len == scan.offset || f.seek(SeekFrom::Start(scan.offset)).is_err() {
        return;
    }
    let mut buf = vec![];
    if f.read_to_end(&mut buf).is_err() {
        return;
    }
    // Only consume complete lines; a half-written last line is picked up next pass.
    let Some(end) = buf.iter().rposition(|&b| b == b'\n') else { return };
    scan.offset += end as u64 + 1;
    for line in String::from_utf8_lossy(&buf[..end]).lines() {
        if let Some((id, ts, tokens)) = parse_line(line) {
            scan.msgs.insert(id, (ts, tokens));
        }
    }
}

fn parse_line(line: &str) -> Option<(String, u64, u64)> {
    if !line.contains("\"usage\"") || !line.contains("\"assistant\"") {
        return None;
    }
    let v: Value = line.parse().ok()?;
    if v["type"] != "assistant" {
        return None;
    }
    let m = &v["message"];
    let u = &m["usage"];
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    let tokens = n("input_tokens") + n("output_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
    Some((m["id"].as_str()?.to_string(), parse_iso_ms(v["timestamp"].as_str()?)?, tokens))
}

/// `2026-10-31T09:02:52.562Z` → unix ms (UTC only, which is what Claude Code writes).
pub fn parse_iso_ms(s: &str) -> Option<u64> {
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let frac = s.get(19..).and_then(|r| r.strip_prefix('.')).map_or(0, |r| {
        let digits: String = r.chars().take_while(char::is_ascii_digit).take(3).collect();
        format!("{digits:0<3}").parse().unwrap_or(0)
    });
    // days_from_civil (Howard Hinnant)
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(((days * 24 + h) * 60 + mi) * 60 + sec).ok().map(|t| t * 1000 + frac)
}

/// 1234 → "1.2k", 3_400_000 → "3.4M".
pub fn compact(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{:.1}B", n as f64 / 1e9),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso_ms("2026-10-31T09:02:52.562Z"), Some(1_793_437_372_562));
        assert_eq!(parse_iso_ms("2024-02-29T12:00:00.5Z"), Some(1_709_208_000_500));
        assert_eq!(parse_iso_ms("garbage"), None);
    }

    #[test]
    fn line() {
        let l = r#"{"type":"assistant","timestamp":"1970-01-01T00:00:01Z","message":{"id":"m1","usage":{"input_tokens":2,"cache_creation_input_tokens":10,"cache_read_input_tokens":100,"output_tokens":5}}}"#;
        assert_eq!(parse_line(l), Some(("m1".into(), 1000, 117)));
        assert_eq!(parse_line(r#"{"type":"user","usage":1}"#), None);
    }

    #[test]
    fn compacts() {
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1234), "1.2k");
        assert_eq!(compact(3_400_000), "3.4M");
    }
}

/// Against this machine's real `~/.claude`: `cargo test -p claude -- --ignored --nocapture`.
#[cfg(test)]
#[test]
#[ignore]
fn live_smoke() {
    for s in crate::sessions::sessions() {
        println!("session {} [{}] status={:?} rc={:?} window={:?} shared={}", s.name, s.id, s.status, s.bridge, s.window, s.shared_window);
    }
    for h in crate::sessions::recent(5) {
        println!("recent {} {} {}", h.id, h.project, h.label.chars().take(50).collect::<String>());
    }
    let t = std::time::Instant::now();
    refresh();
    println!("first scan {:?}", t.elapsed());
    let t = std::time::Instant::now();
    refresh();
    println!("second scan {:?} -> {:?}", t.elapsed(), snapshot());
}
