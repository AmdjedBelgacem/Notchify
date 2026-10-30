//! Mac battery provider.
//!
//! macOS: IOKit power sources (public API, no permissions). If IOKit fails
//! for any reason we fall back to parsing `pmset -g batt`. Linux (headless
//! development): `/sys/class/power_supply`.

use notchforge_core::model::{BatteryStatus, ProviderStatus};
use notchforge_core::provider::{BoxFuture, Provider, ProviderContext, ProviderDescriptor, ProviderError, ProviderEvent, Wake};
use std::time::Duration;

const POLL: Duration = Duration::from_secs(30);
const POLL_NO_BATTERY: Duration = Duration::from_secs(300);

/// Parse `pmset -g batt` output.
pub fn parse_pmset(text: &str) -> Option<BatteryStatus> {
    let on_ac = text.contains("'AC Power'");
    let Some(line) = text.lines().find(|l| l.contains("InternalBattery")) else {
        return text.contains("Now drawing from").then(|| BatteryStatus {
            present: false,
            plugged_in: on_ac,
            ..BatteryStatus::default()
        });
    };
    // " -InternalBattery-0 (id=…)\t76%; discharging; 5:21 remaining present: true"
    let after_id = line.split_once(')').map_or(line, |(_, rest)| rest);
    let mut fields = after_id.split(';').map(str::trim);
    let percent = crate::util::parse_percent(fields.next()?)?;
    let state = fields.next().unwrap_or("").to_ascii_lowercase();
    let charging = state == "charging";
    let remaining = fields.next().unwrap_or("");
    let minutes = remaining
        .split_whitespace()
        .next()
        .and_then(|t| t.split_once(':'))
        .and_then(|(h, m)| Some(h.parse::<u32>().ok()? * 60 + m.parse::<u32>().ok()?))
        .filter(|m| *m > 0);
    Some(BatteryStatus {
        percent,
        charging,
        plugged_in: on_ac,
        minutes_remaining: if on_ac && !charging { None } else { minutes },
        present: !line.contains("present: false"),
    })
}

/// Parse Linux sysfs values (for headless development).
pub fn parse_sysfs(capacity: &str, status: &str, ac_online: Option<&str>) -> Option<BatteryStatus> {
    let percent = crate::util::parse_percent(capacity)?;
    let status = status.trim();
    let charging = status == "Charging";
    let plugged_in = ac_online.map_or(charging || status == "Full" || status == "Not charging", |s| s.trim() == "1");
    Some(BatteryStatus { percent, charging, plugged_in, minutes_remaining: None, present: true })
}

async fn read() -> Result<BatteryStatus, ProviderError> {
    #[cfg(target_os = "macos")]
    {
        if let Some(b) = tokio::task::spawn_blocking(crate::sys::iokit::read_internal_battery)
            .await
            .map_err(ProviderError::other)?
        {
            return Ok(b);
        }
        let out = crate::util::run_command("/usr/bin/pmset", &["-g", "batt"], Duration::from_secs(5)).await?;
        parse_pmset(&String::from_utf8_lossy(&out))
            .ok_or_else(|| ProviderError::Unavailable("unrecognised pmset output".into()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let base = std::path::Path::new("/sys/class/power_supply");
        let mut entries = tokio::fs::read_dir(base).await?;
        while let Some(e) = entries.next_entry().await? {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with("BAT") {
                continue;
            }
            let cap = tokio::fs::read_to_string(e.path().join("capacity")).await?;
            let status = tokio::fs::read_to_string(e.path().join("status")).await.unwrap_or_default();
            let ac = tokio::fs::read_to_string(base.join("AC/online")).await.ok();
            if let Some(b) = parse_sysfs(&cap, &status, ac.as_deref()) {
                return Ok(b);
            }
        }
        Ok(BatteryStatus { present: false, plugged_in: true, ..BatteryStatus::default() })
    }
}

pub struct BatteryProvider;

impl Provider for BatteryProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "battery", name: "Mac battery", permissions: &[] }
    }

    fn run(self: Box<Self>, mut ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            ctx.set_status(ProviderStatus::Running).await;
            let mut last: Option<BatteryStatus> = None;
            loop {
                let b = read().await?;
                let interval = if b.present { POLL } else { POLL_NO_BATTERY };
                if last.as_ref() != Some(&b) {
                    last = Some(b.clone());
                    if !ctx.emit(ProviderEvent::Battery(b)).await {
                        return Ok(());
                    }
                }
                if ctx.sleep_or_refresh(interval).await == Wake::Shutdown {
                    return Ok(());
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pmset_discharging() {
        let b = parse_pmset(
            "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=4653155)\t76%; discharging; 5:21 remaining present: true\n",
        )
        .expect("parse");
        assert_eq!(b, BatteryStatus { percent: 76, charging: false, plugged_in: false, minutes_remaining: Some(321), present: true });
    }

    #[test]
    fn pmset_charging_and_charged() {
        let b = parse_pmset("Now drawing from 'AC Power'\n -InternalBattery-0 (id=1)\t42%; charging; 1:05 remaining present: true")
            .expect("parse");
        assert!(b.charging && b.plugged_in);
        assert_eq!(b.minutes_remaining, Some(65));
        let b = parse_pmset("Now drawing from 'AC Power'\n -InternalBattery-0 (id=1)\t100%; charged; 0:00 remaining present: true")
            .expect("parse");
        assert!(!b.charging && b.plugged_in);
        assert_eq!(b.minutes_remaining, None);
        let b = parse_pmset("Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t50%; discharging; (no estimate) present: true")
            .expect("parse");
        assert_eq!(b.minutes_remaining, None);
    }

    #[test]
    fn pmset_desktop() {
        let b = parse_pmset("Now drawing from 'AC Power'\n").expect("parse");
        assert!(!b.present && b.plugged_in);
        assert!(parse_pmset("garbage").is_none());
    }

    #[test]
    fn sysfs() {
        let b = parse_sysfs("57\n", "Discharging\n", Some("0\n")).expect("parse");
        assert_eq!((b.percent, b.charging, b.plugged_in), (57, false, false));
        let b = parse_sysfs("100", "Full", None).expect("parse");
        assert!(b.plugged_in);
    }
}
