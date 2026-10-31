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
