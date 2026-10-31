//! Application icon cache. Real app icons are used when the app is installed;
//! otherwise the SF Symbol from [`KnownApp::symbol`] is shown, tinted.

use notchforge_core::apps::{AppRef, KnownApp};
use objc2::rc::Retained;
use objc2_app_kit::{NSImage, NSWorkspace};
use objc2_foundation::NSString;
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static CACHE: RefCell<HashMap<String, Option<Retained<NSImage>>>> = RefCell::new(HashMap::new());
}

fn installed_icon(bundle_id: &str) -> Option<Retained<NSImage>> {
    let ws = NSWorkspace::sharedWorkspace();
    let url = ws.URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))?;
    let path = url.path()?;
    Some(ws.iconForFile(&path))
}

/// Real icon for an app, if any of its bundle ids is installed.
pub fn app_icon(app: &AppRef) -> Option<Retained<NSImage>> {
    let mut ids: Vec<String> = app.bundle_id.iter().cloned().collect();
    if let Some(k) = app.known {
        ids.extend(k.bundle_ids().iter().map(|s| (*s).to_owned()));
    }
    for id in ids {
        let hit = CACHE.with(|c| c.borrow_mut().entry(id.clone()).or_insert_with(|| installed_icon(&id)).clone());
        if hit.is_some() {
            return hit;
        }
    }
    None
}

pub fn fallback_symbol(app: &AppRef) -> &'static str {
    app.known.map_or("app.badge", KnownApp::symbol)
}

pub fn accent(app: &AppRef) -> (u8, u8, u8) {
    app.known.map_or((150, 150, 160), KnownApp::accent)
}

/// Finder icon for a file in the tray.
pub fn file_icon(path: &std::path::Path) -> Retained<NSImage> {
    NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(&path.to_string_lossy()))
}

thread_local! {
    static PATH_ICONS: RefCell<HashMap<String, Retained<NSImage>>> = RefCell::new(HashMap::new());
    static FILE_IMAGES: RefCell<Vec<(std::path::PathBuf, Retained<NSImage>)>> = const { RefCell::new(Vec::new()) };
}

/// Icon of an installed app by bundle id (cached).
pub fn bundle_icon(bundle_id: &str) -> Option<Retained<NSImage>> {
    CACHE.with(|c| c.borrow_mut().entry(bundle_id.to_owned()).or_insert_with(|| installed_icon(bundle_id)).clone())
}

/// Icon of an app bundle by path (cached).
pub fn path_icon(path: &str) -> Retained<NSImage> {
    PATH_ICONS.with(|c| {
        c.borrow_mut()
            .entry(path.to_owned())
            .or_insert_with(|| NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(path)))
            .clone()
    })
}

/// Best icon for an audio source: bundle id, then app path.
pub fn source_icon(bundle_id: Option<&str>, app_path: Option<&str>) -> Option<Retained<NSImage>> {
    bundle_id.and_then(bundle_icon).or_else(|| app_path.map(path_icon))
}

/// Image loaded from a file (cover art), with a small LRU so re-renders and
/// repeated tracks don't hit the disk.
pub fn file_image(path: &std::path::Path) -> Option<Retained<NSImage>> {
    const KEEP: usize = 48;
    if let Some(hit) = FILE_IMAGES.with(|c| {
        let mut c = c.borrow_mut();
        let i = c.iter().position(|(p, _)| p == path)?;
        let entry = c.remove(i);
        let img = entry.1.clone();
        c.push(entry);
        Some(img)
    }) {
        return Some(hit);
    }
    let img = NSImage::initWithContentsOfFile(
        <NSImage as objc2::AnyThread>::alloc(),
        &NSString::from_str(&path.to_string_lossy()),
    )?;
    FILE_IMAGES.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= KEEP {
            c.remove(0);
        }
        c.push((path.to_owned(), img.clone()));
    });
    Some(img)
}

/// Average colour of an image (sRGB 0–1), lifted so it reads on black.
pub fn average_color(image: &NSImage) -> Option<(f64, f64, f64)> {
    use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext, CGImageAlphaInfo};
    // SAFETY: null proposed rect / context / hints are documented as allowed.
    let cg = unsafe { image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }?;
    let mut px = [0u8; 4];
    let space = CGColorSpace::new_device_rgb()?;
    // SAFETY: `px` is 1×1×4 bytes and outlives the context.
    unsafe {
        let ctx = CGBitmapContextCreate(
            px.as_mut_ptr().cast(),
            1,
            1,
            8,
            4,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )?;
        CGContext::draw_image(Some(&ctx), crate::ui::rect(0.0, 0.0, 1.0, 1.0), Some(&cg));
    }
    let a = px[3] as f64 / 255.0;
    if a < 0.05 {
        return None;
    }
    let (r, g, b) = (px[0] as f64 / 255.0 / a, px[1] as f64 / 255.0 / a, px[2] as f64 / 255.0 / a);
    Some(lift(r.min(1.0), g.min(1.0), b.min(1.0)))
}

/// Raise very dark colours toward a minimum brightness, keeping the hue.
pub fn lift(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    const MIN: f64 = 0.45;
    if max >= MIN {
        return (r, g, b);
    }
    if max <= 0.001 {
        return (MIN, MIN, MIN);
    }
    let k = MIN / max;
    ((r * k).min(1.0), (g * k).min(1.0), (b * k).min(1.0))
}
