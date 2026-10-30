//! Permission status checks, requests and System Settings deep links.
//!
//! Every permission is optional. Each check is cheap and side-effect free so
//! the Settings window can call it whenever it is shown.

use crate::ffi::ax;
use crate::ui::ns;
use block2::RcBlock;
use notchforge_core::model::{Permission, PermissionState};
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSBundle, NSDictionary, NSError, NSNumber, NSString, NSURL};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNNotificationSettings, UNUserNotificationCenter,
};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU8, Ordering};

/// UserNotifications aborts the process when used from an unbundled binary
/// (`cargo run`), so gate every call on having a bundle identifier.
pub fn is_bundled() -> bool {
    NSBundle::mainBundle().bundleIdentifier().is_some()
}

// Cached asynchronously: 0 unknown, 1 granted, 2 denied, 3 not determined.
static NOTIFICATION_STATE: AtomicU8 = AtomicU8::new(0);

fn refresh_notification_state() {
    if !is_bundled() {
        return;
    }
    let block = RcBlock::new(|settings: NonNull<UNNotificationSettings>| {
        // SAFETY: the framework passes a valid settings object.
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        let v = match status {
            UNAuthorizationStatus::Authorized | UNAuthorizationStatus::Provisional => 1,
            UNAuthorizationStatus::Denied => 2,
            UNAuthorizationStatus::NotDetermined => 3,
            _ => 0,
        };
        NOTIFICATION_STATE.store(v, Ordering::Relaxed);
    });
    UNUserNotificationCenter::currentNotificationCenter().getNotificationSettingsWithCompletionHandler(&block);
}

/// Full Disk Access has no query API: probe files that only FDA can open.
fn full_disk_access_state() -> PermissionState {
    let mut probes = notchforge_providers::usernoted::candidate_paths();
    if let Some(home) = dirs_home() {
        probes.push(home.join("Library/DoNotDisturb/DB/Assertions.json"));
        probes.push(home.join("Library/Safari/Bookmarks.plist"));
    }
    let mut saw_denied = false;
    for p in probes {
        match std::fs::File::open(&p) {
            Ok(_) => return PermissionState::Granted,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => saw_denied = true,
            Err(_) => {}
        }
    }
    if saw_denied { PermissionState::Denied } else { PermissionState::Unknown }
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

pub fn state(p: Permission) -> PermissionState {
    match p {
        Permission::Accessibility => {
            // SAFETY: no arguments.
            if unsafe { ax::AXIsProcessTrusted() } != 0 { PermissionState::Granted } else { PermissionState::Denied }
        }
        Permission::FullDiskAccess => full_disk_access_state(),
        Permission::Location => crate::location::authorization_state(),
        Permission::Notifications => {
            refresh_notification_state();
            match NOTIFICATION_STATE.load(Ordering::Relaxed) {
                1 => PermissionState::Granted,
                2 => PermissionState::Denied,
                3 => PermissionState::NotDetermined,
                _ if !is_bundled() => PermissionState::Unknown,
                _ => PermissionState::NotDetermined,
            }
        }
        Permission::Bluetooth => PermissionState::NotRequired,
    }
}

pub fn state_label(s: PermissionState) -> &'static str {
    match s {
        PermissionState::Granted => "Granted",
        PermissionState::Denied => "Not granted",
        PermissionState::NotDetermined => "Not requested yet",
        PermissionState::NotRequired => "Not required",
        PermissionState::Unknown => "Unknown",
    }
}

fn settings_url(p: Permission) -> &'static str {
    match p {
        Permission::Accessibility => "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        Permission::FullDiskAccess => "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles",
        Permission::Location => "x-apple.systempreferences:com.apple.preference.security?Privacy_LocationServices",
        Permission::Notifications => "x-apple.systempreferences:com.apple.preference.notifications",
        Permission::Bluetooth => "x-apple.systempreferences:com.apple.preference.security?Privacy_Bluetooth",
    }
}

pub fn open_settings(p: Permission) {
    if let Some(url) = NSURL::URLWithString(&ns(settings_url(p))) {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

/// Trigger the system prompt where one exists, otherwise open System Settings.
pub fn request(p: Permission) {
    match p {
        Permission::Accessibility => {
            // SAFETY: the key is a valid CFString constant; NSDictionary is
            // toll-free bridged to CFDictionary.
            unsafe {
                let key: &NSString = &*(ax::kAXTrustedCheckOptionPrompt as *const NSString);
                let yes = NSNumber::new_bool(true);
                let opts = NSDictionary::<NSString, NSNumber>::from_slices(&[key], &[&*yes]);
                let trusted = ax::AXIsProcessTrustedWithOptions(Retained::as_ptr(&opts) as _);
                if trusted == 0 {
                    open_settings(p);
                }
            }
        }
        Permission::Location => crate::location::request_authorization(),
        Permission::Notifications if is_bundled() => {
            let block = RcBlock::new(|granted: Bool, _err: *mut NSError| {
                NOTIFICATION_STATE.store(if granted.as_bool() { 1 } else { 2 }, Ordering::Relaxed);
            });
            UNUserNotificationCenter::currentNotificationCenter().requestAuthorizationWithOptions_completionHandler(
                UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                &block,
            );
        }
        _ => open_settings(p),
    }
}
