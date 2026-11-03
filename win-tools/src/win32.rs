//! Win32 window management helpers. Every operation takes an optional window
//! handle (raw `isize`) defaulting to the foreground window, so the same code
//! drives the "current window" actions and the window-picker wheels.

use knobdeck_sdk::{Error, Result};
use std::collections::HashMap;
use std::ffi::c_void;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, BOOL, COLORREF, HWND, LPARAM, RECT, TRUE, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, HDC, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::*;

pub type Hwnd = isize;

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct MonitorInfoEx {
    pub monitor: Rect,
    pub work: Rect,
    pub primary: bool,
}

pub struct OpenWindow {
    pub hwnd: Hwnd,
    pub title: String,
    pub pid: u32,
    pub exe_name: Option<String>,
}

const WS_CAPTION: u32 = 0x00C0_0000;
const WS_BORDER: u32 = 0x0080_0000;
const WS_DLGFRAME: u32 = 0x0040_0000;
const WS_THICKFRAME: u32 = 0x0004_0000;
const WS_SYSMENU: u32 = 0x0008_0000;
const WS_MINIMIZEBOX: u32 = 0x0002_0000;
const WS_MAXIMIZEBOX: u32 = 0x0001_0000;
const WS_EX_TOPMOST: u32 = 0x0000_0008;
const WS_EX_LAYERED: u32 = 0x0008_0000;
const WS_EX_TRANSPARENT: u32 = 0x0000_0020;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const WS_EX_APPWINDOW: u32 = 0x0004_0000;

fn h(hwnd: Hwnd) -> HWND {
    HWND(hwnd as *mut c_void)
}

fn win_err(what: &str, e: windows::core::Error) -> Error {
    Error::msg(format!("{what} failed: {e}"))
}

// ── Low-level helpers ────────────────────────────────────────────────────────

pub fn foreground_window() -> Result<Hwnd> {
    let w = unsafe { GetForegroundWindow() };
    if w.0.is_null() {
        return Err(Error::msg("No foreground window"));
    }
    Ok(w.0 as isize)
}

fn target(hwnd: Option<Hwnd>) -> Result<Hwnd> {
    hwnd.map_or_else(foreground_window, Ok)
}

pub fn window_rect(hwnd: Hwnd) -> Result<Rect> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(h(hwnd), &mut r) }.map_err(|e| win_err("GetWindowRect", e))?;
    Ok(Rect { x: r.left, y: r.top, width: r.right - r.left, height: r.bottom - r.top })
}

pub fn window_process_id(hwnd: Hwnd) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(h(hwnd), Some(&mut pid)) };
    pid
}

fn style(hwnd: Hwnd) -> u32 {
    unsafe { GetWindowLongPtrW(h(hwnd), GWL_STYLE) as u32 }
}

fn ex_style(hwnd: Hwnd) -> u32 {
    unsafe { GetWindowLongPtrW(h(hwnd), GWL_EXSTYLE) as u32 }
}

// Sign-extend like a JS int32 -> intptr conversion would.
fn set_style(hwnd: Hwnd, v: u32) {
    unsafe { SetWindowLongPtrW(h(hwnd), GWL_STYLE, v as i32 as isize) };
}

fn set_ex_style(hwnd: Hwnd, v: u32) {
    unsafe { SetWindowLongPtrW(h(hwnd), GWL_EXSTYLE, v as i32 as isize) };
}

