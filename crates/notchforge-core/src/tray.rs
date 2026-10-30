//! Temporary file tray.
//!
//! The tray holds *references* to dropped files (it never copies or moves
//! them), so dragging out of the tray hands the original file to the target,
//! exactly like dragging from Finder. Missing files are pruned on access.

use crate::error::{CoreError, Result};
use crate::model::{TrayItem, TrayItemId};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Tray {
    next_id: u64,
    items: Vec<TrayItem>,
}

impl Tray {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add paths; duplicates (same path) are moved to the front instead.
    /// Returns how many new items were added.
    pub fn add<I, P>(&mut self, paths: I, max_items: usize, now_ms: i64) -> usize
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut added = 0;
        for path in paths {
            let path = path.as_ref();
            if let Some(pos) = self.items.iter().position(|i| i.path == path) {
                let mut item = self.items.remove(pos);
                item.added_at_ms = now_ms;
                self.items.insert(0, item);
                continue;
            }
            let meta = std::fs::metadata(path).ok();
            let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
            let bytes = meta.as_ref().filter(|m| m.is_file()).map(|m| m.len());
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            self.next_id += 1;
            self.items.insert(
                0,
                TrayItem {
                    id: TrayItemId(self.next_id),
                    path: path.to_path_buf(),
                    name,
                    is_dir,
                    bytes,
                    added_at_ms: now_ms,
                },
            );
            added += 1;
        }
        self.items.truncate(max_items);
        added
    }

    pub fn remove(&mut self, id: TrayItemId) -> Option<TrayItem> {
        let pos = self.items.iter().position(|i| i.id == id)?;
        Some(self.items.remove(pos))
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn get(&self, id: TrayItemId) -> Option<&TrayItem> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Remove items whose file no longer exists. Returns `true` if anything changed.
    pub fn prune_missing(&mut self) -> bool {
        let before = self.items.len();
        self.items.retain(|i| i.path.exists());
        before != self.items.len()
    }

    pub fn items(&self) -> &[TrayItem] {
        &self.items
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        self.items.iter().map(|i| i.path.clone()).collect()
    }

    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let mut tray: Tray = serde_json::from_str(&text)?;
                tray.prune_missing();
                Ok(tray)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Tray::new()),
            Err(e) => Err(CoreError::io(path, e)),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text).map_err(|e| CoreError::io(path, e))
    }
}

/// "1.2 MB" style size formatting.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nf-tray-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("mkdir");
        d
    }

    #[test]
    fn add_dedupes_and_orders_newest_first() {
        let d = tmpdir("add");
        let a = d.join("a.txt");
        let b = d.join("b.txt");
        std::fs::write(&a, "hello").expect("write");
        std::fs::write(&b, "").expect("write");
        let mut t = Tray::new();
        assert_eq!(t.add([&a, &b], 10, 1), 2);
        assert_eq!(t.items()[0].name, "b.txt");
        assert_eq!(t.add([&a], 10, 2), 0);
        assert_eq!(t.items()[0].name, "a.txt");
        assert_eq!(t.items()[0].bytes, Some(5));
        assert_eq!(t.items().len(), 2);
        assert_eq!(t.add([&d], 10, 3), 1);
        assert!(t.items()[0].is_dir);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn capacity_and_removal() {
        let d = tmpdir("cap");
        let paths: Vec<_> = (0..5).map(|i| d.join(format!("{i}"))).collect();
        let mut t = Tray::new();
        t.add(&paths, 3, 0);
        assert_eq!(t.items().len(), 3);
        let id = t.items()[0].id;
        assert!(t.remove(id).is_some());
        assert!(t.remove(id).is_none());
        // None of these files exist, so pruning empties the tray.
        assert!(t.prune_missing());
        assert!(t.items().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn persistence_round_trip() {
        let d = tmpdir("persist");
        let f = d.join("keep.bin");
        std::fs::write(&f, [0u8; 3]).expect("write");
        let mut t = Tray::new();
        t.add([&f], 10, 0);
        let state = d.join("tray.json");
        t.save(&state).expect("save");
        let loaded = Tray::load(&state).expect("load");
        assert_eq!(loaded.paths(), vec![f.clone()]);
        assert!(Tray::load(&d.join("missing.json")).expect("empty").items().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bytes_format() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1_500_000), "1.5 MB");
    }
}
