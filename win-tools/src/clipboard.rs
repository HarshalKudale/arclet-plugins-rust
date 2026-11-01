//! Windows Clipboard History (the list Win+V shows) via WinRT, plus Ctrl+V injection.
//!
//! History is never cached here: every list/paste call re-queries Windows. An item's
//! bytes are only reachable to a plain (non-packaged) process by momentarily making it
//! the live Win32 clipboard (`SetHistoryItemAsContent`) and reading it back with the
//! ordinary clipboard API; the original content (history item 0) is always restored
//! before returning, so listing has no lasting effect on the clipboard.

use crate::audio::ensure_com;
use base64::Engine;
use std::ffi::c_void;
use std::time::Duration;
use windows::ApplicationModel::DataTransfer::{Clipboard, ClipboardHistoryItem};
use windows::Win32::Foundation::{HGLOBAL, HWND};
use windows::Win32::Foundation::{GlobalFree, HANDLE};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_CONTROL, VK_RETURN, VK_V};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

type R<T> = arclet_sdk::Result<T>;

const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;
const PREVIEW_TEXT_MAX: usize = 120;
/// Thumbnails keep the wheel_result WebSocket frame small: full-resolution screenshots
/// as base64 BMPs can exceed the host's frame limit and silently drop the connection.
const THUMBNAIL_MAX_DIM: usize = 160;

fn werr(what: &str, e: windows::core::Error) -> arclet_sdk::Error {
    arclet_sdk::Error::msg(format!("{what} failed: {e}"))
}

pub struct HistoryItem {
    pub id: String,
    /// "text" | "image" | "files" | "other"
    pub kind: &'static str,
    pub label: String,
    pub icon: Option<String>,
}

pub struct History {
    pub enabled: bool,
    pub items: Vec<HistoryItem>,
}

fn history_items() -> R<Vec<ClipboardHistoryItem>> {
    let result = Clipboard::GetHistoryItemsAsync().and_then(|op| op.get()).map_err(|e| werr("GetHistoryItemsAsync", e))?;
    let items = result.Items().map_err(|e| werr("Items", e))?;
    Ok(items.into_iter().take(16).collect())
}

/// `SetHistoryItemAsContent` can fail transiently (CLIPBRD_E_CANT_OPEN) when another
/// process briefly owns the clipboard; retry a few times before giving up.
fn set_live(item: &ClipboardHistoryItem) -> R<()> {
    let mut last = None;
    for i in 0..4 {
        match Clipboard::SetHistoryItemAsContent(item) {
            Ok(_) => return Ok(()),
            Err(e) => last = Some(e),
        }
        if i < 3 {
            std::thread::sleep(Duration::from_millis(15));
        }
    }
    Err(werr("SetHistoryItemAsContent", last.unwrap()))
}

// ── Reading the live Win32 clipboard ─────────────────────────────────────────

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn i32_at(b: &[u8], o: usize) -> i32 {
    u32_at(b, o) as i32
}
fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}

/// Nearest-neighbour downscale of a CF_DIB (24/32 bpp BI_RGB or BI_BITFIELDS) to a small
/// 24 bpp bottom-up DIB, or `None` for unsupported pixel formats.
fn downscale_dib(dib: &[u8], max_dim: usize) -> Option<Vec<u8>> {
    if dib.len() < 40 {
        return None;
    }
    let bi_size = u32_at(dib, 0) as usize;
    let width = i32_at(dib, 4);
    let raw_height = i32_at(dib, 8);
    let height = raw_height.unsigned_abs() as usize;
    let bits = u16_at(dib, 14);
    let compression = u32_at(dib, 16);
    if !matches!(compression, 0 | 3) || !matches!(bits, 24 | 32) || width <= 0 || height == 0 {
        return None;
    }
    let width = width as usize;
    let top_down = raw_height < 0;
    let src_bpp = bits as usize / 8;
    let src_stride = (width * bits as usize).div_ceil(32) * 4;
    let src_off = bi_size + if compression == 3 { 12 } else { 0 };

    let scale = (max_dim as f64 / width.max(height) as f64).min(1.0);
    let dst_w = ((width as f64 * scale).round() as usize).max(1);
    let dst_h = ((height as f64 * scale).round() as usize).max(1);
    let dst_stride = (dst_w * 24).div_ceil(32) * 4;

    let mut out = vec![0u8; 40 + dst_stride * dst_h];
    out[0..4].copy_from_slice(&40u32.to_le_bytes());
    out[4..8].copy_from_slice(&(dst_w as i32).to_le_bytes());
    out[8..12].copy_from_slice(&(dst_h as i32).to_le_bytes()); // positive = bottom-up
    out[12..14].copy_from_slice(&1u16.to_le_bytes());
    out[14..16].copy_from_slice(&24u16.to_le_bytes());
    out[20..24].copy_from_slice(&((dst_stride * dst_h) as u32).to_le_bytes());

    for dy in 0..dst_h {
        let src_y_top = ((dy as f64 * (height as f64 / dst_h as f64)).floor() as usize).min(height - 1);
        let src_row = if top_down { src_y_top } else { height - 1 - src_y_top };
        let dst_row = dst_h - 1 - dy;
        for dx in 0..dst_w {
            let src_col = ((dx as f64 * (width as f64 / dst_w as f64)).floor() as usize).min(width - 1);
            let s = src_off + src_row * src_stride + src_col * src_bpp;
            let d = 40 + dst_row * dst_stride + dx * 3;
            if s + 2 < dib.len() {
                out[d..d + 3].copy_from_slice(&dib[s..s + 3]); // B, G, R
            }
        }
    }
    Some(out)
}

