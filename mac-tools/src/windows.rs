//! Open-window listing (for the switcher and picker wheels) and per-app info.
//!
//! CGWindowList gives the apps that own normal windows, front to back, without
//! Screen Recording (only owner pids are read, never titles). Each app's windows
//! and titles then come from Accessibility, which also covers minimised windows.

use crate::ax::Elem;
use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use std::ffi::c_void;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFArrayRef;
}

const ON_SCREEN_ONLY: u32 = 1;
const EXCLUDE_DESKTOP: u32 = 16;

pub struct OpenWindow {
    pub elem: Elem,
    pub title: String,
    pub app: AppInfo,
}

#[derive(Clone, Debug)]
pub struct AppInfo {
    pub pid: i32,
    pub name: String,
    /// `/Applications/X.app`, for its icon.
    pub bundle: Option<String>,
}

/// pids owning layer-0 windows, front to back; `on_screen` = visible ones only.
fn window_owner_pids(on_screen: bool) -> Vec<i32> {
    let opts = EXCLUDE_DESKTOP | if on_screen { ON_SCREEN_ONLY } else { 0 };
    let raw = unsafe { CGWindowListCopyWindowInfo(opts, 0) };
    if raw.is_null() {
        return vec![];
    }
    let arr: CFArray<CFDictionary<CFString, CFType>> = unsafe { CFArray::wrap_under_create_rule(raw) };
    let (k_pid, k_layer) = (CFString::from_static_string("kCGWindowOwnerPID"), CFString::from_static_string("kCGWindowLayer"));
    let num = |d: &CFDictionary<CFString, CFType>, k: &CFString| d.find(k).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i64());
    let mut pids = vec![];
    for d in arr.iter() {
        if num(&d, &k_layer) != Some(0) {
            continue;
        }
        if let Some(pid) = num(&d, &k_pid).map(|p| p as i32) {
            if !pids.contains(&pid) {
                pids.push(pid);
            }
        }
    }
    pids
}

fn exe_path(pid: i32) -> Option<String> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf8(buf).ok()
}

/// The outermost `.app` bundle containing `path`.
fn bundle_of(path: &str) -> Option<String> {
    path.find(".app/").map(|i| path[..i + 4].to_string())
}

pub fn app_info(pid: i32) -> AppInfo {
    let bundle = exe_path(pid).as_deref().and_then(bundle_of);
    let name = bundle
        .as_deref()
        .and_then(|b| std::path::Path::new(b).file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .or_else(|| exe_path(pid).and_then(|p| std::path::Path::new(&p).file_name().map(|n| n.to_string_lossy().into_owned())))
        .unwrap_or_else(|| format!("pid {pid}"));
    AppInfo { pid, name, bundle }
}

/// KnobDeck's own dial / config windows are never switch targets.
fn is_knobdeck(app: &AppInfo) -> bool {
    app.pid == std::process::id() as i32 || app.name.eq_ignore_ascii_case("knobdeck") || app.name.to_ascii_lowercase().starts_with("knobdeck")
}

/// Every app that owns a normal window (on screen or not).
pub fn window_app_pids() -> Vec<i32> {
    window_owner_pids(false)
}

pub fn is_knobdeck_pid(pid: i32) -> bool {
    is_knobdeck(&app_info(pid))
}

/// The app owning the frontmost on-screen normal window (not KnobDeck's).
pub fn frontmost_app_pid() -> Option<i32> {
    window_owner_pids(true).into_iter().find(|&pid| !is_knobdeck_pid(pid) && !Elem::app(pid).windows().is_empty())
}

/// Every standard window, frontmost app first (minimised windows included).
pub fn list_open_windows() -> Vec<OpenWindow> {
    let mut pids = window_owner_pids(true);
    for p in window_owner_pids(false) {
        if !pids.contains(&p) {
            pids.push(p);
        }
    }
    let mut out = vec![];
    for pid in pids {
        let app = app_info(pid);
        if is_knobdeck(&app) {
            continue;
        }
        for elem in Elem::app(pid).windows() {
            let title = elem.title();
            out.push(OpenWindow { elem, title, app: app.clone() });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_of_takes_the_outermost_app() {
        assert_eq!(bundle_of("/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper.app/Contents/MacOS/Code Helper").as_deref(), Some("/Applications/Visual Studio Code.app"));
        assert_eq!(bundle_of("/bin/zsh"), None);
    }

    #[test]
    fn finder_or_some_app_owns_windows() {
        // A logged-in session always has at least the Dock/Finder/menu bar owners.
        let all = window_owner_pids(false);
        assert!(!all.is_empty());
        eprintln!("{:?}", all.iter().map(|p| app_info(*p).name).collect::<Vec<_>>());
    }
}
