//! System actions through OS entry points instead of keyboard shortcuts.
//!
//! Rule (see CLAUDE.md): a keystroke is the last resort. Shortcuts can be
//! remapped, turned off, or captured by the app in front; an OS call can't.
//!
//! | Action | Route |
//! |---|---|
//! | Mission Control / App Exposé / Show Desktop | `CoreDockSendNotification` (what the Dock's own triggers use) |
//! | Lock Screen | `SACLockScreenImmediate` (login.framework; what the  menu uses) |
//! | Emoji & Symbols | Text Input Sources: select `com.apple.CharacterPaletteIM` |
//! | Hide Others | Accessibility `AXHidden` on every other app |
//! | Paste | Accessibility: press the front app's Edit ▸ Paste menu item |
//! | Spaces ←/→, Spotlight | No API exists. The **user's own** symbolic hotkey is read from `com.apple.symbolichotkeys` and posted exactly; WindowServer consumes symbolic hotkeys before any app sees them, so the app in front can't capture them. |
//!
//! Private symbols are resolved with `dlsym` at call time, so a future macOS
//! that drops one gets a clear error (or the documented fallback), never a
//! crash at launch.

use crate::ax::{self, Elem};
use knobdeck_sdk::{log, Error, Result};
use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::propertylist::CFPropertyList;
use core_foundation::string::{CFString, CFStringRef};
use std::ffi::{c_void, CStr};

fn sym(lib: &CStr, name: &CStr) -> Option<*mut c_void> {
    let h = unsafe { libc::dlopen(lib.as_ptr(), libc::RTLD_LAZY) };
    if h.is_null() {
        return None;
    }
    let p = unsafe { libc::dlsym(h, name.as_ptr()) };
    (!p.is_null()).then_some(p)
}

// ── Dock (Mission Control, App Exposé, Show Desktop) ─────────────────────────

pub enum Dock {
    MissionControl,
    AppExpose,
    ShowDesktop,
}

/// Ask the Dock to run Mission Control / App Exposé / Show Desktop directly.
pub fn dock(which: Dock) -> Result {
    let name = match which {
        Dock::MissionControl => "com.apple.expose.awake",
        Dock::AppExpose => "com.apple.expose.front.awake",
        Dock::ShowDesktop => "com.apple.showdesktop.awake",
    };
    let f = sym(c"/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices", c"CoreDockSendNotification")
        .ok_or_else(|| Error::msg("This macOS no longer offers the Dock trigger Mission Control uses"))?;
    let f: extern "C" fn(CFStringRef, i32) -> i32 = unsafe { std::mem::transmute(f) };
    let n = CFString::new(name);
    match f(n.as_concrete_TypeRef(), 0) {
        0 => Ok(()),
        e => Err(Error::msg(format!("The Dock refused {name} (error {e})"))),
    }
}

// ── Lock Screen ──────────────────────────────────────────────────────────────

pub fn lock_screen() -> Result {
    let f = sym(c"/System/Library/PrivateFrameworks/login.framework/Versions/Current/login", c"SACLockScreenImmediate")
        .ok_or_else(|| Error::msg("This macOS no longer offers the lock-screen call"))?;
    let f: extern "C" fn() -> i32 = unsafe { std::mem::transmute(f) };
    match f() {
        0 => Ok(()),
        e => Err(Error::msg(format!("Lock Screen failed (error {e})"))),
    }
}

// ── Emoji & Symbols (Character Viewer) ───────────────────────────────────────

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn TISCreateInputSourceList(props: CFDictionaryRef, include_all: u8) -> CFArrayRef;
    fn TISSelectInputSource(source: *const c_void) -> i32;
    static kTISPropertyInputSourceID: CFStringRef;
}

/// Open the Character Viewer, as Edit ▸ Emoji & Symbols does.
pub fn character_viewer() -> Result {
    let key = unsafe { CFString::wrap_under_get_rule(kTISPropertyInputSourceID) };
    let filter = CFDictionary::from_CFType_pairs(&[(key, CFString::from_static_string("com.apple.CharacterPaletteIM"))]);
    let raw = unsafe { TISCreateInputSourceList(filter.as_concrete_TypeRef(), 1) };
    if raw.is_null() {
        return Err(Error::msg("The Character Viewer input source is not available"));
    }
    let list: CFArray<*const c_void> = unsafe { CFArray::wrap_under_create_rule(raw) };
    let src = list.iter().next().map(|p| *p).ok_or_else(|| Error::msg("The Character Viewer input source is not available"))?;
    match unsafe { TISSelectInputSource(src) } {
        0 => Ok(()),
        e => Err(Error::msg(format!("Couldn't open the Character Viewer (error {e})"))),
    }
}