fn refresh_frame(hwnd: Hwnd) {
    let _ = unsafe { SetWindowPos(h(hwnd), HWND::default(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED) };
}

fn rect_of(r: RECT) -> Rect {
    Rect { x: r.left, y: r.top, width: r.right - r.left, height: r.bottom - r.top }
}

fn monitor_info(hm: HMONITOR) -> Result<MonitorInfoEx> {
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if !unsafe { GetMonitorInfoW(hm, &mut info) }.as_bool() {
        return Err(Error::msg("GetMonitorInfoW failed"));
    }
    Ok(MonitorInfoEx { monitor: rect_of(info.rcMonitor), work: rect_of(info.rcWork), primary: info.dwFlags & 1 != 0 })
}

// ── Monitors ─────────────────────────────────────────────────────────────────

unsafe extern "system" fn monitor_cb(hm: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let list = &mut *(data.0 as *mut Vec<MonitorInfoEx>);
    if let Ok(i) = monitor_info(hm) {
        list.push(i);
    }
    TRUE
}

/// Monitors sorted left-to-right, then top-to-bottom.
pub fn list_monitors() -> Vec<MonitorInfoEx> {
    let mut list: Vec<MonitorInfoEx> = vec![];
    let _ = unsafe { EnumDisplayMonitors(HDC::default(), None, Some(monitor_cb), LPARAM(&mut list as *mut _ as isize)) };
    list.sort_by_key(|m| (m.monitor.x, m.monitor.y));
    list
}

pub fn window_monitor(hwnd: Hwnd) -> Result<MonitorInfoEx> {
    monitor_info(unsafe { MonitorFromWindow(h(hwnd), MONITOR_DEFAULTTONEAREST) })
}

/// Monitor index string ("0", "1", …) or "current"/unknown → the window's own monitor.
pub fn resolve_monitor(hwnd: Hwnd, key: &str) -> Result<MonitorInfoEx> {
    if key.is_empty() || key == "current" {
        return window_monitor(hwnd);
    }
    match key.trim().parse::<usize>() {
        Ok(i) => match list_monitors().get(i) {
            Some(m) => Ok(*m),
            None => window_monitor(hwnd),
        },
        Err(_) => window_monitor(hwnd),
    }
}

// ── Processes ────────────────────────────────────────────────────────────────

/// pid → exe file name (e.g. "chrome.exe") for every running process.
pub fn snapshot_process_names() -> HashMap<u32, String> {
    let mut names = HashMap::new();
    let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else { return names };
    let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
    if unsafe { Process32FirstW(snap, &mut entry) }.is_ok() {
        loop {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            names.insert(entry.th32ProcessID, String::from_utf16_lossy(&entry.szExeFile[..len]));
            if unsafe { Process32NextW(snap, &mut entry) }.is_err() {
                break;
            }
        }
    }
    let _ = unsafe { CloseHandle(snap) };
    names
}

pub fn process_exe_name(pid: u32) -> Option<String> {
    snapshot_process_names().remove(&pid)
}

/// Full path of the exe owning `pid`.
pub fn process_exe_path(pid: u32) -> Option<String> {
    let proc = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 1024];
    let mut size = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(proc, PROCESS_NAME_FORMAT(0), PWSTR(buf.as_mut_ptr()), &mut size) }.is_ok();
    let _ = unsafe { CloseHandle(proc) };
    ok.then(|| String::from_utf16_lossy(&buf[..size as usize]))
}

// ── Open-window enumeration ──────────────────────────────────────────────────

