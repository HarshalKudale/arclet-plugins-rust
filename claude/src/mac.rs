//! macOS bits, same interface as `win.rs`: is a session's process still alive,
//! which app (and which Terminal / iTerm2 tab) hosts it, what is in front, and
//! bringing it there.
//!
//! - Processes: libproc (`proc_pidinfo` / `proc_pidpath`), no `ps` spawning.
//! - Host: the nearest ancestor that is an app bundle's main executable
//!   (Terminal, iTerm2, VS Code, the Claude app…). In Terminal and iTerm2 the
//!   session's tty pins down the exact tab, so tabs are never "shared".
//! - Front: the Accessibility API's focused application (Arclet already holds
//!   Accessibility, and its plugins inherit it), plus the terminal's front tab.
//! - Terminal tabs are found and selected through AppleScript (Automation
//!   permission for Terminal / iTerm2, asked on first use).

use arclet_sdk::{log, Value};
use std::collections::HashMap;
use std::ffi::{c_void, CStr};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Term {
    Terminal,
    ITerm,
}

impl Term {
    fn of(bundle: &str) -> Option<Term> {
        if bundle.ends_with("/Terminal.app") {
            Some(Term::Terminal)
        } else if bundle.ends_with("/iTerm.app") {
            Some(Term::ITerm)
        } else {
            None
        }
    }
}

/// What hosts a session: an app, plus the tab's tty when the app is a terminal
/// we can address by tab. Equal windows = sessions that can't be told apart.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Window {
    pub pid: i32,
    /// The app bundle (`/System/Applications/Utilities/Terminal.app`).
    pub bundle: String,
    /// `/dev/ttys003`, only for Terminal / iTerm2.
    pub tty: Option<String>,
}

// ── Processes (libproc) ──────────────────────────────────────────────────────

fn bsd_info(pid: u32) -> Option<libc::proc_bsdinfo> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let n = unsafe { libc::proc_pidinfo(pid as i32, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut c_void, size) };
    (n == size).then_some(info)
}

/// Parent pid. The short info is readable for other users' processes too
/// (`login`, between Terminal and the shell, runs as root), unlike the full one.
fn ppid_of(pid: u32) -> Option<u32> {
    let mut info: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdshortinfo>() as i32;
    let n = unsafe { libc::proc_pidinfo(pid as i32, libc::PROC_PIDT_SHORTBSDINFO, 0, &mut info as *mut _ as *mut c_void, size) };
    (n == size).then_some(info.pbsi_ppid)
}

fn exe_path(pid: u32) -> Option<String> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = unsafe { libc::proc_pidpath(pid as i32, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf8(buf).ok()
}

/// `/Applications/X.app` when `path` is that bundle's main executable
/// (`X.app/Contents/MacOS/x`), not a helper nested inside it.
fn main_bundle(path: &str) -> Option<String> {
    let i = path.find(".app/")?;
    let (bundle, rest) = (&path[..i + 4], &path[i + 4..]);
    let name = rest.strip_prefix("/Contents/MacOS/")?;
    (!name.is_empty() && !name.contains('/')).then(|| bundle.to_string())
}

fn tty_of(info: &libc::proc_bsdinfo) -> Option<String> {
    extern "C" {
        fn devname(dev: libc::dev_t, kind: libc::mode_t) -> *const libc::c_char;
    }
    if info.e_tdev == u32::MAX || info.e_tdev == 0 {
        return None;
    }
    let p = unsafe { devname(info.e_tdev as libc::dev_t, libc::S_IFCHR) };
    if p.is_null() {
        return None;
    }
    let name = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    (!name.is_empty() && name != "??").then(|| format!("/dev/{name}"))
}

/// Seconds since the epoch from Claude Code's `procStart` on macOS, which is the
/// UTC `ps -o lstart` form: `"Sun Nov  1 08:32:23 2026"`.
fn parse_lstart(s: &str) -> Option<u64> {
    let f: Vec<&str> = s.split_whitespace().collect();
    let [_, mon, day, hms, year] = f.as_slice() else { return None };
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let m = MONTHS.iter().position(|x| x == mon)? as i64 + 1;
    let (d, y): (i64, i64) = (day.parse().ok()?, year.parse().ok()?);
    let t: Vec<i64> = hms.split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let [hh, mm, ss] = t.as_slice() else { return None };
    // days-from-civil (Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + hh * 3600 + mm * 60 + ss).ok()
}

/// Whether `pid` is still the process that wrote the session file (a pid can be reused).
pub fn alive(pid: u32, proc_start: &Value) -> bool {
    let Some(info) = bsd_info(pid) else { return false };
    match proc_start.as_str().and_then(parse_lstart) {
        Some(t) => info.pbi_start_tvsec.abs_diff(t) <= 1,
        None => true,
    }
}

// ── Host window ──────────────────────────────────────────────────────────────

/// Nothing to snapshot up front: lookups are per pid and cached.
pub struct Procs;

pub fn snapshot() -> Procs {
    Procs
}

/// (pid, start time) → host. A session's host never changes, and the iTerm2
/// fallback runs AppleScript, so look each session up once.
static HOSTS: Mutex<Option<HashMap<(u32, u64), Option<Window>>>> = Mutex::new(None);

pub fn host_window(pid: u32, _: &Procs) -> Option<Window> {
    let info = bsd_info(pid)?;
    let key = (pid, info.pbi_start_tvsec);
    if let Some(hit) = HOSTS.lock().unwrap().as_ref().and_then(|m| m.get(&key).cloned()) {
        return hit;
    }
    let found = find_host(&info);
    let mut g = HOSTS.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    m.retain(|(p, t), _| bsd_info(*p).is_some_and(|i| i.pbi_start_tvsec == *t));
    m.insert(key, found.clone());
    found
}

fn find_host(info: &libc::proc_bsdinfo) -> Option<Window> {
    let tty = tty_of(info);
    let mut p = info.pbi_ppid;
    for _ in 0..20 {
        if p <= 1 {
            break;
        }
        if let Some(bundle) = exe_path(p).as_deref().and_then(main_bundle) {
            let tty = Term::of(&bundle).and(tty.clone());
            return Some(Window { pid: p as i32, bundle, tty });
        }
        p = ppid_of(p)?;
    }
    // iTerm2 runs shells under its own server process (parented to launchd), so
    // the ancestry never reaches iTerm2 itself: ask it whether it owns the tty.
    let tty = tty?;
    let (pid, bundle) = running_app("/iTerm.app")?;
    let owns = osascript(ITERM_HAS_TTY, &[&tty]).is_some_and(|out| out == "yes");
    owns.then(|| Window { pid: pid as i32, bundle, tty: Some(tty) })
}

/// A running app whose bundle path ends with `suffix`: (pid, bundle).
fn running_app(suffix: &str) -> Option<(u32, String)> {
    let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if n <= 0 {
        return None;
    }
    let mut pids = vec![0i32; n as usize + 64];
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr() as *mut c_void, (pids.len() * 4) as i32) };
    pids.truncate(n.max(0) as usize);
    pids.into_iter().filter(|&p| p > 1).find_map(|p| {
        let bundle = exe_path(p as u32).as_deref().and_then(main_bundle)?;
        bundle.ends_with(suffix).then_some((p as u32, bundle))
    })
}