// ── Hide Others ──────────────────────────────────────────────────────────────

/// Hide every app with windows except the one in front (and KnobDeck).
pub fn hide_others() -> Result {
    let front = ax::focused_app()?.pid();
    for pid in crate::windows::window_app_pids() {
        if Some(pid) != front && !crate::windows::is_knobdeck_pid(pid) {
            let _ = Elem::app(pid).set_bool("AXHidden", true);
        }
    }
    Ok(())
}

// ── Paste ────────────────────────────────────────────────────────────────────

/// Press the front app's own Paste command: the menu item bound to ⌘V
/// (whatever its title or language), else one titled Paste. Falls back to ⌘V
/// only for apps without such a menu item.
pub fn paste() -> Result {
    match ax::focused_app().ok().and_then(|app| app.find_menu_item(|i| i.cmd_char().as_deref() == Some("V") && i.cmd_modifiers() == Some(0), "Paste")) {
        Some(item) => item.perform("AXPress"),
        None => {
            log("paste: no Paste menu item in the front app; sending ⌘V");
            knobdeck_sdk::send_keys("cmd v")
        }
    }
}

// ── The user's own system shortcuts (no API exists) ──────────────────────────

/// `com.apple.symbolichotkeys` ids.
#[derive(Clone, Copy)]
pub enum Hotkey {
    SpaceLeft = 79,
    SpaceRight = 81,
    Spotlight = 64,
}

impl Hotkey {
    /// macOS defaults (keycode, modifier mask) when the user never changed it.
    fn default_binding(self) -> (u16, u64) {
        match self {
            Hotkey::SpaceLeft => (123, 0x84_0000),  // ⌃← (+Fn, as the keyboard sends it)
            Hotkey::SpaceRight => (124, 0x84_0000), // ⌃→
            Hotkey::Spotlight => (49, 0x10_0000),   // ⌘Space
        }
    }

    fn name(self) -> &'static str {
        match self {
            Hotkey::SpaceLeft => "Move left a space",
            Hotkey::SpaceRight => "Move right a space",
            Hotkey::Spotlight => "Show Spotlight search",
        }
    }
}

extern "C" {
    fn CFPreferencesCopyAppValue(key: CFStringRef, app: CFStringRef) -> *const c_void;
}

/// `Ok(Some((keycode, mask)))`, `Ok(None)` when the user turned the shortcut off.
fn binding(hk: Hotkey) -> Option<(u16, u64)> {
    let raw = unsafe { CFPreferencesCopyAppValue(CFString::from_static_string("AppleSymbolicHotKeys").as_concrete_TypeRef(), CFString::from_static_string("com.apple.symbolichotkeys").as_concrete_TypeRef()) };
    if raw.is_null() {
        return Some(hk.default_binding());
    }
    let all = unsafe { CFPropertyList::wrap_under_create_rule(raw) };
    let Some(all) = all.downcast_into::<CFDictionary>() else { return Some(hk.default_binding()) };
    let all: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_get_rule(all.as_concrete_TypeRef()) };
    let Some(entry) = all.find(CFString::new(&(hk as i32).to_string())).and_then(|v| v.downcast::<CFDictionary>()) else {
        return Some(hk.default_binding());
    };
    let entry: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_get_rule(entry.as_concrete_TypeRef()) };
    let enabled = entry.find(CFString::from_static_string("enabled")).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i64());
    if enabled == Some(0) {
        return None;
    }
    let params = entry
        .find(CFString::from_static_string("value"))
        .and_then(|v| v.downcast::<CFDictionary>())
        .map(|v| unsafe { CFDictionary::<CFString, CFType>::wrap_under_get_rule(v.as_concrete_TypeRef()) })
        .and_then(|v| v.find(CFString::from_static_string("parameters")).map(|p| p.clone()))
        .and_then(|p| p.downcast::<CFArray>())
        .map(|a| unsafe { CFArray::<CFType>::wrap_under_get_rule(a.as_concrete_TypeRef()) });
    let nums: Vec<i64> = params.map(|a| a.iter().filter_map(|v| v.downcast::<CFNumber>().and_then(|n| n.to_i64())).collect()).unwrap_or_default();
    match nums.as_slice() {
        [_, code, mask, ..] if *code >= 0 && *code < 0xFFFF => Some((*code as u16, *mask as u64)),
        _ => Some(hk.default_binding()),
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreateKeyboardEvent(src: *const c_void, key: u16, down: bool) -> *mut c_void;
    fn CGEventSetFlags(ev: *mut c_void, flags: u64);
    fn CGEventSetIntegerValueField(ev: *mut c_void, field: u32, value: i64);
    fn CGEventPost(tap: u32, ev: *mut c_void);
    fn CGEventKeyboardSetUnicodeString(ev: *mut c_void, len: usize, chars: *const u16);
}

