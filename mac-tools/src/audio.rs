//! CoreAudio: list / switch the default output and input device, and mute the
//! microphone. (Per-app volume, which Win Tools offers, needs an audio driver
//! on macOS, so it is not part of Mac Tools.)

use knobdeck_sdk::{Error, Result};
use core_foundation::base::TCFType;
use core_foundation::string::{CFString, CFStringRef};
use std::ffi::c_void;
use std::sync::Mutex;

const fn fourcc(s: &[u8; 4]) -> u32 {
    ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | s[3] as u32
}

const SYSTEM_OBJECT: u32 = 1;
const DEVICES: u32 = fourcc(b"dev#");
const DEFAULT_OUTPUT: u32 = fourcc(b"dOut");
const DEFAULT_INPUT: u32 = fourcc(b"dIn ");
const DEFAULT_SYSTEM_OUTPUT: u32 = fourcc(b"sOut");
const NAME: u32 = fourcc(b"lnam");
const STREAMS: u32 = fourcc(b"stm#");
const MUTE: u32 = fourcc(b"mute");
const VOLUME: u32 = fourcc(b"volm");
const SCOPE_GLOBAL: u32 = fourcc(b"glob");
const SCOPE_INPUT: u32 = fourcc(b"inpt");
const SCOPE_OUTPUT: u32 = fourcc(b"outp");
const ELEMENT_MAIN: u32 = 0;

#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectHasProperty(obj: u32, addr: *const Address) -> u8;
    fn AudioObjectIsPropertySettable(obj: u32, addr: *const Address, settable: *mut u8) -> i32;
    fn AudioObjectGetPropertyDataSize(obj: u32, addr: *const Address, qsize: u32, qdata: *const c_void, size: *mut u32) -> i32;
    fn AudioObjectGetPropertyData(obj: u32, addr: *const Address, qsize: u32, qdata: *const c_void, size: *mut u32, data: *mut c_void) -> i32;
    fn AudioObjectSetPropertyData(obj: u32, addr: *const Address, qsize: u32, qdata: *const c_void, size: u32, data: *const c_void) -> i32;
}

fn addr(selector: u32, scope: u32, element: u32) -> Address {
    Address { selector, scope, element }
}

fn get<T: Copy + Default>(obj: u32, a: &Address) -> Option<T> {
    let mut v = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    let st = unsafe { AudioObjectGetPropertyData(obj, a, 0, std::ptr::null(), &mut size, &mut v as *mut T as *mut c_void) };
    (st == 0).then_some(v)
}

fn set<T: Copy>(obj: u32, a: &Address, v: T) -> Result {
    let st = unsafe { AudioObjectSetPropertyData(obj, a, 0, std::ptr::null(), std::mem::size_of::<T>() as u32, &v as *const T as *const c_void) };
    if st == 0 {
        Ok(())
    } else {
        Err(Error::msg(format!("CoreAudio refused the change (OSStatus {st})")))
    }
}

fn has(obj: u32, a: &Address) -> bool {
    unsafe { AudioObjectHasProperty(obj, a) != 0 }
}

fn settable(obj: u32, a: &Address) -> bool {
    let mut s = 0u8;
    has(obj, a) && unsafe { AudioObjectIsPropertySettable(obj, a, &mut s) } == 0 && s != 0
}

fn scope(input: bool) -> u32 {
    if input {
        SCOPE_INPUT
    } else {
        SCOPE_OUTPUT
    }
}

#[derive(Clone, Debug)]
pub struct Device {
    pub id: u32,
    pub name: String,
}

fn name(id: u32) -> String {
    let a = addr(NAME, SCOPE_GLOBAL, ELEMENT_MAIN);
    let mut s: CFStringRef = std::ptr::null();
    let mut size = std::mem::size_of::<CFStringRef>() as u32;
    let st = unsafe { AudioObjectGetPropertyData(id, &a, 0, std::ptr::null(), &mut size, &mut s as *mut _ as *mut c_void) };
    if st != 0 || s.is_null() {
        return format!("Device {id}");
    }
    unsafe { CFString::wrap_under_create_rule(s) }.to_string()
}

fn has_streams(id: u32, input: bool) -> bool {
    let a = addr(STREAMS, scope(input), ELEMENT_MAIN);
    let mut size = 0u32;
    unsafe { AudioObjectGetPropertyDataSize(id, &a, 0, std::ptr::null(), &mut size) == 0 && size > 0 }
}

