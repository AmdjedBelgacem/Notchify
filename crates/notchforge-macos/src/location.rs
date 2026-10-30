//! CoreLocation for the "auto" weather location.
//!
//! One coarse fix (3 km accuracy) at launch and then hourly; results are sent
//! to the weather provider through a `watch` channel. Requires a bundled app
//! with `NSLocationUsageDescription` (see `packaging/Info.plist`).

use notchforge_core::model::{Coordinates, PermissionState};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_core_location::{
    CLAuthorizationStatus, CLLocation, CLLocationManager, CLLocationManagerDelegate, kCLLocationAccuracyThreeKilometers,
};
use objc2_foundation::{NSArray, NSError};
use std::cell::RefCell;
use tokio::sync::watch;

pub struct LocationIvars {
    sender: watch::Sender<Option<Coordinates>>,
}

define_class!(
    // SAFETY: NSObject subclass, no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = LocationIvars]
    struct LocationDelegate;

    unsafe impl NSObjectProtocol for LocationDelegate {}

    unsafe impl CLLocationManagerDelegate for LocationDelegate {
        #[unsafe(method(locationManager:didUpdateLocations:))]
        fn did_update(&self, manager: &CLLocationManager, locations: &NSArray<CLLocation>) {
            if let Some(loc) = locations.lastObject() {
                // SAFETY: plain struct getter.
                let c = unsafe { loc.coordinate() };
                tracing::debug!(lat = c.latitude, lon = c.longitude, "location fix");
                self.ivars().sender.send_replace(Some(Coordinates { latitude: c.latitude, longitude: c.longitude }));
            }
            // SAFETY: documented call.
            unsafe { manager.stopUpdatingLocation() };
        }

        #[unsafe(method(locationManager:didFailWithError:))]
        fn did_fail(&self, _manager: &CLLocationManager, error: &NSError) {
            tracing::info!(error = %error.localizedDescription(), "location unavailable");
        }

        #[unsafe(method(locationManagerDidChangeAuthorization:))]
        fn did_change_authorization(&self, manager: &CLLocationManager) {
            if is_authorized(manager) {
                // SAFETY: documented call.
                unsafe { manager.requestLocation() };
            }
        }
    }
);

thread_local! {
    static SERVICE: RefCell<Option<(Retained<CLLocationManager>, Retained<LocationDelegate>)>> = const { RefCell::new(None) };
}

fn is_authorized(manager: &CLLocationManager) -> bool {
    // SAFETY: property read.
    let s = unsafe { manager.authorizationStatus() };
    s == CLAuthorizationStatus::AuthorizedAlways || s == CLAuthorizationStatus::AuthorizedWhenInUse
}

/// Create the manager and request a first fix if already authorized.
pub fn start(mtm: MainThreadMarker, sender: watch::Sender<Option<Coordinates>>) {
    let delegate = LocationDelegate::alloc(mtm).set_ivars(LocationIvars { sender });
    // SAFETY: NSObject init.
    let delegate: Retained<LocationDelegate> = unsafe { msg_send![super(delegate), init] };
    // SAFETY: documented CoreLocation setup.
    let manager = unsafe { CLLocationManager::new() };
    unsafe {
        manager.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        manager.setDesiredAccuracy(kCLLocationAccuracyThreeKilometers);
    }
    if is_authorized(&manager) {
        // SAFETY: documented call.
        unsafe { manager.requestLocation() };
    }
    SERVICE.with(|s| *s.borrow_mut() = Some((manager, delegate)));
}

/// Ask for a fresh fix (called hourly and when the weather location is "auto").
pub fn refresh() {
    SERVICE.with(|s| {
        if let Some((m, _)) = s.borrow().as_ref() {
            if is_authorized(m) {
                // SAFETY: documented call.
                unsafe { m.requestLocation() };
            }
        }
    });
}

pub fn request_authorization() {
    SERVICE.with(|s| {
        if let Some((m, _)) = s.borrow().as_ref() {
            // SAFETY: documented call; no-op if already determined.
            unsafe { m.requestWhenInUseAuthorization() };
        }
    });
    if authorization_state() == PermissionState::Denied {
        crate::permissions::open_settings(notchforge_core::model::Permission::Location);
    }
}

pub fn authorization_state() -> PermissionState {
    SERVICE.with(|s| match s.borrow().as_ref() {
        // SAFETY: property read.
        Some((m, _)) => match unsafe { m.authorizationStatus() } {
            CLAuthorizationStatus::AuthorizedAlways | CLAuthorizationStatus::AuthorizedWhenInUse => {
                PermissionState::Granted
            }
            CLAuthorizationStatus::Denied | CLAuthorizationStatus::Restricted => PermissionState::Denied,
            CLAuthorizationStatus::NotDetermined => PermissionState::NotDetermined,
            _ => PermissionState::Unknown,
        },
        None => PermissionState::Unknown,
    })
}
