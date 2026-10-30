//! Time helpers.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds between the Unix epoch and the Core Foundation / Cocoa reference
/// date (2001-01-01T00:00:00Z). The Notification Center database stores
/// `delivered_date` relative to the latter.
pub const MAC_EPOCH_OFFSET_SECS: f64 = 978_307_200.0;

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Convert Cocoa absolute time (seconds since 2001) to Unix milliseconds.
pub fn mac_absolute_to_unix_ms(secs: f64) -> i64 {
    ((secs + MAC_EPOCH_OFFSET_SECS) * 1000.0) as i64
}

/// Human-friendly relative time: "now", "3m", "2h", "5d".
pub fn relative(timestamp_ms: i64, now_ms: i64) -> String {
    let secs = ((now_ms - timestamp_ms).max(0)) / 1000;
    match secs {
        0..=44 => "now".to_owned(),
        45..=3599 => format!("{}m", (secs + 30) / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// Spoken form for VoiceOver: "3 minutes ago".
pub fn relative_spoken(timestamp_ms: i64, now_ms: i64) -> String {
    let secs = ((now_ms - timestamp_ms).max(0)) / 1000;
    let (n, unit) = match secs {
        0..=44 => return "just now".to_owned(),
        45..=3599 => ((secs + 30) / 60, "minute"),
        3600..=86_399 => (secs / 3600, "hour"),
        _ => (secs / 86_400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// "1h 05m" style duration for battery estimates.
pub fn hours_minutes(minutes: u32) -> String {
    if minutes < 60 { format!("{minutes}m") } else { format!("{}h {:02}m", minutes / 60, minutes % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_formats() {
        let now = 10_000_000;
        assert_eq!(relative(now, now), "now");
        assert_eq!(relative(now - 90_000, now), "2m");
        assert_eq!(relative(now - 7_200_000, now), "2h");
        assert_eq!(relative(now - 3 * 86_400_000, now), "3d");
        assert_eq!(relative(now + 5000, now), "now");
        assert_eq!(relative_spoken(now - 60_000, now), "1 minute ago");
        assert_eq!(relative_spoken(now - 7_200_000, now), "2 hours ago");
    }

    #[test]
    fn mac_epoch() {
        assert_eq!(mac_absolute_to_unix_ms(0.0), 978_307_200_000);
    }

    #[test]
    fn durations() {
        assert_eq!(hours_minutes(5), "5m");
        assert_eq!(hours_minutes(125), "2h 05m");
    }
}
