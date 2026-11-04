//! Token usage from local transcripts, summed over the last 30 days: Claude Code's
//! `projects/**/*.jsonl` and Codex's `sessions/YYYY/MM/DD/*.jsonl`. Files are read
//! incrementally (only bytes appended since the last pass), so a 1 GB+ history is parsed
//! once, not every refresh.

use crate::limits::Provider;
use knobdeck_sdk::Value;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const SPAN_MS: u64 = 30 * 86400_000;

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[derive(Default)]
struct FileScan {
    offset: u64,
    /// dedupe key → (timestamp ms, tokens). Claude writes one line per content block, each
    /// repeating the message's usage (key: message id); Codex can re-emit a token_count event
    /// unchanged (key: the session's cumulative total). Last wins per key.
    msgs: HashMap<String, (u64, u64)>,
}

/// Per (provider, data dir): every transcript file seen in the last 30 days.
static FILES: Mutex<Option<HashMap<(Provider, PathBuf), HashMap<PathBuf, FileScan>>>> = Mutex::new(None);

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

/// Tokens used over the last 30 days, read from `dir` (`~/.claude` or `~/.codex`).
/// The first call over a large history takes seconds; later ones only read new bytes.
pub fn tokens(p: Provider, dir: &Path) -> u64 {
    let cutoff = SystemTime::now() - Duration::from_millis(SPAN_MS);
    let mut paths = vec![];
    match p {
        // projects/<project>/<session>.jsonl and projects/<project>/<session>/subagents/*.jsonl
        Provider::Claude => walk(&dir.join("projects"), 3, cutoff, &mut paths),
        // sessions/<yyyy>/<mm>/<dd>/rollout-*.jsonl
        Provider::Codex => walk(&dir.join("sessions"), 3, cutoff, &mut paths),
    }
    let parse = match p {
        Provider::Claude => parse_claude,
        Provider::Codex => parse_codex,
    };

    let mut guard = FILES.lock().unwrap();
    let files = guard.get_or_insert_with(HashMap::new).entry((p, dir.to_path_buf())).or_default();
    files.retain(|f, _| paths.contains(f));
    for f in paths {
        read_new(&f, files.entry(f.clone()).or_default(), parse);
    }
    let now = now_ms();
    let mut total = 0;
    for scan in files.values_mut() {
        scan.msgs.retain(|_, (ts, _)| now.saturating_sub(*ts) <= SPAN_MS);
        total += scan.msgs.values().map(|(_, t)| t).sum::<u64>();
    }
    total
}

fn read_new(path: &Path, scan: &mut FileScan, parse: fn(&str) -> Option<(String, u64, u64)>) {
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
        if let Some((id, ts, tokens)) = parse(line) {
            scan.msgs.insert(id, (ts, tokens));
        }
    }
}

fn parse_claude(line: &str) -> Option<(String, u64, u64)> {
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

/// A Codex `token_count` event: `last_token_usage.total_tokens`, keyed by the cumulative
/// `total_token_usage.total_tokens` so a repeated event counts once.
fn parse_codex(line: &str) -> Option<(String, u64, u64)> {
    if !line.contains("\"token_count\"") {
        return None;
    }
    let v: Value = line.parse().ok()?;
    let info = &v["payload"]["info"];
    if v["payload"]["type"] != "token_count" {
        return None;
    }
    let total = info["total_token_usage"]["total_tokens"].as_u64()?;
    Some((total.to_string(), parse_iso_ms(v["timestamp"].as_str()?)?, info["last_token_usage"]["total_tokens"].as_u64()?))
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
        assert_eq!(parse_claude(l), Some(("m1".into(), 1000, 117)));
        assert_eq!(parse_claude(r#"{"type":"user","usage":1}"#), None);
    }

    #[test]
    fn codex_line() {
        let l = r#"{"timestamp":"1970-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":44039},"last_token_usage":{"total_tokens":22223}}}}"#;
        assert_eq!(parse_codex(l), Some(("44039".into(), 1000, 22223)));
        assert_eq!(parse_codex(r#"{"payload":{"type":"token_count","info":null}}"#), None);
    }

    #[test]
    fn compacts() {
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1234), "1.2k");
        assert_eq!(compact(3_400_000), "3.4M");
    }
}

/// Against this machine's real data: `cargo test -p ai-stats -- --ignored --nocapture`.
#[cfg(test)]
#[test]
#[ignore]
fn live_smoke() {
    let home = PathBuf::from(std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap());
    for (p, d) in [(Provider::Claude, ".claude"), (Provider::Codex, ".codex")] {
        let t = std::time::Instant::now();
        let n = tokens(p, &home.join(d));
        println!("{p:?} first {:?} -> {n}", t.elapsed());
        let t = std::time::Instant::now();
        println!("{p:?} again {:?} -> {}", t.elapsed(), tokens(p, &home.join(d)));
    }
    for (p, f) in [(Provider::Claude, ".claude/.credentials.json"), (Provider::Codex, ".codex/auth.json")] {
        let v = crate::limits::fetch(p, home.join(f).to_str().unwrap());
        println!("{p:?} limits: {:?}", v.map(|v| (crate::limits::claude_window(&v, "5h"), crate::limits::claude_window(&v, "7d"), crate::limits::codex_window(&v, "primary"))));
    }
}