/// Wrap a CF_DIB payload as a standalone .bmp file.
fn dib_to_bmp(dib: &[u8]) -> Vec<u8> {
    let bi_size = u32_at(dib, 0) as usize;
    let bits = u16_at(dib, 14);
    let compression = u32_at(dib, 16);
    let mut palette = u32_at(dib, 32) as usize;
    if palette == 0 && bits <= 8 {
        palette = 1 << bits;
    }
    let off_bits = 14 + bi_size + palette * 4 + if compression == 3 { 12 } else { 0 };
    let mut out = Vec::with_capacity(14 + dib.len());
    out.extend(b"BM");
    out.extend(((14 + dib.len()) as u32).to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend((off_bits as u32).to_le_bytes());
    out.extend(dib);
    out
}

struct Preview {
    kind: &'static str,
    label: String,
    icon: Option<String>,
}

fn other() -> Preview {
    Preview { kind: "other", label: "Clipboard item".into(), icon: None }
}

/// Describe whatever is currently on the live Win32 clipboard.
fn read_live_preview() -> Preview {
    if unsafe { OpenClipboard(HWND::default()) }.is_err() {
        return other();
    }
    let preview = (|| unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT).is_ok() {
            let Ok(handle) = GetClipboardData(CF_UNICODETEXT) else { return other() };
            let g = HGLOBAL(handle.0 as *mut c_void);
            let p = GlobalLock(g) as *const u16;
            let text = if p.is_null() {
                String::new()
            } else {
                let mut len = 0;
                while *p.add(len) != 0 {
                    len += 1;
                }
                String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
            };
            let _ = GlobalUnlock(g);
            let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let label = if one_line.is_empty() {
                "(empty text)".to_string()
            } else if one_line.chars().count() > PREVIEW_TEXT_MAX {
                format!("{}…", one_line.chars().take(PREVIEW_TEXT_MAX).collect::<String>())
            } else {
                one_line
            };
            return Preview { kind: "text", label, icon: None };
        }
        if IsClipboardFormatAvailable(CF_DIB).is_ok() {
            let Ok(handle) = GetClipboardData(CF_DIB) else { return other() };
            let g = HGLOBAL(handle.0 as *mut c_void);
            let size = GlobalSize(g);
            let p = GlobalLock(g) as *const u8;
            let dib = (!p.is_null() && size >= 40).then(|| std::slice::from_raw_parts(p, size).to_vec());
            let _ = GlobalUnlock(g);
            let Some(dib) = dib else { return other() };
            // Always thumbnail before base64: see THUMBNAIL_MAX_DIM.
            return match downscale_dib(&dib, THUMBNAIL_MAX_DIM) {
                None => Preview { kind: "image", label: "Image".into(), icon: None },
                Some(thumb) => Preview {
                    kind: "image",
                    label: String::new(),
                    icon: Some(format!("data:image/bmp;base64,{}", base64::engine::general_purpose::STANDARD.encode(dib_to_bmp(&thumb)))),
                },
            };
        }
        if IsClipboardFormatAvailable(CF_HDROP).is_ok() {
            let mut names = vec![];
            if let Ok(handle) = GetClipboardData(CF_HDROP) {
                let drop = HDROP(handle.0 as *mut c_void);
                let count = DragQueryFileW(drop, u32::MAX, None);
                for i in 0..count {
                    let mut buf = [0u16; 260];
                    let n = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
                    let full = String::from_utf16_lossy(&buf[..n]);
                    names.push(full.rsplit('\\').next().unwrap_or("").to_string());
                }
            }
            let label = match names.len() {
                0 => "Files".to_string(),
                1 => names[0].clone(),
                n => format!("{n} files: {}", names.join(", ")),
            };
            return Preview { kind: "files", label, icon: None };
        }
        other()
    })();
    let _ = unsafe { CloseClipboard() };
    preview
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Up to 16 live history entries; `enabled == false` when history is off system-wide.
pub fn list_history() -> R<History> {
    ensure_com();
    if !Clipboard::IsHistoryEnabled().map_err(|e| werr("IsHistoryEnabled", e))? {
        return Ok(History { enabled: false, items: vec![] });
    }
    let items = history_items()?;
    // Each item previews independently: one transient failure must not abort the listing or
    // skip the restore below (a stuck temporary preview as live clipboard is worse than a gap).
    let out = items
        .iter()
        .map(|item| {
            let id = item.Id().map(|h| h.to_string()).unwrap_or_default();
            match set_live(item) {
                Ok(()) => {
                    let p = read_live_preview();
                    HistoryItem { id, kind: p.kind, label: p.label, icon: p.icon }
                }
                Err(_) => HistoryItem { id, kind: "other", label: "Clipboard item (preview unavailable)".into(), icon: None },
            }
        })
        .collect();
    // Restore the original clipboard content (item 0 = whatever was live before).
    if let Some(first) = items.first() {
        let _ = set_live(first);
    }
    Ok(History { enabled: true, items: out })
}