// ── AppleScript (Terminal / iTerm2 tabs) ─────────────────────────────────────

/// Run `script` with `args` as `argv` (never spliced into the source). Trimmed stdout.
fn osascript(script: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("/usr/bin/osascript").arg("-e").arg(script).args(args).output().ok()?;
    if !out.status.success() {
        log(format!("osascript failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

const ITERM_HAS_TTY: &str = r#"on run argv
  tell application "iTerm2"
    repeat with w in windows
      repeat with t in tabs of w
        repeat with s in sessions of t
          if tty of s is (item 1 of argv) then return "yes"
        end repeat
      end repeat
    end repeat
  end tell
  return "no"
end run"#;

const TERMINAL_SELECT: &str = r#"on run argv
  tell application "Terminal"
    repeat with w in windows
      repeat with t in tabs of w
        if tty of t is (item 1 of argv) then
          set selected of t to true
          set index of w to 1
          activate
          return "ok"
        end if
      end repeat
    end repeat
  end tell
  return "missing"
end run"#;

const ITERM_SELECT: &str = r#"on run argv
  tell application "iTerm2"
    repeat with w in windows
      repeat with t in tabs of w
        repeat with s in sessions of t
          if tty of s is (item 1 of argv) then
            select w
            tell t to select
            tell s to select
            activate
            return "ok"
          end if
        end repeat
      end repeat
    end repeat
  end tell
  return "missing"
end run"#;

const TERMINAL_FRONT_TTY: &str = r#"tell application "Terminal" to return tty of selected tab of front window"#;
const ITERM_FRONT_TTY: &str = r#"tell application "iTerm2" to return tty of current session of current window"#;

// ── Front / focus ────────────────────────────────────────────────────────────

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> *const c_void;
    fn AXUIElementCopyAttributeValue(el: *const c_void, attr: *const c_void, value: *mut *const c_void) -> i32;
    fn AXUIElementGetPid(el: *const c_void, pid: *mut i32) -> i32;
}

/// pid of the focused (frontmost) app, through the Accessibility API.
fn frontmost_pid() -> Option<i32> {
    use core_foundation::base::{CFRelease, TCFType};
    use core_foundation::string::CFString;
    let attr = CFString::from_static_string("AXFocusedApplication");
    unsafe {
        let sys = AXUIElementCreateSystemWide();
        if sys.is_null() {
            return None;
        }
        let mut app: *const c_void = std::ptr::null();
        let err = AXUIElementCopyAttributeValue(sys, attr.as_concrete_TypeRef() as *const c_void, &mut app);
        CFRelease(sys);
        if err != 0 || app.is_null() {
            return None;
        }
        let mut pid = 0;
        let ok = AXUIElementGetPid(app, &mut pid) == 0;
        CFRelease(app);
        ok.then_some(pid)
    }
}

/// What is in front right now. The terminal's front tab is only asked for
/// (AppleScript) when a session in that terminal is being checked.
pub struct Front {
    pid: Option<i32>,
    tty: OnceLock<Option<String>>,
}

pub fn front() -> Front {
    Front { pid: frontmost_pid(), tty: OnceLock::new() }
}

impl Front {
    pub fn shows(&self, w: &Window) -> bool {
        if self.pid != Some(w.pid) {
            return false;
        }
        let Some(tty) = &w.tty else { return true };
        let front_tty = self.tty.get_or_init(|| match Term::of(&w.bundle)? {
            Term::Terminal => osascript(TERMINAL_FRONT_TTY, &[]),
            Term::ITerm => osascript(ITERM_FRONT_TTY, &[]),
        });
        front_tty.as_deref() == Some(tty.as_str())
    }
}

/// Bring the session to the front: its exact tab in Terminal / iTerm2, else its app.
pub fn focus(w: &Window) {
    let selected = match (Term::of(&w.bundle), &w.tty) {
        (Some(Term::Terminal), Some(tty)) => osascript(TERMINAL_SELECT, &[tty]),
        (Some(Term::ITerm), Some(tty)) => osascript(ITERM_SELECT, &[tty]),
        _ => None,
    };
    if selected.as_deref() == Some("ok") {
        return;
    }
    if let Err(e) = arclet_sdk::focus_app(&w.bundle) {
        log(format!("focus {}: {e}", w.bundle));
    }
}

pub fn open_url(url: &str) -> std::io::Result<()> {
    let status = Command::new("/usr/bin/open").arg(url).status()?;
    if !status.success() {
        return Err(std::io::Error::other(format!("open {url} failed")));
    }
    Ok(())
}

/// Run `claude <args>` in a new Terminal.app window in `cwd`. Every value is
/// passed as `argv` and shell-quoted by AppleScript's `quoted form of`.
pub fn run_in_terminal(cwd: &str, args: &[&str]) -> std::io::Result<()> {
    const SCRIPT: &str = r#"on run argv
  set cmd to "cd " & quoted form of (item 1 of argv) & " && claude"
  repeat with i from 2 to count of argv
    set cmd to cmd & " " & quoted form of (item i of argv)
  end repeat
  tell application "Terminal"
    activate
    do script cmd
  end tell
end run"#;
    let mut argv = vec![cwd];
    argv.extend_from_slice(args);
    osascript(SCRIPT, &argv).map(|_| ()).ok_or_else(|| std::io::Error::other("Couldn't open Terminal (allow Arclet to control Terminal in System Settings → Privacy & Security → Automation)"))
}

// ── Directory watch ──────────────────────────────────────────────────────────

/// Call `on_change` whenever a file in `dir` is written, created or removed (blocks forever).
/// Returns if the directory can't be read (e.g. it doesn't exist yet). Polls names,
/// sizes and mtimes every 250 ms: `sessions/` holds a handful of small files.
pub fn watch_dir(dir: &std::path::Path, mut on_change: impl FnMut()) {
    let stamp = || -> Option<Vec<(std::ffi::OsString, Option<std::time::SystemTime>, u64)>> {
        let mut v: Vec<_> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| {
                let md = e.metadata().ok();
                (e.file_name(), md.as_ref().and_then(|m| m.modified().ok()), md.map_or(0, |m| m.len()))
            })
            .collect();
        v.sort();
        Some(v)
    };
    let Some(mut last) = stamp() else { return };
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let Some(now) = stamp() else { return };
        if now != last {
            last = now;
            // Claude Code writes several fields in quick succession; settle before reading.
            std::thread::sleep(Duration::from_millis(100));
            on_change();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lstart_parses_as_utc() {
        // date -u -r 1793521943
        assert_eq!(parse_lstart("Sun Nov  1 08:32:23 2026"), Some(1793521943));
        assert_eq!(parse_lstart("Thu Jan  1 00:00:00 1970"), Some(0));
        assert_eq!(parse_lstart("Tue Feb 29 12:00:00 2028"), Some(1835438400));
        assert_eq!(parse_lstart("garbage"), None);
    }

    #[test]
    fn main_bundle_skips_helpers() {
        assert_eq!(main_bundle("/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal").as_deref(), Some("/System/Applications/Utilities/Terminal.app"));
        assert_eq!(main_bundle("/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper.app/Contents/MacOS/Code Helper"), None);
        assert_eq!(main_bundle("/bin/zsh"), None);
    }

    #[test]
    fn this_process_is_alive_with_its_own_start_time() {
        let me = std::process::id();
        let start = bsd_info(me).unwrap().pbi_start_tvsec;
        assert!(alive(me, &Value::Null));
        assert!(!alive(me, &Value::String("Thu Jan  1 00:00:00 1970".into())));
        assert!(!alive(u32::MAX - 1, &Value::Null));
        assert!(start > 0);
    }

    #[test]
    fn watch_dir_fires_on_write() {
        let dir = std::env::temp_dir().join(format!("claude-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let d = dir.clone();
        std::thread::spawn(move || watch_dir(&d, move || {
            let _ = tx.send(());
        }));
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(dir.join("123.json"), "{}").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(3)).is_ok(), "no change notification");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
