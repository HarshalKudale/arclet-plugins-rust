//! Windows Core Audio (WASAPI).
//!
//! Per-application volume: sessions are matched to "an app" by exe file name, not raw PID:
//! multi-process apps (browsers, Discord…) spread audio over several processes sharing one exe
//! name, and grouping by name mirrors Windows' own Volume Mixer.
//!
//! Devices: list playback/recording devices, switch the default, and set a device's volume and mute.

use crate::win32;
use std::ffi::c_void;
use windows::core::{Interface, IUnknown, GUID, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eMultimedia, eRender, EDataFlow, ERole, IAudioSessionControl2, IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ};

type R<T> = knobdeck_sdk::Result<T>;

fn werr(what: &str, e: windows::core::Error) -> knobdeck_sdk::Error {
    knobdeck_sdk::Error::msg(format!("{what} failed: {e}"))
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

// ── Audio devices (endpoints) ────────────────────────────────────────────────

/// An active playback (`input = false`) or recording (`input = true`) device.
#[derive(Clone, Debug)]
pub struct Device {
    pub id: String,
    pub name: String,
}

fn flow(input: bool) -> EDataFlow {
    if input {
        eCapture
    } else {
        eRender
    }
}

fn enumerator() -> R<IMMDeviceEnumerator> {
    ensure_com();
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(|e| werr("CoCreateInstance(MMDeviceEnumerator)", e))
}

/// Read a COM-allocated wide string and free it.
fn take_pwstr(p: PWSTR) -> String {
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

fn device_id(d: &IMMDevice) -> Option<String> {
    unsafe { d.GetId() }.ok().map(take_pwstr)
}

fn friendly_name(d: &IMMDevice) -> Option<String> {
    let store = unsafe { d.OpenPropertyStore(STGM_READ) }.ok()?;
    let value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }.ok()?;
    unsafe { PropVariantToStringAlloc(&value) }.ok().map(take_pwstr)
}

/// Active devices for one direction, in Windows' order.
pub fn devices(input: bool) -> R<Vec<Device>> {
    let list = unsafe { enumerator()?.EnumAudioEndpoints(flow(input), DEVICE_STATE_ACTIVE) }.map_err(|e| werr("EnumAudioEndpoints", e))?;
    let count = unsafe { list.GetCount() }.map_err(|e| werr("IMMDeviceCollection::GetCount", e))?;
    Ok((0..count)
        .filter_map(|i| unsafe { list.Item(i) }.ok())
        .filter_map(|d| {
            let id = device_id(&d)?;
            let name = friendly_name(&d).unwrap_or_else(|| id.clone());
            Some(Device { id, name })
        })
        .collect())
}

/// Id of the default device for `role` (`None` when there is no device at all).
pub fn default_device(input: bool, role: ERole) -> Option<String> {
    device_id(&unsafe { enumerator().ok()?.GetDefaultAudioEndpoint(flow(input), role) }.ok()?)
}

/// Whether `id` is the current default for the roles a switch would set.
pub fn is_default(input: bool, id: &str, roles: Roles) -> bool {
    default_device(input, roles.probe()).is_some_and(|d| d == id)
}

/// Which Windows default the switch sets: everything, or only one of "Default device" / "Default communication device".
#[derive(Clone, Copy, PartialEq)]
pub enum Roles {
    All,
    Default,
    Communications,
}

impl Roles {
    pub fn parse(s: &str) -> Self {
        match s {
            "default" => Roles::Default,
            "communications" => Roles::Communications,
            _ => Roles::All,
        }
    }
    fn list(self) -> &'static [ERole] {
        match self {
            Roles::All => &[eConsole, eMultimedia, eCommunications],
            Roles::Default => &[eConsole, eMultimedia],
            Roles::Communications => &[eCommunications],
        }
    }
    /// The role whose current default decides whether a device "is selected".
    pub fn probe(self) -> ERole {
        if self == Roles::Communications {
            eCommunications
        } else {
            eConsole
        }
    }
}

// Windows has no public API to change the default device. Every switcher (Sound control panel,
// SoundSwitch, AudioDeviceCmdlets, EarTrumpet) uses the undocumented IPolicyConfig, unchanged
// since Windows 7. Only SetDefaultEndpoint is called, so its slot is all we declare.
const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
const IID_POLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
/// IUnknown (3) + GetMixFormat … SetPropertyValue (10) come before SetDefaultEndpoint.
const SET_DEFAULT_ENDPOINT_SLOT: usize = 13;
type SetDefaultEndpointFn = unsafe extern "system" fn(this: *mut c_void, id: PCWSTR, role: ERole) -> HRESULT;