/// Make the entry with `id` (from `list_history`) the live clipboard content.
pub fn apply_history_item(id: &str) -> R<()> {
    ensure_com();
    let items = history_items()?;
    let item = items
        .iter()
        .find(|i| i.Id().map_or(false, |h| h.to_string() == id))
        .ok_or_else(|| arclet_sdk::Error::msg("That clipboard item is no longer available"))?;
    set_live(item)
}

/// Arclet's `INJECTED_INPUT_TAG` (arclet-core `action.rs`): the dial's keyboard hook passes
/// keys carrying it through to the app instead of swallowing them while the dial is open.
const INJECTED_INPUT_TAG: usize = 0x4152_434C; // "ARCL"

fn key_input(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS, up: bool) -> INPUT {
    let flags = if up { flags | KEYEVENTF_KEYUP } else { flags };
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: INJECTED_INPUT_TAG } } }
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    key_input(vk, 0, KEYBD_EVENT_FLAGS(0), up)
}

/// Type `text` into whichever window has input focus, as Unicode key events (layout-independent).
/// A keystroke on purpose: the action *is* typing. Line breaks press Enter, which apps expect
/// instead of a U+000A character.
pub fn type_text(text: &str) {
    let mut inputs = Vec::with_capacity(text.len() * 2);
    for c in text.chars().filter(|&c| c != '\r') {
        let mut buf = [0u16; 2];
        for up in [false, true] {
            if c == '\n' {
                inputs.push(key(VK_RETURN, up));
            } else {
                inputs.extend(c.encode_utf16(&mut buf).iter().map(|&u| key_input(VIRTUAL_KEY(0), u, KEYEVENTF_UNICODE, up)));
            }
        }
    }
    if !inputs.is_empty() {
        unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    }
}

/// Replace the live clipboard with `text` (CF_UNICODETEXT).
pub fn set_text(text: &str) -> R<()> {
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let bytes = wide.len() * 2;
    unsafe {
        OpenClipboard(HWND::default()).map_err(|e| werr("OpenClipboard", e))?;
        let result = (|| {
            EmptyClipboard().map_err(|e| werr("EmptyClipboard", e))?;
            let g = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|e| werr("GlobalAlloc", e))?;
            let p = GlobalLock(g) as *mut u16;
            if p.is_null() {
                let _ = GlobalFree(g);
                return Err(arclet_sdk::Error::msg("GlobalLock failed"));
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
            let _ = GlobalUnlock(g);
            // On success the clipboard owns the memory; on failure it is still ours to free.
            SetClipboardData(CF_UNICODETEXT, HANDLE(g.0)).map(|_| ()).map_err(|e| {
                let _ = GlobalFree(g);
                werr("SetClipboardData", e)
            })
        })();
        let _ = CloseClipboard();
        result
    }
}

/// Send a synthetic Ctrl+V to whichever window has input focus. A keystroke
/// on purpose: Windows has no app-independent paste call (`WM_PASTE` only
/// reaches classic edit controls; UI Automation has no paste pattern).
pub fn send_paste() {
    let inputs = [key(VK_CONTROL, false), key(VK_V, false), key(VK_V, true), key(VK_CONTROL, true)];
    unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
}
