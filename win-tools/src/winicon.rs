//! Extract a process's own icon (from its .exe) as a PNG data URI for the window
//! switcher wheels: ExtractIconExW → GetIconInfo → GetDIBits to RGBA, then a tiny
//! dependency-free PNG encoder (stored zlib blocks — no compression needed).

use base64::Engine;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;
use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, HBITMAP, HGDIOBJ};
use windows::Win32::UI::Shell::ExtractIconExW;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

fn extract_hicon(exe_path: &str) -> Option<HICON> {
    let wide: Vec<u16> = exe_path.encode_utf16().chain(Some(0)).collect();
    let (mut large, mut small) = (HICON::default(), HICON::default());
    let n = unsafe { ExtractIconExW(PCWSTR(wide.as_ptr()), 0, Some(&mut large), Some(&mut small), 1) };
    if n == 0 || n == u32::MAX {
        return None;
    }
    if !large.is_invalid() {
        if !small.is_invalid() {
            let _ = unsafe { DestroyIcon(small) };
        }
        return Some(large);
    }
    (!small.is_invalid()).then_some(small)
}

fn read_dib(hdc: windows::Win32::Graphics::Gdi::HDC, hbm: HBITMAP, w: i32, h: i32) -> Option<Vec<u8>> {
    let mut bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let got = unsafe { GetDIBits(hdc, hbm, 0, h as u32, Some(buf.as_mut_ptr() as *mut c_void), &mut bi, DIB_RGB_COLORS) };
    (got != 0).then_some(buf)
}

/// Rasterise to top-down RGBA. Returns (pixels, width, height).
fn hicon_to_rgba(hicon: HICON) -> Option<(Vec<u8>, u32, u32)> {
    let mut ii = ICONINFO::default();
    unsafe { GetIconInfo(hicon, &mut ii) }.ok()?;
    let result = (|| {
        if ii.hbmColor.is_invalid() {
            return None;
        }
        let mut bmp = BITMAP::default();
        if unsafe { GetObjectW(HGDIOBJ(ii.hbmColor.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut bmp as *mut _ as *mut c_void)) } == 0 {
            return None;
        }
        let (w, h) = (bmp.bmWidth.abs(), bmp.bmHeight.abs());
        if w == 0 || h == 0 || w > 1024 || h > 1024 {
            return None;
        }
        let hdc = unsafe { CreateCompatibleDC(None) };
        if hdc.is_invalid() {
            return None;
        }
        let out = (|| {
            let mut px = read_dib(hdc, ii.hbmColor, w, h)?;
            // Modern PNG-based icons carry alpha already; older ones need it from the AND mask.
            let has_alpha = px.chunks_exact(4).any(|p| p[3] != 0);
            px.chunks_exact_mut(4).for_each(|p| p.swap(0, 2)); // BGRA → RGBA
            if !has_alpha {
                let mask = if ii.hbmMask.is_invalid() { None } else { read_dib(hdc, ii.hbmMask, w, h) };
                for (i, p) in px.chunks_exact_mut(4).enumerate() {
                    p[3] = match &mask {
                        Some(m) => if m[i * 4] == 0 { 255 } else { 0 },
                        None => 255,
                    };
                }
            }
            Some((px, w as u32, h as u32))
        })();
        let _ = unsafe { DeleteDC(hdc) };
        out
    })();
    unsafe {
        if !ii.hbmColor.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(ii.hbmColor.0));
        }
        if !ii.hbmMask.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(ii.hbmMask.0));
        }
    }
    result
}

// ── Minimal PNG encoder (RGBA, stored deflate blocks) ────────────────────────

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (!(crc & 1)).wrapping_add(1));
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in bytes {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut chunks = data.chunks(0xFFFF).peekable();
    if data.is_empty() {
        out.extend([1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(c) = chunks.next() {
        out.push(u8::from(chunks.peek().is_none())); // BFINAL, BTYPE 00
        out.extend((c.len() as u16).to_le_bytes());
        out.extend((!(c.len() as u16)).to_le_bytes());
        out.extend(c);
    }
    out.extend(adler32(data).to_be_bytes());
    out
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend(kind);
    out.extend(data);
    let crc = crc32(&out[start..]);
    out.extend(crc.to_be_bytes());
}

pub fn png_encode_rgba(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let stride = w as usize * 4;
    let mut raw = Vec::with_capacity(h as usize * (1 + stride));
    for row in rgba.chunks(stride) {
        raw.push(0); // filter: None
        raw.extend(row);
    }
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]);
    let mut out = vec![137, 80, 78, 71, 13, 10, 26, 10];
    png_chunk(&mut out, b"IHDR", &ihdr);
    png_chunk(&mut out, b"IDAT", &zlib_store(&raw));
    png_chunk(&mut out, b"IEND", &[]);
    out
}

static CACHE: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

/// `data:image/png;base64,…` for the exe's icon, or `None` on any failure (cached per path).
pub fn extract_exe_icon_data_uri(exe_path: &str) -> Option<String> {
    if exe_path.is_empty() {
        return None;
    }
    let key = exe_path.to_lowercase();
    if let Some(hit) = CACHE.lock().unwrap().as_ref().and_then(|c| c.get(&key)) {
        return hit.clone();
    }
    let result = extract_hicon(exe_path).and_then(|hicon| {
        let raster = hicon_to_rgba(hicon);
        let _ = unsafe { DestroyIcon(hicon) };
        raster.map(|(px, w, h)| format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png_encode_rgba(&px, w, h))))
    });
    CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, result.clone());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_structure_and_checksums() {
        let png = png_encode_rgba(&[255, 0, 0, 255, 0, 255, 0, 255], 2, 1);
        assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
        assert_eq!(&png[12..16], b"IHDR");
        // Well-known CRC-32 and Adler-32 check values.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn extracts_icon_from_a_real_exe() {
        let uri = extract_exe_icon_data_uri("C:\\Windows\\explorer.exe").expect("explorer.exe has an icon");
        assert!(uri.starts_with("data:image/png;base64,"));
    }
}