pub fn set_default(id: &str, roles: Roles) -> R<()> {
    ensure_com();
    let unknown: IUnknown = unsafe { CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL) }.map_err(|e| werr("CoCreateInstance(PolicyConfig)", e))?;
    let mut raw = std::ptr::null_mut();
    unsafe { unknown.query(&IID_POLICY_CONFIG, &mut raw) }.ok().map_err(|e| werr("QueryInterface(IPolicyConfig)", e))?;
    // Owns the reference returned by QueryInterface, so it is released on drop.
    let policy = unsafe { IUnknown::from_raw(raw) };
    let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let vtable = *(policy.as_raw() as *const *const usize);
        let set: SetDefaultEndpointFn = std::mem::transmute(*vtable.add(SET_DEFAULT_ENDPOINT_SLOT));
        for &role in roles.list() {
            set(policy.as_raw(), PCWSTR(wide.as_ptr()), role).ok().map_err(|e| werr("IPolicyConfig::SetDefaultEndpoint", e))?;
        }
    }
    Ok(())
}

/// The device's volume control. `id` empty or `"__default__"` means the current default device.
fn endpoint(input: bool, id: &str) -> R<IAudioEndpointVolume> {
    let en = enumerator()?;
    let device = if id.is_empty() || id == "__default__" {
        unsafe { en.GetDefaultAudioEndpoint(flow(input), eConsole) }.map_err(|_| knobdeck_sdk::Error::msg(if input { "No microphone is connected" } else { "No playback device is connected" }))?
    } else {
        let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { en.GetDevice(PCWSTR(wide.as_ptr())) }.map_err(|_| knobdeck_sdk::Error::msg("That audio device is not connected"))?
    };
    unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|e| werr("IMMDevice::Activate(IAudioEndpointVolume)", e))
}

/// Volume 0–100 (rounded).
pub fn device_volume(input: bool, id: &str) -> R<i64> {
    let v = unsafe { endpoint(input, id)?.GetMasterVolumeLevelScalar() }.map_err(|e| werr("GetMasterVolumeLevelScalar", e))?;
    Ok((v * 100.0).round() as i64)
}

pub fn set_device_volume(input: bool, id: &str, percent: f64) -> R<()> {
    let level = (percent.clamp(0.0, 100.0) / 100.0) as f32;
    unsafe { endpoint(input, id)?.SetMasterVolumeLevelScalar(level, std::ptr::null()) }.map_err(|e| werr("SetMasterVolumeLevelScalar", e))
}

/// Nudge by `delta_percent`; returns the new volume.
pub fn adjust_device_volume(input: bool, id: &str, delta_percent: f64) -> R<i64> {
    let target = (device_volume(input, id)? as f64 + delta_percent).clamp(0.0, 100.0);
    set_device_volume(input, id, target)?;
    Ok(target.round() as i64)
}

pub fn device_muted(input: bool, id: &str) -> R<bool> {
    Ok(unsafe { endpoint(input, id)?.GetMute() }.map_err(|e| werr("IAudioEndpointVolume::GetMute", e))?.as_bool())
}

pub fn set_device_mute(input: bool, id: &str, muted: bool) -> R<()> {
    unsafe { endpoint(input, id)?.SetMute(muted, std::ptr::null()) }.map_err(|e| werr("IAudioEndpointVolume::SetMute", e))
}

/// Flip mute; returns the new state.
pub fn toggle_device_mute(input: bool, id: &str) -> R<bool> {
    let target = !device_muted(input, id)?;
    set_device_mute(input, id, target)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read-only: lists devices and reads the defaults' volume and mute without changing them.
    #[test]
    fn endpoints_list_and_read() {
        for input in [false, true] {
            let list = devices(input).expect("EnumAudioEndpoints");
            eprintln!("input={input}: {:?}", list.iter().map(|d| &d.name).collect::<Vec<_>>());
            let Some(def) = default_device(input, eConsole) else { continue };
            assert!(list.iter().any(|d| d.id == def), "default device is listed");
            let v = device_volume(input, &def).expect("volume of default");
            assert!((0..=100).contains(&v));
            assert_eq!(device_volume(input, "__default__").unwrap(), v);
            device_muted(input, &def).expect("mute of default");
        }
        assert!(device_volume(false, "{not-a-device}").is_err());
    }

    /// Switching the default needs a second device and changes the user's setup, so it is opt-in.
    /// It switches to another device and back.
    #[test]
    #[ignore]
    fn switch_default_and_back() {
        let list = devices(false).unwrap();
        let original = default_device(false, eConsole).unwrap();
        let other = list.iter().find(|d| d.id != original).expect("needs two playback devices");
        set_default(&other.id, Roles::Default).unwrap();
        assert_eq!(default_device(false, eConsole).as_deref(), Some(other.id.as_str()));
        set_default(&original, Roles::Default).unwrap();
        assert_eq!(default_device(false, eConsole).as_deref(), Some(original.as_str()));
    }

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
