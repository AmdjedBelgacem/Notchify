//! Accessory batteries (AirPods, Magic Keyboard/Mouse/Trackpad, Beats,
//! third-party HID devices).
//!
//! Two sources are merged, neither needs the Bluetooth permission:
//!
//! 1. `ioreg -r -a -k BatteryPercent` — HID devices (Magic accessories and
//!    many third-party keyboards/mice/controllers) publish `BatteryPercent` in
//!    the IORegistry. Fast (~20 ms), XML plist output.
//! 2. `system_profiler SPBluetoothDataType -json` — the only public place
//!    AirPods/Beats per-bud and case levels appear. Slower (~0.5–2 s), so it
//!    runs less often and off the async workers.
//!
//! Apple Watch battery is not exposed to macOS by any public interface; see
//! README "Known limitations".

use crate::util::{normalize_address, parse_percent, run_command};
use notchforge_core::model::{BatteryComponent, DeviceBattery, DeviceKind, ProviderStatus};
use notchforge_core::provider::{BoxFuture, Provider, ProviderContext, ProviderDescriptor, ProviderError, ProviderEvent, Wake};
use std::time::{Duration, Instant};

const IOREG_INTERVAL: Duration = Duration::from_secs(60);
const PROFILER_INTERVAL: Duration = Duration::from_secs(180);
const MIN_REFRESH_GAP: Duration = Duration::from_secs(20);

/// Parse `ioreg -r -a -k BatteryPercent` (an XML plist array of dictionaries).
pub fn parse_ioreg(xml: &[u8]) -> Vec<DeviceBattery> {
    let Ok(plist::Value::Array(entries)) = plist::Value::from_reader(std::io::Cursor::new(xml)) else {
        return Vec::new();
    };
    let mut out: Vec<DeviceBattery> = Vec::new();
    for entry in entries {
        let Some(d) = entry.as_dictionary() else { continue };
        let Some(level) = d.get("BatteryPercent").and_then(|v| v.as_signed_integer()) else { continue };
        let name = d
            .get("Product")
            .and_then(|v| v.as_string())
            .or_else(|| d.get("Name").and_then(|v| v.as_string()))
            .unwrap_or("Accessory")
            .trim()
            .to_owned();
        // Internal trackpads/keyboards of the MacBook itself do not report a level.
        let id = d
            .get("DeviceAddress")
            .and_then(|v| v.as_string())
            .map(normalize_address)
            .unwrap_or_else(|| name.to_ascii_lowercase());
        if out.iter().any(|e| e.id == id) {
            continue;
        }
        out.push(DeviceBattery {
            id,
            kind: DeviceKind::guess(&name),
            name,
            levels: vec![(BatteryComponent::Main, level.clamp(0, 100) as u8)],
        });
    }
    out
}

fn minor_type_kind(minor: &str, name: &str) -> DeviceKind {
    let guess = DeviceKind::guess(name);
    if guess != DeviceKind::Other {
        return guess;
    }
    match minor.to_ascii_lowercase().as_str() {
        "headphones" | "headset" => DeviceKind::Headphones,
        "keyboard" => DeviceKind::Keyboard,
        "mouse" => DeviceKind::Mouse,
        "gamepad" | "joystick" => DeviceKind::GameController,
        _ => DeviceKind::Other,
    }
}

/// Parse `system_profiler SPBluetoothDataType -json`.
pub fn parse_system_profiler(json: &[u8]) -> Vec<DeviceBattery> {
    let Ok(root) = serde_json::from_slice::<serde_json::Value>(json) else { return Vec::new() };
    let mut out = Vec::new();
    let controllers = root.get("SPBluetoothDataType").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    for controller in controllers {
        let Some(connected) = controller.get("device_connected").and_then(|v| v.as_array()) else { continue };
        for device in connected {
            let Some(obj) = device.as_object() else { continue };
            for (name, props) in obj {
                let level = |key: &str| props.get(key).and_then(|v| v.as_str()).and_then(parse_percent);
                let mut levels = Vec::new();
                for (key, comp) in [
                    ("device_batteryLevelMain", BatteryComponent::Main),
                    ("device_batteryLevel", BatteryComponent::Main),
                    ("device_batteryLevelLeft", BatteryComponent::Left),
                    ("device_batteryLevelRight", BatteryComponent::Right),
                    ("device_batteryLevelCase", BatteryComponent::Case),
                ] {
                    if let Some(l) = level(key) {
                        if !levels.iter().any(|(c, _)| *c == comp) {
                            levels.push((comp, l));
                        }
                    }
                }
                if levels.is_empty() {
                    continue;
                }
                let minor = props.get("device_minorType").and_then(|v| v.as_str()).unwrap_or("");
                let id = props
                    .get("device_address")
                    .and_then(|v| v.as_str())
                    .map(normalize_address)
                    .unwrap_or_else(|| name.to_ascii_lowercase());
                out.push(DeviceBattery { id, kind: minor_type_kind(minor, name), name: name.clone(), levels });
            }
        }
    }
    out
}

/// Merge both sources. system_profiler wins for devices it knows (it has
/// per-component levels); ioreg adds the rest. Matching is by address, then by
/// case-insensitive name.
pub fn merge(profiler: Vec<DeviceBattery>, ioreg: Vec<DeviceBattery>) -> Vec<DeviceBattery> {
    let mut out = profiler;
    for dev in ioreg {
        let dup = out.iter().any(|p| p.id == dev.id || p.name.eq_ignore_ascii_case(&dev.name));
        if !dup {
            out.push(dev);
        }
    }
    out.sort_by_key(|d| d.name.to_lowercase());
    out
}

