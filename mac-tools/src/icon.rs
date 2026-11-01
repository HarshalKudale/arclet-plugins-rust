//! App icons and image thumbnails as PNG data URIs (wheel slices accept Lucide
//! names or `data:` URIs only).

use base64::Engine;
use objc2::rc::{autoreleasepool, Retained};
use objc2::AllocAnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSWorkspace};
use objc2_foundation::{NSData, NSDictionary, NSPoint, NSRect, NSSize, NSString};
use std::collections::HashMap;
use std::sync::Mutex;

const SIZE: f64 = 64.0;

fn png_of(image: &NSImage, size: f64) -> Option<Vec<u8>> {
    let mut rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(size, size));
    let cg = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
    let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }?;
    Some(data.to_vec())
}

fn data_uri(png: Vec<u8>) -> String {
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))
}

static CACHE: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

/// The icon of an app bundle (or any file) as a 64 px PNG data URI. Cached.
pub fn app_icon(bundle: &str) -> Option<String> {
    if let Some(hit) = CACHE.lock().unwrap().as_ref().and_then(|m| m.get(bundle).cloned()) {
        return hit;
    }
    let uri = autoreleasepool(|_| {
        let image = NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(bundle));
        png_of(&image, SIZE).map(data_uri)
    });
    CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(bundle.to_string(), uri.clone());
    uri
}

/// A thumbnail (fit into 64×64) of image data (PNG, TIFF, JPEG…).
pub fn thumbnail(bytes: &[u8]) -> Option<String> {
    autoreleasepool(|_| {
        let data = NSData::with_bytes(bytes);
        let image: Retained<NSImage> = NSImage::initWithData(NSImage::alloc(), &data)?;
        let s = image.size();
        if s.width <= 0.0 || s.height <= 0.0 {
            return None;
        }
        let scale = SIZE / s.width.max(s.height);
        let mut rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new((s.width * scale).max(1.0), (s.height * scale).max(1.0)));
        let cg = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
        let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
        let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }?;
        Some(data_uri(png.to_vec()))
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn finder_has_an_icon() {
        let uri = super::app_icon("/System/Library/CoreServices/Finder.app").expect("Finder icon");
        assert!(uri.starts_with("data:image/png;base64,"));
    }
}
