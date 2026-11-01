//! Displays in AX coordinates (points, top-left origin on the main display).
//!
//! Bounds come from CoreGraphics, which is always current. The work area
//! (without the menu bar and Dock) comes from NSScreen's `visibleFrame`.
//! NSScreen is formally main-thread API and refreshes its list through the
//! app run loop a plugin doesn't have, so it is only used for the insets of
//! displays it knows; a display it doesn't (just plugged in) gets a menu-bar inset.

use crate::ax::Rect;
use objc2::rc::autoreleasepool;
use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use objc2_foundation::{NSNumber, NSString};

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayBounds(display: u32) -> CGRect;
    fn CGMainDisplayID() -> u32;
}

#[derive(Clone, Debug)]
pub struct Display {
    pub frame: Rect,
    pub work: Rect,
    pub primary: bool,
}

/// display id → (frame inset left, top, right, bottom) from NSScreen.
fn insets() -> Vec<(u32, [f64; 4])> {
    // SAFETY: NSScreen is only read here (frame/visibleFrame/deviceDescription);
    // plugin handlers run off the main thread, which has no run loop to hop to.
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    autoreleasepool(|_| {
        let screens = NSScreen::screens(mtm);
        let key = NSString::from_str("NSScreenNumber");
        screens
            .iter()
            .filter_map(|s| {
                let desc = s.deviceDescription();
                let num = desc.objectForKey(&key)?;
                let id = num.downcast_ref::<NSNumber>()?.unsignedIntValue();
                let (f, v) = (s.frame(), s.visibleFrame());
                // Cocoa rects have a bottom-left origin: the top inset is the menu bar.
                let top = (f.origin.y + f.size.height) - (v.origin.y + v.size.height);
                let bottom = v.origin.y - f.origin.y;
                let left = v.origin.x - f.origin.x;
                let right = (f.origin.x + f.size.width) - (v.origin.x + v.size.width);
                Some((id, [left, top, right, bottom]))
            })
            .collect()
    })
}

/// Active displays: the main one first, then left to right.
pub fn list() -> Vec<Display> {
    let mut ids = [0u32; 16];
    let mut n = 0u32;
    if unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) } != 0 {
        return vec![];
    }
    let main = unsafe { CGMainDisplayID() };
    let known = insets();
    let mut out: Vec<Display> = ids[..n as usize]
        .iter()
        .map(|&id| {
            let b = unsafe { CGDisplayBounds(id) };
            let frame = Rect { x: b.x, y: b.y, w: b.w, h: b.h };
            let [l, t, r, btm] = known.iter().find(|(k, _)| *k == id).map_or([0.0, 25.0, 0.0, 0.0], |(_, i)| *i);
            let work = Rect { x: frame.x + l, y: frame.y + t, w: (frame.w - l - r).max(1.0), h: (frame.h - t - btm).max(1.0) };
            Display { frame, work, primary: id == main }
        })
        .collect();
    out.sort_by(|a, b| b.primary.cmp(&a.primary).then(a.frame.x.total_cmp(&b.frame.x)).then(a.frame.y.total_cmp(&b.frame.y)));
    out
}

/// The display showing most of `r` (else the main one).
pub fn containing(displays: &[Display], r: &Rect) -> usize {
    let c = r.center();
    displays
        .iter()
        .position(|d| d.frame.contains(c))
        .or_else(|| displays.iter().enumerate().max_by(|a, b| a.1.frame.overlap(r).total_cmp(&b.1.frame.overlap(r))).map(|(i, _)| i))
        .unwrap_or(0)
}
