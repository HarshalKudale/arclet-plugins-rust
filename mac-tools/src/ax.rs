//! Accessibility (AXUIElement) wrappers: the only public way to move, resize,
//! minimise, close and raise *other* apps' windows on macOS.
//!
//! Needs the Accessibility permission. Arclet already holds it, and a plugin
//! process inherits it from Arclet (the responsible process). Coordinates are
//! global points with a top-left origin on the primary display, the same space
//! `CGDisplayBounds` uses.

use arclet_sdk::{Error, Result};
use core_foundation::base::{CFEqual, CFGetTypeID, CFRelease, CFRetain, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::string::{CFString, CFStringRef};
use std::ffi::c_void;

type AXUIElementRef = *const c_void;
type AXValueRef = *const c_void;

const AX_OK: i32 = 0;
const AX_API_DISABLED: i32 = -25211;
const AX_VALUE_CGPOINT: u32 = 1;
const AX_VALUE_CGSIZE: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct CGSize {
    width: f64,
    height: f64,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(el: AXUIElementRef, attr: CFStringRef, value: *mut CFTypeRef) -> i32;
    fn AXUIElementSetAttributeValue(el: AXUIElementRef, attr: CFStringRef, value: CFTypeRef) -> i32;
    fn AXUIElementPerformAction(el: AXUIElementRef, action: CFStringRef) -> i32;
    fn AXUIElementGetPid(el: AXUIElementRef, pid: *mut i32) -> i32;
    fn AXUIElementSetMessagingTimeout(el: AXUIElementRef, seconds: f32) -> i32;
    fn AXUIElementGetTypeID() -> usize;
    fn AXValueCreate(kind: u32, value: *const c_void) -> AXValueRef;
    fn AXValueGetValue(v: AXValueRef, kind: u32, out: *mut c_void) -> bool;
}

fn attr(name: &'static str) -> CFString {
    CFString::from_static_string(name)
}

/// Fail early with a clear message instead of every call returning -25211.
pub fn ensure_trusted() -> Result {
    if unsafe { AXIsProcessTrusted() } {
        Ok(())
    } else {
        Err(Error::msg("Mac Tools needs the Accessibility permission: System Settings → Privacy & Security → Accessibility → turn on Arclet"))
    }
}

fn check(what: &str, code: i32) -> Result {
    match code {
        AX_OK => Ok(()),
        AX_API_DISABLED => ensure_trusted(),
        c => Err(Error::msg(format!("{what} failed (AXError {c})"))),
    }
}

/// An owned (+1) AXUIElement. AX calls are thread-safe; the handle is sent
/// between handler threads (window pickers).
pub struct Elem(AXUIElementRef);

unsafe impl Send for Elem {}
unsafe impl Sync for Elem {}

impl Drop for Elem {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

impl Clone for Elem {
    fn clone(&self) -> Self {
        unsafe { CFRetain(self.0) };
        Elem(self.0)
    }
}

impl PartialEq for Elem {
    fn eq(&self, other: &Self) -> bool {
        unsafe { CFEqual(self.0, other.0) != 0 }
    }
}

impl std::fmt::Debug for Elem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Elem({:p})", self.0)
    }
}

/// An owned CF value returned by a Copy call.
struct Owned(CFTypeRef);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

impl Elem {
    /// Take ownership of a +1 reference.
    fn from_owned(r: AXUIElementRef) -> Option<Elem> {
        // Lazily: an `Elem` of NULL would CFRelease(NULL) when dropped.
        (!r.is_null()).then(|| Elem(r))
    }

    pub fn app(pid: i32) -> Elem {
        let e = Elem(unsafe { AXUIElementCreateApplication(pid) });
        // A hung app would otherwise block each call for the 6 s default.
        unsafe { AXUIElementSetMessagingTimeout(e.0, 1.0) };
        e
    }