/// Devices that can play (`input = false`) or record (`input = true`).
pub fn devices(input: bool) -> Vec<Device> {
    let a = addr(DEVICES, SCOPE_GLOBAL, ELEMENT_MAIN);
    let mut size = 0u32;
    if unsafe { AudioObjectGetPropertyDataSize(SYSTEM_OBJECT, &a, 0, std::ptr::null(), &mut size) } != 0 {
        return vec![];
    }
    let mut ids = vec![0u32; size as usize / 4];
    if unsafe { AudioObjectGetPropertyData(SYSTEM_OBJECT, &a, 0, std::ptr::null(), &mut size, ids.as_mut_ptr() as *mut c_void) } != 0 {
        return vec![];
    }
    ids.truncate(size as usize / 4);
    ids.into_iter().filter(|&id| has_streams(id, input)).map(|id| Device { id, name: name(id) }).collect()
}

pub fn default_device(input: bool) -> Option<u32> {
    get::<u32>(SYSTEM_OBJECT, &addr(if input { DEFAULT_INPUT } else { DEFAULT_OUTPUT }, SCOPE_GLOBAL, ELEMENT_MAIN)).filter(|&id| id != 0)
}

/// Make `id` the default device; outputs also become the alert/system-sound device.
pub fn set_default(input: bool, id: u32) -> Result {
    if input {
        return set(SYSTEM_OBJECT, &addr(DEFAULT_INPUT, SCOPE_GLOBAL, ELEMENT_MAIN), id);
    }
    set(SYSTEM_OBJECT, &addr(DEFAULT_OUTPUT, SCOPE_GLOBAL, ELEMENT_MAIN), id)?;
    let _ = set(SYSTEM_OBJECT, &addr(DEFAULT_SYSTEM_OUTPUT, SCOPE_GLOBAL, ELEMENT_MAIN), id);
    Ok(())
}

// ── Microphone mute ──

/// Input volume before a volume-based mute, per device (for mics without a mute control).
static SAVED_VOLUME: Mutex<Vec<(u32, f32)>> = Mutex::new(Vec::new());

/// Elements that carry the control: the main element, else channels 1 and 2.
fn control_elements(id: u32, selector: u32) -> Vec<u32> {
    if settable(id, &addr(selector, SCOPE_INPUT, ELEMENT_MAIN)) {
        return vec![ELEMENT_MAIN];
    }
    (1..=2).filter(|&ch| settable(id, &addr(selector, SCOPE_INPUT, ch))).collect()
}

fn mic() -> Result<u32> {
    default_device(true).ok_or_else(|| Error::msg("No microphone is selected"))
}

/// Whether the default microphone is muted (mute control on, or volume at 0).
pub fn mic_muted() -> Option<bool> {
    let id = default_device(true)?;
    if let Some(&el) = control_elements(id, MUTE).first() {
        return get::<u32>(id, &addr(MUTE, SCOPE_INPUT, el)).map(|m| m != 0);
    }
    let el = *control_elements(id, VOLUME).first()?;
    get::<f32>(id, &addr(VOLUME, SCOPE_INPUT, el)).map(|v| v <= 0.0001)
}

/// Flip the default microphone's mute; returns the new muted state.
pub fn toggle_mic_mute() -> Result<bool> {
    let id = mic()?;
    let target = !mic_muted().unwrap_or(false);
    let mutes = control_elements(id, MUTE);
    if !mutes.is_empty() {
        for el in mutes {
            set(id, &addr(MUTE, SCOPE_INPUT, el), target as u32)?;
        }
        return Ok(target);
    }
    // No mute control (many USB mics): mute by volume, restoring it afterwards.
    let vols = control_elements(id, VOLUME);
    if vols.is_empty() {
        return Err(Error::msg("This microphone can't be muted from software"));
    }
    let mut saved = SAVED_VOLUME.lock().unwrap();
    let level = if target {
        let cur = get::<f32>(id, &addr(VOLUME, SCOPE_INPUT, vols[0])).unwrap_or(1.0);
        saved.retain(|(d, _)| *d != id);
        saved.push((id, cur.max(0.05)));
        0.0
    } else {
        saved.iter().find(|(d, _)| *d == id).map_or(0.75, |(_, v)| *v)
    };
    for el in vols {
        set(id, &addr(VOLUME, SCOPE_INPUT, el), level)?;
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_outputs_with_names() {
        let outs = devices(false);
        eprintln!("outputs: {:?}\ninputs: {:?}\nmic muted: {:?}", outs, devices(true), mic_muted());
        if let Some(def) = default_device(false) {
            assert!(outs.iter().any(|d| d.id == def), "default output is listed");
        }
    }
}
