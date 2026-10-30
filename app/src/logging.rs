//! Logging: human-readable stderr plus a plain-text log file
//! (`~/Library/Logs/NotchForge/notchforge.log`, rotated once at 5 MB).

use std::path::Path;
use std::sync::Mutex;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

const MAX_LOG: u64 = 5 * 1024 * 1024;

pub struct Guard;

pub fn init(log_dir: &Path, verbose: bool) -> Guard {
    let default = if verbose { "debug" } else { "info" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let path = log_dir.join("notchforge.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG) {
        let _ = std::fs::rename(&path, log_dir.join("notchforge.log.1"));
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok();
    let stderr_layer = fmt::layer().with_writer(std::io::stderr).with_target(false);
    let file_layer = file.map(|f| fmt::layer().with_ansi(false).with_writer(Mutex::new(f)));
    let _ = tracing_subscriber::registry().with(filter).with(stderr_layer).with(file_layer).try_init();
    Guard
}