    fn copy(&self, name: &'static str) -> Option<Owned> {
        let mut v: CFTypeRef = std::ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(self.0, attr(name).as_concrete_TypeRef(), &mut v) };
        // Lazily: `then_some(Owned(v))` would build (and drop → CFRelease) an `Owned(NULL)`.
        (err == AX_OK && !v.is_null()).then(|| Owned(v))
    }

    fn elem_attr(&self, name: &'static str) -> Option<Elem> {
        let v = self.copy(name)?;
        if unsafe { CFGetTypeID(v.0) } != unsafe { AXUIElementGetTypeID() } {
            return None;
        }
        let r = v.0;
        std::mem::forget(v); // ownership moves into the Elem
        Elem::from_owned(r)
    }

    pub fn string(&self, name: &'static str) -> Option<String> {
        let v = self.copy(name)?;
        if unsafe { CFGetTypeID(v.0) } != CFString::type_id() {
            return None;
        }
        Some(unsafe { CFString::wrap_under_get_rule(v.0 as CFStringRef) }.to_string())
    }

    pub fn bool(&self, name: &'static str) -> Option<bool> {
        let v = self.copy(name)?;
        if unsafe { CFGetTypeID(v.0) } != CFBoolean::type_id() {
            return None;
        }
        Some(bool::from(unsafe { CFBoolean::wrap_under_get_rule(v.0 as _) }))
    }

    pub fn set_bool(&self, name: &'static str, value: bool) -> Result {
        let b = if value { CFBoolean::true_value() } else { CFBoolean::false_value() };
        check(name, unsafe { AXUIElementSetAttributeValue(self.0, attr(name).as_concrete_TypeRef(), b.as_CFTypeRef()) })
    }

    fn children(&self, name: &'static str) -> Vec<Elem> {
        use core_foundation::array::{CFArray, CFArrayRef};
        let Some(v) = self.copy(name) else { return vec![] };
        if unsafe { CFGetTypeID(v.0) } != CFArray::<*const c_void>::type_id() {
            return vec![];
        }
        let arr: CFArray<*const c_void> = unsafe { CFArray::wrap_under_get_rule(v.0 as CFArrayRef) };
        arr.iter()
            .filter(|e| unsafe { CFGetTypeID(**e) } == unsafe { AXUIElementGetTypeID() })
            .map(|e| {
                unsafe { CFRetain(*e) };
                Elem(*e)
            })
            .collect()
    }

    pub fn perform(&self, action: &'static str) -> Result {
        check(action, unsafe { AXUIElementPerformAction(self.0, attr(action).as_concrete_TypeRef()) })
    }

    fn number(&self, name: &'static str) -> Option<i64> {
        use core_foundation::number::CFNumber;
        let v = self.copy(name)?;
        if unsafe { CFGetTypeID(v.0) } != CFNumber::type_id() {
            return None;
        }
        unsafe { CFNumber::wrap_under_get_rule(v.0 as _) }.to_i64()
    }

    /// A menu item's shortcut key (`"V"` for ⌘V).
    pub fn cmd_char(&self) -> Option<String> {
        self.string("AXMenuItemCmdChar")
    }

    /// A menu item's shortcut modifiers (0 = ⌘ alone).
    pub fn cmd_modifiers(&self) -> Option<i64> {
        self.number("AXMenuItemCmdModifiers")
    }

    /// Search an app's menu bar for an enabled item: first by `matches`, then by
    /// `title`. Only the bar's menus and their direct items are walked.
    pub fn find_menu_item(&self, matches: impl Fn(&Elem) -> bool, title: &str) -> Option<Elem> {
        let bar = self.elem_attr("AXMenuBar")?;
        let mut items = vec![];
        for top in bar.children("AXChildren") {
            for menu in top.children("AXChildren") {
                items.extend(menu.children("AXChildren").into_iter().filter(|i| i.bool("AXEnabled") != Some(false)));
            }
        }
        let i = items.iter().position(&matches).or_else(|| items.iter().position(|i| i.title() == title))?;
        Some(items.swap_remove(i))
    }

    pub fn pid(&self) -> Option<i32> {
        let mut pid = 0;
        (unsafe { AXUIElementGetPid(self.0, &mut pid) } == AX_OK).then_some(pid)
    }

    // ── Windows ──

    /// Standard (titled, resizable document/app) windows of an app element.
    pub fn windows(&self) -> Vec<Elem> {
        self.children("AXWindows").into_iter().filter(Elem::is_standard_window).collect()
    }

    fn is_standard_window(&self) -> bool {
        self.string("AXRole").as_deref() == Some("AXWindow") && matches!(self.string("AXSubrole").as_deref(), Some("AXStandardWindow") | None)
    }

    pub fn title(&self) -> String {
        self.string("AXTitle").unwrap_or_default()
    }

    pub fn frame(&self) -> Result<Rect> {
        let pos = self.copy("AXPosition").ok_or_else(|| Error::msg("Window has no position"))?;
        let size = self.copy("AXSize").ok_or_else(|| Error::msg("Window has no size"))?;
        let (mut p, mut s) = (CGPoint::default(), CGSize::default());
        unsafe {
            AXValueGetValue(pos.0, AX_VALUE_CGPOINT, &mut p as *mut _ as *mut c_void);
            AXValueGetValue(size.0, AX_VALUE_CGSIZE, &mut s as *mut _ as *mut c_void);
        }
        Ok(Rect { x: p.x, y: p.y, w: s.width, h: s.height })
    }

    fn set_point(&self, x: f64, y: f64) -> Result {
        let p = CGPoint { x, y };
        let v = Owned(unsafe { AXValueCreate(AX_VALUE_CGPOINT, &p as *const _ as *const c_void) });
        check("Move window", unsafe { AXUIElementSetAttributeValue(self.0, attr("AXPosition").as_concrete_TypeRef(), v.0) })
    }

    fn set_size(&self, w: f64, h: f64) -> Result {
        let s = CGSize { width: w, height: h };
        let v = Owned(unsafe { AXValueCreate(AX_VALUE_CGSIZE, &s as *const _ as *const c_void) });
        check("Resize window", unsafe { AXUIElementSetAttributeValue(self.0, attr("AXSize").as_concrete_TypeRef(), v.0) })
    }

    /// Move + resize. Size, position, size again: apps clamp a size that would
    /// overflow the screen at the old position, and some resize on move.
    pub fn set_frame(&self, r: Rect) -> Result {
        if self.bool("AXFullScreen") == Some(true) {
            return Err(Error::msg("Exit full screen first"));
        }
        let _ = self.set_size(r.w, r.h);
        self.set_point(r.x, r.y)?;
        self.set_size(r.w, r.h)
    }

    pub fn is_minimized(&self) -> bool {
        self.bool("AXMinimized") == Some(true)
    }

    pub fn set_minimized(&self, on: bool) -> Result {
        self.set_bool("AXMinimized", on)
    }

    pub fn is_fullscreen(&self) -> bool {
        self.bool("AXFullScreen") == Some(true)
    }

    pub fn set_fullscreen(&self, on: bool) -> Result {
        self.set_bool("AXFullScreen", on)
    }

    /// Press the window's red close button (the app may still ask to save).
    pub fn close(&self) -> Result {
        let btn = self.elem_attr("AXCloseButton").ok_or_else(|| Error::msg("This window has no close button"))?;
        btn.perform("AXPress")
    }

    /// Raise this window and make its app frontmost.
    pub fn bring_to_front(&self) -> Result {
        if self.is_minimized() {
            let _ = self.set_minimized(false);
        }
        let pid = self.pid().ok_or_else(|| Error::msg("Window's app is gone"))?;
        let app = Elem::app(pid);
        if app.bool("AXHidden") == Some(true) {
            let _ = app.set_bool("AXHidden", false);
        }
        let _ = self.perform("AXRaise");
        let _ = self.set_bool("AXMain", true);
        app.set_bool("AXFrontmost", true)
    }
}