pub struct BluetoothProvider;

impl Provider for BluetoothProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "bluetooth", name: "Accessory batteries", permissions: &[] }
    }

    fn run(self: Box<Self>, mut ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            if !cfg!(target_os = "macos") {
                ctx.set_status(ProviderStatus::Failing("accessory batteries need macOS".into())).await;
                return Ok(());
            }
            ctx.set_status(ProviderStatus::Running).await;
            let mut profiler_devices: Vec<DeviceBattery> = Vec::new();
            let mut last_profiler: Option<Instant> = None;
            let mut last_poll: Option<Instant>;
            let mut last_sent: Option<Vec<DeviceBattery>> = None;
            loop {
                let now = Instant::now();
                if last_profiler.is_none_or(|t| now - t >= PROFILER_INTERVAL) {
                    match run_command(
                        "/usr/sbin/system_profiler",
                        &["SPBluetoothDataType", "-json", "-detailLevel", "basic"],
                        Duration::from_secs(20),
                    )
                    .await
                    {
                        Ok(json) => profiler_devices = parse_system_profiler(&json),
                        Err(e) => tracing::debug!(error = %e, "system_profiler failed"),
                    }
                    last_profiler = Some(now);
                }
                let ioreg = match run_command("/usr/sbin/ioreg", &["-r", "-a", "-k", "BatteryPercent"], Duration::from_secs(5)).await {
                    Ok(xml) => parse_ioreg(&xml),
                    Err(e) => {
                        tracing::debug!(error = %e, "ioreg failed");
                        Vec::new()
                    }
                };
                last_poll = Some(Instant::now());
                let merged = merge(profiler_devices.clone(), ioreg);
                if last_sent.as_ref() != Some(&merged) {
                    last_sent = Some(merged.clone());
                    if !ctx.emit(ProviderEvent::Devices(merged)).await {
                        return Ok(());
                    }
                }
                loop {
                    match ctx.sleep_or_refresh(IOREG_INTERVAL).await {
                        Wake::Shutdown => return Ok(()),
                        Wake::Timeout => break,
                        // Throttle refreshes caused by rapid panel open/close.
                        Wake::Refresh if last_poll.is_some_and(|t| t.elapsed() < MIN_REFRESH_GAP) => continue,
                        Wake::Refresh => {
                            // A refresh also re-reads AirPods levels if they're stale-ish.
                            if last_profiler.is_some_and(|t| t.elapsed() > MIN_REFRESH_GAP * 3) {
                                last_profiler = None;
                            }
                            break;
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IOREG: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<array>
  <dict>
    <key>BatteryPercent</key><integer>64</integer>
    <key>DeviceAddress</key><string>aa-bb-cc-dd-ee-01</string>
    <key>Product</key><string>Magic Keyboard with Touch ID</string>
  </dict>
  <dict>
    <key>BatteryPercent</key><integer>12</integer>
    <key>Product</key><string>Magic Trackpad</string>
  </dict>
  <dict>
    <key>Product</key><string>No battery here</string>
  </dict>
</array>
</plist>"#;

    const PROFILER: &str = r#"{
      "SPBluetoothDataType": [{
        "controller_properties": {"controller_state": "attrib_on"},
        "device_connected": [
          {"Amdjed's AirPods Pro": {
              "device_address": "AA:BB:CC:DD:EE:02",
              "device_batteryLevelCase": "45%",
              "device_batteryLevelLeft": "100 %",
              "device_batteryLevelRight": "98%",
              "device_minorType": "Headphones"}},
          {"Magic Keyboard with Touch ID": {
              "device_address": "AA:BB:CC:DD:EE:01",
              "device_batteryLevelMain": "64%",
              "device_minorType": "Keyboard"}},
          {"Some Speaker": {"device_address": "AA:BB:CC:DD:EE:03", "device_minorType": "Speaker"}}
        ],
        "device_not_connected": [{"Old Mouse": {"device_batteryLevelMain": "5%"}}]
      }]
    }"#;

    #[test]
    fn ioreg_parsing() {
        let devs = parse_ioreg(IOREG.as_bytes());
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].id, "aa:bb:cc:dd:ee:01");
        assert_eq!(devs[0].kind, DeviceKind::Keyboard);
        assert_eq!(devs[1].kind, DeviceKind::Trackpad);
        assert_eq!(devs[1].levels, vec![(BatteryComponent::Main, 12)]);
        assert!(parse_ioreg(b"garbage").is_empty());
    }

    #[test]
    fn profiler_parsing() {
        let devs = parse_system_profiler(PROFILER.as_bytes());
        assert_eq!(devs.len(), 2, "connected devices with batteries only");
        let pods = &devs[0];
        assert_eq!(pods.kind, DeviceKind::AirPods);
        assert_eq!(
            pods.levels,
            vec![(BatteryComponent::Left, 100), (BatteryComponent::Right, 98), (BatteryComponent::Case, 45)]
        );
        assert_eq!(pods.min_level(), Some(98));
        assert!(parse_system_profiler(b"{}").is_empty());
    }

    #[test]
    fn merging_dedupes_by_address() {
        let merged = merge(parse_system_profiler(PROFILER.as_bytes()), parse_ioreg(IOREG.as_bytes()));
        let names: Vec<_> = merged.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["Amdjed's AirPods Pro", "Magic Keyboard with Touch ID", "Magic Trackpad"]);
    }
}
