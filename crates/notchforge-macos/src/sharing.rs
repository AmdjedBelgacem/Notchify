//! File and app actions: Reveal in Finder, AirDrop, copy path, open.

use crate::ui::ns;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString, NSSharingService, NSSharingServiceNameSendViaAirDrop, NSWorkspace};
use objc2_foundation::{NSArray, NSString, NSURL};
use std::path::{Path, PathBuf};

pub fn file_url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&ns(&path.to_string_lossy()))
}

pub fn reveal(paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    let urls: Vec<_> = paths.iter().map(|p| file_url(p)).collect();
    let arr = NSArray::from_retained_slice(&urls);
    NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&arr);
}

pub fn airdrop(paths: &[PathBuf]) -> bool {
    if paths.is_empty() {
        return false;
    }
    // SAFETY: reading an AppKit constant.
    let name = unsafe { NSSharingServiceNameSendViaAirDrop };
    let Some(service) = NSSharingService::sharingServiceNamed(name) else { return false };
    let urls: Vec<Retained<AnyObject>> = paths.iter().map(|p| Retained::into_super(Retained::into_super(file_url(p)))).collect();
    let items = NSArray::from_retained_slice(&urls);
    // SAFETY: items are file URLs, which the AirDrop service accepts.
    unsafe {
        if !service.canPerformWithItems(Some(&items)) {
            return false;
        }
        service.performWithItems(&items);
    }
    true
}

pub fn copy_text(text: &str) {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    // SAFETY: reading an AppKit constant.
    pb.setString_forType(&ns(text), unsafe { NSPasteboardTypeString });
}

pub fn copy_paths(paths: &[PathBuf]) {
    let text = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n");
    copy_text(&text);
}

pub fn open_path(path: &Path) -> bool {
    NSWorkspace::sharedWorkspace().openURL(&file_url(path))
}

/// Open a URL string (web link, custom scheme, or file path).
pub fn open_url(url: &str) -> bool {
    if url.starts_with('/') {
        return open_path(Path::new(url));
    }
    match NSURL::URLWithString(&NSString::from_str(url)) {
        Some(u) => NSWorkspace::sharedWorkspace().openURL(&u),
        None => false,
    }
}

/// Bring an app to the front by bundle id.
pub fn open_app(bundle_id: &str) -> bool {
    std::process::Command::new("/usr/bin/open")
        .args(["-b", bundle_id])
        .spawn()
        .is_ok()
}
