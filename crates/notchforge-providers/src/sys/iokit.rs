//! IOKit power-source API (`IOPowerSources.h`), public and stable since 10.x.

use super::cf::{self, CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef, CFDictionaryRef, CFTypeRef, Owned};
use notchforge_core::model::BatteryStatus;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
    fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> CFArrayRef;
    fn IOPSGetPowerSourceDescription(blob: CFTypeRef, ps: CFTypeRef) -> CFDictionaryRef;
}

/// Read the internal battery. `None` if the API fails; `Some(present=false)`
/// on Macs without a battery.
pub fn read_internal_battery() -> Option<BatteryStatus> {
    // SAFETY: Copy functions return +1 objects (wrapped in `Owned`), the
    // description dictionary is borrowed from `blob`, which outlives its use.
    unsafe {
        let blob = Owned::from_create(IOPSCopyPowerSourcesInfo())?;
        let list = Owned::from_create(IOPSCopyPowerSourcesList(blob.as_ptr()))?;
        let count = CFArrayGetCount(list.as_ptr());
        for i in 0..count {
            let ps = CFArrayGetValueAtIndex(list.as_ptr(), i);
            let desc = IOPSGetPowerSourceDescription(blob.as_ptr(), ps);
            if desc.is_null() {
                continue;
            }
            let ty = cf::as_string(cf::dict_get(desc, "Type"));
            if ty.as_deref() != Some("InternalBattery") {
                continue;
            }
            let current = cf::as_i64(cf::dict_get(desc, "Current Capacity")).unwrap_or(0);
            let max = cf::as_i64(cf::dict_get(desc, "Max Capacity")).unwrap_or(100).max(1);
            let charging = cf::as_bool(cf::dict_get(desc, "Is Charging")).unwrap_or(false);
            let on_ac = cf::as_string(cf::dict_get(desc, "Power Source State")).as_deref() == Some("AC Power");
            let present = cf::as_bool(cf::dict_get(desc, "Is Present")).unwrap_or(true);
            let key = if charging { "Time to Full Charge" } else { "Time to Empty" };
            let minutes = cf::as_i64(cf::dict_get(desc, key)).filter(|m| *m > 0 && *m < 24 * 60).map(|m| m as u32);
            return Some(BatteryStatus {
                percent: ((current * 100) / max).clamp(0, 100) as u8,
                charging,
                plugged_in: on_ac,
                minutes_remaining: if on_ac && !charging { None } else { minutes },
                present,
            });
        }
        Some(BatteryStatus { present: false, plugged_in: true, ..BatteryStatus::default() })
    }
}