fn window_title(hwnd: Hwnd) -> String {
    let len = unsafe { GetWindowTextLengthW(h(hwnd)) };
    if len == 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len as usize + 1];
    let n = unsafe { GetWindowTextW(h(hwnd), &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// DWM-cloaked windows (suspended UWP frames etc.) report visible but are invisible to the user.
fn is_cloaked(hwnd: Hwnd) -> bool {
    let mut cloaked = 0u32;
    unsafe { DwmGetWindowAttribute(h(hwnd), DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut c_void, 4) }.map_or(false, |_| cloaked != 0)
}

/// Visible, top-level, not owned, not a bare tool window, not cloaked — the shape the taskbar groups by.
fn is_open_window(hwnd: Hwnd) -> bool {
    if !unsafe { IsWindowVisible(h(hwnd)) }.as_bool() {
        return false;
    }
    if unsafe { GetWindow(h(hwnd), GW_OWNER) }.map_or(false, |o| !o.0.is_null()) {
        return false;
    }
    let ex = ex_style(hwnd);
    if ex & WS_EX_TOOLWINDOW != 0 && ex & WS_EX_APPWINDOW == 0 {
        return false;
    }
    !is_cloaked(hwnd)
}

unsafe extern "system" fn window_cb(hwnd: HWND, data: LPARAM) -> BOOL {
    (*(data.0 as *mut Vec<Hwnd>)).push(hwnd.0 as isize);
    TRUE
}

// KnobDeck's own helper windows (overlay, tray…) must not list themselves as switch targets.
// Dev builds run `knobdeck-app.exe`; installed Tauri builds are named after productName.
const KNOBDECK_EXE_NAMES: [&str; 2] = ["knobdeck-app.exe", "KnobDeck.exe"];
const KNOBDECK_MAIN_WINDOW_TITLE: &str = "KnobDeck — Wheel Editor";

/// Every open top-level window, in Z-order (topmost first).
pub fn list_open_windows() -> Vec<OpenWindow> {
    let mut hwnds: Vec<Hwnd> = vec![];
    let _ = unsafe { EnumWindows(Some(window_cb), LPARAM(&mut hwnds as *mut _ as isize)) };
    let names = snapshot_process_names();
    hwnds
        .into_iter()
        .filter(|&w| is_open_window(w))
        .filter_map(|hwnd| {
            let title = window_title(hwnd);
            if title.is_empty() {
                return None;
            }
            let pid = window_process_id(hwnd);
            let exe_name = names.get(&pid).cloned();
            let internal = exe_name.as_deref().map_or(false, |e| KNOBDECK_EXE_NAMES.iter().any(|n| e.eq_ignore_ascii_case(n))) && title != KNOBDECK_MAIN_WINDOW_TITLE;
            (!internal).then_some(OpenWindow { hwnd, title, pid, exe_name })
        })
        .collect()
}

/// Restore (if minimized) and foreground `hwnd`. A bare SetForegroundWindow from a
/// background process only flashes the taskbar button, so borrow the current
/// foreground thread's input state via AttachThreadInput for the duration.
pub fn bring_window_to_front(hwnd: Hwnd) {
    unsafe {
        let cur_thread = GetCurrentThreadId();
        let fg = GetForegroundWindow();
        let fg_thread = if fg.0.is_null() { 0 } else { GetWindowThreadProcessId(fg, None) };
        let attached = fg_thread != 0 && fg_thread != cur_thread && AttachThreadInput(cur_thread, fg_thread, true).as_bool();
        if IsIconic(h(hwnd)).as_bool() {
            let _ = ShowWindow(h(hwnd), SW_RESTORE);
        }
        let _ = BringWindowToTop(h(hwnd));
        let _ = SetForegroundWindow(h(hwnd));
        if attached {
            let _ = AttachThreadInput(cur_thread, fg_thread, false);
        }
    }
}

// ── High-level window operations ─────────────────────────────────────────────

pub fn minimize(hwnd: Option<Hwnd>) -> Result {
    let w = target(hwnd)?;
    let _ = unsafe { ShowWindow(h(w), SW_MINIMIZE) };
    Ok(())
}

pub fn maximize(hwnd: Option<Hwnd>) -> Result {
    let w = target(hwnd)?;
    let _ = unsafe { ShowWindow(h(w), SW_MAXIMIZE) };
    Ok(())
}

pub fn restore(hwnd: Option<Hwnd>) -> Result {
    let w = target(hwnd)?;
    let _ = unsafe { ShowWindow(h(w), SW_RESTORE) };
    Ok(())
}

pub fn close_win(hwnd: Option<Hwnd>) -> Result {
    let w = target(hwnd)?;
    let _ = unsafe { PostMessageW(h(w), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    Ok(())
}

pub fn bring_to_front(hwnd: Option<Hwnd>) -> Result {
    let w = target(hwnd)?;
    unsafe {
        let _ = BringWindowToTop(h(w));
        let _ = SetWindowPos(h(w), HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
    }
    Ok(())
}

/// Raise above other normal windows without taking focus (keeps cross-monitor moves visible).
fn bring_to_front_no_activate(hwnd: Hwnd) {
    let _ = unsafe { SetWindowPos(h(hwnd), HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW) };
}

pub fn send_to_back(hwnd: Option<Hwnd>) -> Result {
    let w = target(hwnd)?;
    let _ = unsafe { SetWindowPos(h(w), HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
    Ok(())
}

pub fn toggle_always_on_top(hwnd: Option<Hwnd>) -> Result<&'static str> {
    let w = target(hwnd)?;
    let topmost = ex_style(w) & WS_EX_TOPMOST != 0;
    let after = if topmost { HWND_NOTOPMOST } else { HWND_TOPMOST };
    let _ = unsafe { SetWindowPos(h(w), after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
    Ok(if topmost { "off" } else { "on" })
}

pub fn always_on_top_state(hwnd: Option<Hwnd>) -> Result<&'static str> {
    Ok(if ex_style(target(hwnd)?) & WS_EX_TOPMOST != 0 { "on" } else { "off" })
}

/// Original styles saved when a window goes borderless, so it can be restored exactly.
static SAVED_STYLES: std::sync::Mutex<Option<HashMap<Hwnd, u32>>> = std::sync::Mutex::new(None);

pub fn toggle_borderless(hwnd: Option<Hwnd>) -> Result<&'static str> {
    let w = target(hwnd)?;
    let s = style(w);
    let mut saved = SAVED_STYLES.lock().unwrap();
    let saved = saved.get_or_insert_with(HashMap::new);
    if s & (WS_CAPTION | WS_THICKFRAME) != 0 {
        saved.insert(w, s);
        set_style(w, s & !(WS_CAPTION | WS_THICKFRAME | WS_BORDER | WS_DLGFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX));
        refresh_frame(w);
        Ok("borderless")
    } else {
        set_style(w, saved.remove(&w).unwrap_or(s | WS_CAPTION | WS_THICKFRAME));
        refresh_frame(w);
        Ok("bordered")
    }
}

pub fn borderless_state(hwnd: Option<Hwnd>) -> Result<&'static str> {
    Ok(if style(target(hwnd)?) & (WS_CAPTION | WS_THICKFRAME) != 0 { "bordered" } else { "borderless" })
}

fn set_alpha(hwnd: Hwnd, alpha: u8) {
    let _ = unsafe { SetLayeredWindowAttributes(h(hwnd), COLORREF(0), alpha, LWA_ALPHA) };
}

pub fn toggle_transparency(alpha: u8, hwnd: Option<Hwnd>) -> Result<&'static str> {
    let w = target(hwnd)?;
    let ex = ex_style(w);
    if ex & WS_EX_LAYERED != 0 {
        set_ex_style(w, ex & !WS_EX_LAYERED);
        refresh_frame(w);
        Ok("opaque")
    } else {
        set_ex_style(w, ex | WS_EX_LAYERED);
        set_alpha(w, alpha);
        Ok("transparent")
    }
}

pub fn transparency_state(hwnd: Option<Hwnd>) -> Result<&'static str> {
    Ok(if ex_style(target(hwnd)?) & WS_EX_LAYERED != 0 { "transparent" } else { "opaque" })
}

pub fn set_opacity(percent: i64, hwnd: Option<Hwnd>) -> Result {
    let w = target(hwnd)?;
    let alpha = ((percent as f64 / 100.0) * 255.0).round().clamp(0.0, 255.0) as u8;
    let ex = ex_style(w);
    if ex & WS_EX_LAYERED == 0 {
        set_ex_style(w, ex | WS_EX_LAYERED);
    }
    set_alpha(w, alpha);
    Ok(())
}

pub fn toggle_click_through(hwnd: Option<Hwnd>) -> Result<&'static str> {
    let w = target(hwnd)?;
    let mut ex = ex_style(w);
    if ex & WS_EX_TRANSPARENT != 0 {
        set_ex_style(w, ex & !WS_EX_TRANSPARENT);
        Ok("normal")
    } else {
        // WS_EX_TRANSPARENT only works on a layered window.
        if ex & WS_EX_LAYERED == 0 {
            ex |= WS_EX_LAYERED;
            set_alpha(w, 255);
        }
        set_ex_style(w, ex | WS_EX_TRANSPARENT);
        Ok("clickthrough")
    }
}

pub fn click_through_state(hwnd: Option<Hwnd>) -> Result<&'static str> {
    Ok(if ex_style(target(hwnd)?) & WS_EX_TRANSPARENT != 0 { "clickthrough" } else { "normal" })
}

pub fn toggle_maximize_restore(hwnd: Option<Hwnd>) -> Result<&'static str> {
    let w = target(hwnd)?;
    if unsafe { IsZoomed(h(w)) }.as_bool() {
        let _ = unsafe { ShowWindow(h(w), SW_RESTORE) };
        Ok("normal")
    } else {
        let _ = unsafe { ShowWindow(h(w), SW_MAXIMIZE) };
        Ok("maximized")
    }
}

