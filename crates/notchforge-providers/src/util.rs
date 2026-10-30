//! Small helpers shared by providers.

use notchforge_core::provider::ProviderError;
use std::time::Duration;

/// Run a command with a timeout and return stdout. The child is killed if the
/// timeout elapses (`kill_on_drop`).
pub async fn run_command(program: &str, args: &[&str], timeout: Duration) -> Result<Vec<u8>, ProviderError> {
    let child = tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(timeout, child)
        .await
        .map_err(|_| ProviderError::Unavailable(format!("{program} timed out")))??;
    if !out.status.success() {
        return Err(ProviderError::Unavailable(format!("{program} exited with {}", out.status)));
    }
    Ok(out.stdout)
}

/// `"85%"` / `"85"` / `85` → `Some(85)`, clamped to 0–100.
pub fn parse_percent(s: &str) -> Option<u8> {
    let digits: String = s.trim().trim_end_matches('%').trim().chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u32>().ok().map(|v| v.min(100) as u8)
}

/// Normalise a Bluetooth address: `AA-BB-CC…` / `aa:bb:cc…` → `aa:bb:cc…`.
pub fn normalize_address(addr: &str) -> String {
    addr.trim().to_ascii_lowercase().replace('-', ":")
}

/// Truncate to `max` characters on a char boundary, adding an ellipsis.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent() {
        assert_eq!(parse_percent("85%"), Some(85));
        assert_eq!(parse_percent(" 7 % "), Some(7));
        assert_eq!(parse_percent("150"), Some(100));
        assert_eq!(parse_percent("n/a"), None);
    }

    #[test]
    fn addresses() {
        assert_eq!(normalize_address("AA-BB-CC-DD-EE-FF"), "aa:bb:cc:dd:ee:ff");
    }

    #[test]
    fn truncation() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("héllo wörld", 6), "héllo…");
    }
}