/// The focused app (system-wide AX), as an app element.
/// The app the user is working in. Accessibility's focused application first;
/// while Arclet's own dial / deck is involved that lookup fails or returns
/// Arclet, so fall back to the owner of the frontmost normal window that isn't
/// Arclet's (window-server order, front to back).
pub fn focused_app() -> Result<Elem> {
    ensure_trusted()?;
    let sys = Elem::from_owned(unsafe { AXUIElementCreateSystemWide() }).ok_or_else(|| Error::msg("Accessibility is unavailable"))?;
    if let Some(app) = sys.elem_attr("AXFocusedApplication") {
        if app.pid().is_some_and(|pid| !crate::windows::is_arclet_pid(pid)) {
            return Ok(app);
        }
    }
    crate::windows::frontmost_app_pid().map(Elem::app).ok_or_else(|| Error::msg("No app with a window is in front"))
}

/// The focused window of the frontmost app.
pub fn focused_window() -> Result<Elem> {
    let app = focused_app()?;
    app.elem_attr("AXFocusedWindow")
        .or_else(|| app.elem_attr("AXMainWindow"))
        .or_else(|| app.windows().into_iter().next())
        .ok_or_else(|| Error::msg("The front app has no window"))
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub fn overlap(&self, o: &Rect) -> f64 {
        let w = (self.x + self.w).min(o.x + o.w) - self.x.max(o.x);
        let h = (self.y + self.h).min(o.y + o.h) - self.y.max(o.y);
        w.max(0.0) * h.max(0.0)
    }

    /// Same frame within a couple of points.
    #[cfg(test)]
    pub fn approx(&self, o: &Rect) -> bool {
        (self.x - o.x).abs() <= 4.0 && (self.y - o.y).abs() <= 4.0 && (self.w - o.w).abs() <= 4.0 && (self.h - o.h).abs() <= 4.0
    }
}
