//! Per-application volume via the Windows Core Audio Session API (WASAPI).
//! Sessions are matched to "an app" by exe file name, not raw PID: multi-process
//! apps (browsers, Discord…) spread audio over several processes sharing one exe
//! name, and grouping by name mirrors Windows' own Volume Mixer.

use crate::win32;
use windows::core::Interface;
use windows::Win32::Media::Audio::{eConsole, eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

type R<T> = arclet_sdk::Result<T>;

fn werr(what: &str, e: windows::core::Error) -> arclet_sdk::Error {
    arclet_sdk::Error::msg(format!("{what} failed: {e}"))
}

thread_local! {
    static COM: () = {
        // S_OK / S_FALSE / RPC_E_CHANGED_MODE are all fine for our purposes; never uninitialised
        // (the thread, or the whole process, ends soon after).
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    };
}

/// Initialise COM on the calling thread (idempotent).
pub fn ensure_com() {
    COM.with(|_| {});
}

/// Visit every active audio session on the default playback endpoint:
/// `f(pid, exe_name, simple_volume)`. A single odd/expired session never aborts the walk.
fn for_each_session(mut f: impl FnMut(u32, Option<&str>, &ISimpleAudioVolume)) -> R<()> {
    ensure_com();
    let enumerator: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(|e| werr("CoCreateInstance(MMDeviceEnumerator)", e))?;
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }.map_err(|e| werr("GetDefaultAudioEndpoint", e))?;
    let manager: IAudioSessionManager2 = unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|e| werr("IMMDevice::Activate(IAudioSessionManager2)", e))?;
    let sessions = unsafe { manager.GetSessionEnumerator() }.map_err(|e| werr("GetSessionEnumerator", e))?;
    let names = win32::snapshot_process_names();
    let count = unsafe { sessions.GetCount() }.map_err(|e| werr("IAudioSessionEnumerator::GetCount", e))?;
    for i in 0..count {
        let Ok(control) = (unsafe { sessions.GetSession(i) }) else { continue };
        let Ok(control2) = control.cast::<IAudioSessionControl2>() else { continue };
        // S_OK means "yes, system sounds"; S_FALSE means no.
        if unsafe { control2.IsSystemSoundsSession() } == windows::Win32::Foundation::S_OK {
            continue;
        }
        // Errors (e.g. AUDCLNT_S_NO_SINGLE_PROCESS) → not attributable to one PID: skip.
        let Ok(pid) = (unsafe { control2.GetProcessId() }) else { continue };
        if pid == 0 {
            continue;
        }
        let Ok(volume) = control2.cast::<ISimpleAudioVolume>() else { continue };
        f(pid, names.get(&pid).map(String::as_str), &volume);
    }
    Ok(())
}

/// One entry per distinct exe name that has an active session.
pub fn list_session_apps() -> R<Vec<String>> {
    let mut seen: Vec<String> = vec![];
    for_each_session(|_, exe, _| {
        if let Some(e) = exe {
            if !seen.iter().any(|s| s.eq_ignore_ascii_case(e)) {
                seen.push(e.to_string());
            }
        }
    })?;
    Ok(seen)
}

/// Run `f` for each session of `exe_name` (case-insensitive); returns how many matched.
fn for_each_of_exe(exe_name: &str, mut f: impl FnMut(&ISimpleAudioVolume)) -> R<usize> {
    if exe_name.is_empty() {
        return Ok(0);
    }
    let mut matched = 0;
    for_each_session(|_, name, v| {
        if name.map_or(false, |n| n.eq_ignore_ascii_case(exe_name)) {
            matched += 1;
            f(v);
        }
    })?;
    Ok(matched)
}

fn set_master(v: &ISimpleAudioVolume, level: f32) {
    let _ = unsafe { v.SetMasterVolume(level, std::ptr::null()) };
}

fn master(v: &ISimpleAudioVolume) -> f32 {
    unsafe { v.GetMasterVolume() }.unwrap_or(0.0)
}

/// Set volume (0–100) on every session of the app; returns the number of sessions affected.
pub fn set_app_volume(exe: &str, percent: f64) -> R<usize> {
    let level = (percent.clamp(0.0, 100.0) / 100.0) as f32;
    for_each_of_exe(exe, |v| set_master(v, level))
}

/// Volume (0–100, rounded) of the first matching session, if any.
pub fn app_volume(exe: &str) -> R<Option<i64>> {
    let mut out = None;
    for_each_of_exe(exe, |v| {
        out.get_or_insert((master(v) * 100.0).round() as i64);
    })?;
    Ok(out)
}

/// Nudge by `delta_percent`; returns the resulting volume, or `None` if no session.
pub fn adjust_app_volume(exe: &str, delta_percent: f64) -> R<Option<i64>> {
    let mut out = None;
    for_each_of_exe(exe, |v| {
        let level = (master(v) + (delta_percent / 100.0) as f32).clamp(0.0, 1.0);
        set_master(v, level);
        out = Some((level * 100.0).round() as i64);
    })?;
    Ok(out)
}

/// Mute state of the first matching session, if any.
pub fn app_mute(exe: &str) -> R<Option<bool>> {
    let mut out = None;
    for_each_of_exe(exe, |v| {
        out.get_or_insert(unsafe { v.GetMute() }.map_or(false, |b| b.as_bool()));
    })?;
    Ok(out)
}

/// Toggle mute (based on the first session's state); returns the new state, or `None` if no session.
pub fn toggle_app_mute(exe: &str) -> R<Option<bool>> {
    let Some(cur) = app_mute(exe)? else { return Ok(None) };
    for_each_of_exe(exe, |v| {
        let _ = unsafe { v.SetMute(!cur, std::ptr::null()) };
    })?;
    Ok(Some(!cur))
}

/// Exe name that owns the current foreground window.
pub fn foreground_exe_name() -> R<Option<String>> {
    let hwnd = win32::foreground_window()?;
    Ok(win32::process_exe_name(win32::window_process_id(hwnd)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_enumeration_does_not_error() {
        let apps = list_session_apps().expect("WASAPI session enumeration");
        // Reading an app's volume must agree with whether it has a session.
        if let Some(first) = apps.first() {
            assert!(app_volume(first).unwrap().is_some());
        }
        assert_eq!(app_volume("definitely-not-running.exe").unwrap(), None);
    }
}