pub fn maximize_restore_state(hwnd: Option<Hwnd>) -> Result<&'static str> {
    Ok(if unsafe { IsZoomed(h(target(hwnd)?)) }.as_bool() { "maximized" } else { "normal" })
}

/// Apply real window bounds. A minimized/maximized window is restored first so the
/// geometry applies to the actual window; then it is raised without stealing focus.
pub fn set_window_bounds(hwnd: Hwnd, x: i32, y: i32, width: i32, height: i32) {
    unsafe {
        if IsIconic(h(hwnd)).as_bool() || IsZoomed(h(hwnd)).as_bool() {
            let _ = ShowWindow(h(hwnd), SW_RESTORE);
        }
        let _ = SetWindowPos(h(hwnd), HWND::default(), x, y, width, height, SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
    bring_to_front_no_activate(hwnd);
}

/// Move to the next monitor (left-to-right, wrapping), keeping the relative position.
pub fn move_to_next_monitor(hwnd: Option<Hwnd>) -> Result {
    let w = target(hwnd)?;
    let monitors = list_monitors();
    if monitors.len() <= 1 {
        return Ok(());
    }
    let cur = window_monitor(w)?;
    let idx = monitors.iter().position(|m| m.monitor.x == cur.monitor.x && m.monitor.y == cur.monitor.y).unwrap_or(0);
    let next = monitors[(idx + 1) % monitors.len()];
    let r = window_rect(w)?;
    set_window_bounds(w, next.monitor.x + (r.x - cur.monitor.x), next.monitor.y + (r.y - cur.monitor.y), r.width, r.height);
    Ok(())
}

/// Center on the monitor work area, keeping the window size.
pub fn center_window(hwnd: Hwnd, work: Rect) -> Result {
    let r = window_rect(hwnd)?;
    set_window_bounds(hwnd, work.x + (work.width - r.width).div_euclid(2), work.y + (work.height - r.height).div_euclid(2), r.width, r.height);
    Ok(())
}

/// Region as fractions (0..1) of the work area.
#[derive(Clone, Copy)]
pub struct Region {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

pub fn move_to_region(hwnd: Hwnd, work: Rect, r: Region) {
    let f = |total: i32, frac: f64| (total as f64 * frac).floor() as i32;
    set_window_bounds(hwnd, work.x + f(work.width, r.x), work.y + f(work.height, r.y), f(work.width, r.w), f(work.height, r.h));
}

/// Open a shell target (URI, `shell:::{CLSID}` object, or program) the way
/// Explorer does: `ShellExecuteW(open)`. Handles UAC for elevated programs
/// (Task Manager), which a plain CreateProcess refuses.
pub fn shell_open(target: &str) -> Result {
    use windows::core::{w, HSTRING};
    use windows::Win32::UI::Shell::ShellExecuteW;
    let r = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(target), None, None, SW_SHOWNORMAL) };
    // ShellExecute returns a value > 32 on success.
    if r.0 as isize > 32 {
        Ok(())
    } else {
        Err(Error::msg(format!("Couldn't open {target} (error {})", r.0 as isize)))
    }
}

pub fn lock_workstation() -> Result {
    unsafe { windows::Win32::System::Shutdown::LockWorkStation() }.map_err(|e| win_err("LockWorkStation", e))
}

/// Cover the entire monitor, taskbar included.
pub fn make_fullscreen(hwnd: Hwnd, m: Rect) {
    set_window_bounds(hwnd, m.x, m.y, m.width, m.height);
}

#[cfg(test)]
mod tests {
    use super::*;

    // Read-only smoke tests against the live desktop session.
    #[test]
    fn monitors_processes_and_windows() {
        let monitors = list_monitors();
        assert!(!monitors.is_empty() && monitors.iter().any(|m| m.primary));
        assert!(snapshot_process_names().values().any(|n| n.eq_ignore_ascii_case("explorer.exe")));
        let windows = list_open_windows();
        assert!(!windows.is_empty(), "expected at least one open window");
        assert!(process_exe_path(windows[0].pid).is_some());
        assert!(resolve_monitor(windows[0].hwnd, "current").is_ok());
    }
}
