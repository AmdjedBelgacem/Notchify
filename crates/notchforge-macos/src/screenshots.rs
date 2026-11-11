//! Screenshots straight into the tray.
//!
//! macOS marks every screenshot (⌘⇧3 / ⌘⇧4 / ⌘⇧5, and the Touch Bar's
//! ones) with the `com.apple.metadata:kMDItemIsScreenShot` extended
//! attribute — Finder and Spotlight both key off it. The Desktop is scanned
//! every few seconds; a new file carrying that mark lands in the tray, no
//! matter what the screenshot is named or which language the system speaks.

use std::collections::HashSet;
use std::path::PathBuf;

const XATTR_SCREENSHOT: &[u8] = b"com.apple.metadata:kMDItemIsScreenShot";

#[link(name = "c")]
unsafe extern "C" {
    fn getxattr(
        path: *const std::ffi::c_char,
        name: *const std::ffi::c_char,
        value: *mut u8,
        size: usize,
        position: u32,
        options: i32,
    ) -> isize;
}

/// Does this file carry the screenshot mark?
fn is_screenshot(path: &std::path::Path) -> bool {
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else { return false };
    let Ok(name) = std::ffi::CString::new(XATTR_SCREENSHOT) else { return false };
    // SAFETY: both C strings outlive the call; the buffer is only read when
    // the attribute exists (any length means yes).
    let n = unsafe { getxattr(c.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, 0) };
    n >= 0
}

/// The directories to watch: the user's own Desktop (screenshots) and
/// Downloads (new files, once they stopped growing).
fn watched_dirs() -> Vec<(PathBuf, Kind)> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Vec::new() };
    vec![(home.join("Desktop"), Kind::Screenshot), (home.join("Downloads"), Kind::Download)]
}

enum Kind {
    /// Only files carrying the screenshot xattr.
    Screenshot,
    /// Any finished file (browser partials skipped by extension).
    Download,
}

impl Kind {
    fn accepts(&self, path: &std::path::Path) -> bool {
        match self {
            Kind::Screenshot => is_screenshot(path),
            Kind::Download => !matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("download") | Some("crdownload") | Some("part") | Some("partial")
            ),
        }
    }
}

/// New screenshots since the last scan. Files older than `since_ms` (app
/// start) never count, so a restart does not re-shelve the whole Desktop.
pub struct Watcher {
    seen: HashSet<PathBuf>,
    since_ms: i64,
}

impl Watcher {
    pub fn new(now_ms: i64) -> Self {
        Self { seen: HashSet::new(), since_ms: now_ms }
    }

    /// Scan the watched directories; returns new files worth shelving.
    /// Downloads must have sat still for a few seconds (no partials).
    pub fn scan(&mut self) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for (dir, kind) in watched_dirs() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if self.seen.contains(&path) || !path.is_file() {
                    continue;
                }
                self.seen.insert(path.clone());
                let Ok(meta) = entry.metadata() else { continue };
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                if modified < self.since_ms {
                    continue;
                }
                if matches!(kind, Kind::Download)
                    && std::time::SystemTime::now()
                        .duration_since(meta.modified().unwrap_or(std::time::UNIX_EPOCH))
                        .is_ok_and(|d| d < std::time::Duration::from_secs(10))
                {
                    // Still being written: forget it so the next scan can take it.
                    self.seen.remove(&path);
                    continue;
                }
                if kind.accepts(&path) {
                    found.push(path);
                }
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_only_real_screenshots() {
        // A text file does not carry the mark; the fixture screenshot does.
        let dir = std::env::temp_dir().join(format!("nf-sshot-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let plain = dir.join("plain.txt");
        std::fs::write(&plain, b"hello").expect("write");
        assert!(!is_screenshot(&plain));
        if let Ok(out) = std::process::Command::new("xattr")
            .args(["-w", "com.apple.metadata:kMDItemIsScreenShot", "bplist00"])
            .arg(&plain)
            .status()
        {
            if out.success() {
                assert!(is_screenshot(&plain), "a file with the mark is a screenshot");
            }
        }
        let _ = std::fs::remove_file(&plain);
        let _ = std::fs::remove_dir(&dir);
    }
}