/// Type `text` into whatever has focus. A keystroke on purpose: the action *is* typing.
/// Each character rides on a key event as a Unicode string (layout-independent); line
/// breaks press Return (keycode 36), which apps expect instead of a U+000A character.
pub fn type_text(text: &str) -> Result {
    ax::ensure_trusted()?;
    for c in text.chars().filter(|&c| c != '\r') {
        let mut buf = [0u16; 2];
        let units = c.encode_utf16(&mut buf);
        for down in [true, false] {
            unsafe {
                let ev = CGEventCreateKeyboardEvent(std::ptr::null(), if c == '\n' { 36 } else { 0 }, down);
                if ev.is_null() {
                    return Err(Error::msg("Couldn't create the key event"));
                }
                // No modifier flags: a held ⌘/⌥ (e.g. from the dial's trigger) must not turn letters into shortcuts.
                CGEventSetFlags(ev, 0);
                if c != '\n' {
                    CGEventKeyboardSetUnicodeString(ev, units.len(), units.as_ptr());
                }
                CGEventSetIntegerValueField(ev, FIELD_SOURCE_USER_DATA, INJECTED_INPUT_TAG);
                CGEventPost(0, ev); // kCGHIDEventTap
                core_foundation::base::CFRelease(ev);
            }
        }
        // Apps drop characters when events arrive faster than their run loop drains them.
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Ok(())
}

/// KnobDeck's tag for its own injected input (`knobdeck_core::action::INJECTED_INPUT_TAG`),
/// so KnobDeck's key listener ignores these events.
const INJECTED_INPUT_TAG: i64 = 0x4B4E_4F42;
const FIELD_SOURCE_USER_DATA: u32 = 42;

/// Trigger a system shortcut the way the user configured it (or the macOS default).
pub fn system_hotkey(hk: Hotkey) -> Result {
    ax::ensure_trusted()?;
    let (code, mask) = binding(hk).ok_or_else(|| Error::msg(format!("\"{}\" is turned off in System Settings → Keyboard → Keyboard Shortcuts", hk.name())))?;
    for down in [true, false] {
        unsafe {
            let ev = CGEventCreateKeyboardEvent(std::ptr::null(), code, down);
            if ev.is_null() {
                return Err(Error::msg("Couldn't create the key event"));
            }
            // Arrows also carry NumericPad, as a hardware keyboard sends them.
            let numpad = if (123..=126).contains(&code) { 0x20_0000 } else { 0 };
            CGEventSetFlags(ev, mask | numpad);
            CGEventSetIntegerValueField(ev, FIELD_SOURCE_USER_DATA, INJECTED_INPUT_TAG);
            CGEventPost(0, ev); // kCGHIDEventTap
            core_foundation::base::CFRelease(ev);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_entry_points_resolve() {
        assert!(sym(c"/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices", c"CoreDockSendNotification").is_some());
        assert!(sym(c"/System/Library/PrivateFrameworks/login.framework/Versions/Current/login", c"SACLockScreenImmediate").is_some());
    }

    #[test]
    fn hotkey_bindings_read_or_default() {
        for hk in [Hotkey::SpaceLeft, Hotkey::SpaceRight, Hotkey::Spotlight] {
            let b = binding(hk);
            eprintln!("{}: {b:?}", hk.name());
            if let Some((code, _)) = b {
                assert!(code < 128);
            }
        }
        // Untouched defaults on this machine.
        assert_eq!(binding(Hotkey::SpaceLeft), Some((123, 0x84_0000)));
    }

    /// Finds (does not press) the front app's Paste item: `cargo test -p mac-tools -- --ignored`.
    #[test]
    #[ignore]
    fn live_finds_paste_menu_item() {
        let app = ax::focused_app().expect("focused app");
        let item = app.find_menu_item(|i| i.cmd_char().as_deref() == Some("V") && i.cmd_modifiers() == Some(0), "Paste").expect("Paste item");
        eprintln!("found: {:?} ({:?} mods {:?})", item.title(), item.cmd_char(), item.cmd_modifiers());
        assert_eq!(item.cmd_char().as_deref(), Some("V"));
    }
}
