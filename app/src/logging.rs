//! Logging: human-readable stderr plus a plain-text log file
//! (`~/Library/Logs/NotchForge/notchforge.log`, rotated once at 5 MB).

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

const MAX_LOG: u64 = 5 * 1024 * 1024;

pub struct Guard;

/// The log file, opened again if it's deleted while the app runs. Otherwise
/// everything would keep going to a file no one can read any more.
struct LogFile {
    path: PathBuf,
    file: File,
}

fn open_log(path: &Path) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

impl Write for LogFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.path.exists() {
            self.file = open_log(&self.path)?;
        }
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

pub fn init(log_dir: &Path, verbose: bool) -> Guard {
    let default = if verbose { "debug" } else { "info" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let path = log_dir.join("notchforge.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG) {
        let _ = std::fs::rename(&path, log_dir.join("notchforge.log.1"));
    }
    let file = open_log(&path).ok().map(|file| LogFile { path, file });
    let stderr_layer = fmt::layer().with_writer(std::io::stderr).with_target(false);
    let file_layer = file.map(|f| fmt::layer().with_ansi(false).with_writer(Mutex::new(f)));
    let _ = tracing_subscriber::registry().with(filter).with(stderr_layer).with(file_layer).try_init();
    Guard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deleted_log_is_opened_again() {
        let dir = std::env::temp_dir().join(format!("notchforge-log-{}", std::process::id()));
        let path = dir.join("logs").join("notchforge.log");
        let mut log = LogFile { path: path.clone(), file: open_log(&path).expect("opens") };
        log.write_all(b"before\n").expect("writes");
        std::fs::remove_dir_all(&dir).expect("removes");
        log.write_all(b"after\n").expect("writes");
        assert_eq!(std::fs::read_to_string(&path).expect("reads"), "after\n");
        std::fs::remove_dir_all(&dir).expect("removes");
    }
}
