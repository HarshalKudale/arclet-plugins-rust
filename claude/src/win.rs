//! Win32 bits: process tree, process start time (to tell a live session from a
//! stale pid file) and finding / focusing the window that hosts a process.

use std::collections::HashMap;
use std::ffi::c_void;
use windows::Win32::Foundation::{CloseHandle, BOOL, FILETIME, HWND, LPARAM, TRUE};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::*;

pub type Hwnd = isize;

fn h(hwnd: Hwnd) -> HWND {
    HWND(hwnd as *mut c_void)
}

/// pid → (parent pid, lowercase exe name).
pub fn process_tree() -> HashMap<u32, (u32, String)> {
    let mut map = HashMap::new();
    let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else { return map };
    let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
    if unsafe { Process32FirstW(snap, &mut e) }.is_ok() {
        loop {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            map.insert(e.th32ProcessID, (e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase()));
            if unsafe { Process32NextW(snap, &mut e) }.is_err() {
                break;
            }
        }
    }
    let _ = unsafe { CloseHandle(snap) };
    map
}

/// Process creation time as raw FILETIME ticks (what Claude Code writes as `procStart`).
pub fn started_at(pid: u32) -> Option<u64> {
    let proc = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let (mut c, mut x, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    let ok = unsafe { GetProcessTimes(proc, &mut c, &mut x, &mut k, &mut u) }.is_ok();
    let _ = unsafe { CloseHandle(proc) };
    ok.then(|| ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64)
}

unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
    (*(data.0 as *mut Vec<Hwnd>)).push(hwnd.0 as isize);
    TRUE
}

/// pid → its visible, titled, unowned top-level windows.
pub fn windows_by_pid() -> HashMap<u32, Vec<Hwnd>> {
    let mut all: Vec<Hwnd> = vec![];
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(&mut all as *mut _ as isize)) };
    let mut map: HashMap<u32, Vec<Hwnd>> = HashMap::new();
    for w in all {
        let visible = unsafe { IsWindowVisible(h(w)) }.as_bool();
        let owned = unsafe { GetWindow(h(w), GW_OWNER) }.map_or(false, |o| !o.0.is_null());
        if !visible || owned || unsafe { GetWindowTextLengthW(h(w)) } == 0 {
            continue;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(h(w), Some(&mut pid)) };
        map.entry(pid).or_default().push(w);
    }
    map
}

/// The window hosting `pid`: the nearest ancestor (or its conhost child) that owns
/// a top-level window — Windows Terminal, VS Code, a classic console, the Claude app.
pub fn host_window(pid: u32, tree: &HashMap<u32, (u32, String)>, wins: &HashMap<u32, Vec<Hwnd>>) -> Option<Hwnd> {
    let mut p = pid;
    for _ in 0..12 {
        if let Some(w) = wins.get(&p).and_then(|v| v.first()) {
            return Some(*w);
        }
        let console = tree.iter().find(|(c, (parent, exe))| *parent == p && matches!(exe.as_str(), "conhost.exe" | "openconsole.exe") && wins.contains_key(c));
        if let Some((c, _)) = console {
            return wins[c].first().copied();
        }
        let (parent, _) = tree.get(&p)?;
        // Past the shell's launcher: explorer's windows are not this session's.
        if tree.get(parent).map_or(true, |(_, exe)| exe == "explorer.exe") {
            return None;
        }
        p = *parent;
    }
    None
}

pub fn foreground() -> Hwnd {
    unsafe { GetForegroundWindow() }.0 as isize
}

/// Restore + foreground. A bare SetForegroundWindow from a background process only
/// flashes the taskbar, so borrow the foreground thread's input state meanwhile.
pub fn bring_to_front(hwnd: Hwnd) {
    unsafe {
        let me = GetCurrentThreadId();
        let fg = GetForegroundWindow();
        let fg_thread = if fg.0.is_null() { 0 } else { GetWindowThreadProcessId(fg, None) };
        let attached = fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool();
        if IsIconic(h(hwnd)).as_bool() {
            let _ = ShowWindow(h(hwnd), SW_RESTORE);
        }
        let _ = BringWindowToTop(h(hwnd));
        let _ = SetForegroundWindow(h(hwnd));
        if attached {
            let _ = AttachThreadInput(me, fg_thread, false);
        }
    }
}

/// Call `on_change` whenever a file in `dir` is written, created or removed (blocks forever).
/// Returns if the directory can't be watched (e.g. it doesn't exist yet).
pub fn watch_dir(dir: &std::path::Path, mut on_change: impl FnMut()) {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::WAIT_OBJECT_0;
    use windows::Win32::Storage::FileSystem::{FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE};
    use windows::Win32::System::Threading::{WaitForSingleObject, INFINITE};
    let Ok(handle) = (unsafe { FindFirstChangeNotificationW(&HSTRING::from(dir.as_os_str()), false, FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME) }) else { return };
    while unsafe { WaitForSingleObject(handle, INFINITE) } == WAIT_OBJECT_0 {
        // Claude Code writes several fields in quick succession; settle before reading.
        std::thread::sleep(std::time::Duration::from_millis(100));
        on_change();
        if unsafe { FindNextChangeNotification(handle) }.is_err() {
            break;
        }
    }
    let _ = unsafe { FindCloseChangeNotification(handle) };
}

#[cfg(test)]
#[test]
fn watch_dir_fires_on_write() {
    let dir = std::env::temp_dir().join(format!("claude-watch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let d = dir.clone();
    std::thread::spawn(move || watch_dir(&d, move || {
        let _ = tx.send(());
    }));
    std::thread::sleep(std::time::Duration::from_millis(200));
    std::fs::write(dir.join("123.json"), "{}").unwrap();
    assert!(rx.recv_timeout(std::time::Duration::from_secs(3)).is_ok(), "no change notification");
    let _ = std::fs::remove_dir_all(&dir);
}
