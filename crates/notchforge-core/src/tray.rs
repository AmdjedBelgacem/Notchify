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
                    text: None,
                    pinned: false,
                },
            );
            added += 1;
        }
        self.trim(max_items);
        added
    }

    /// Shelve a clipboard entry (text or a link). Duplicate text moves to the
    /// front. Returns whether anything changed.
    pub fn add_text(&mut self, text: String, max_items: usize, now_ms: i64) -> bool {
        let trimmed = text.trim().to_owned();
        if trimmed.is_empty() {
            return false;
        }
        if let Some(pos) = self.items.iter().position(|i| i.text.as_deref() == Some(trimmed.as_str())) {
            let mut item = self.items.remove(pos);
            item.added_at_ms = now_ms;
            self.items.insert(0, item);
            return true;
        }
        // The first line stands in for the name; a bare URL names itself.
        let name = single_line(&trimmed, 48);
        // History cap: at most 8 unpinned text items, oldest dropped first.
        const MAX_TEXTS: usize = 8;
        let mut texts = self.items.iter().filter(|i| i.text.is_some() && !i.pinned).count();
        while texts >= MAX_TEXTS {
            let Some(pos) = self.items.iter().rposition(|i| i.text.is_some() && !i.pinned) else { break };
            self.items.remove(pos);
            texts -= 1;
        }
        self.next_id += 1;
        let first_unpinned = self.items.iter().position(|i| !i.pinned).unwrap_or(self.items.len());
        self.items.insert(
            first_unpinned,
            TrayItem {
                id: TrayItemId(self.next_id),
                path: PathBuf::new(),
                name,
                is_dir: false,
                bytes: Some(trimmed.len() as u64),
                added_at_ms: now_ms,
                text: Some(trimmed),
                pinned: false,
            },
        );
        self.trim(max_items);
        true
    }

    /// Pin or unpin an item; pinned ones float to the top. Returns the new
    /// state.
    pub fn toggle_pin(&mut self, id: TrayItemId) -> bool {
        let Some(item) = self.items.iter_mut().find(|i| i.id == id) else { return false };
        item.pinned = !item.pinned;
        true
    }

    /// Keep the pinned items and the newest `max_items` of the rest.
    fn trim(&mut self, max_items: usize) {
        if self.items.len() <= max_items {
            return;
        }
        let (pinned, rest): (Vec<_>, Vec<_>) = self.items.drain(..).partition(|i| i.pinned);
        let keep = rest.into_iter().take(max_items.max(pinned.len()) - pinned.len().min(max_items));
        self.items = pinned.into_iter().chain(keep).collect();
    }

    /// Is this a shelved link (a bare http(s) URL)?
    pub fn is_link(item: &TrayItem) -> bool {
        let Some(t) = &item.text else { return false };
        t.len() <= 2048
            && (t.starts_with("http://") || t.starts_with("https://"))
            && t[8..].chars().all(|c| !c.is_whitespace())
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

    /// Remove items whose file no longer exists. Text items and pinned ones
    /// stay. Returns `true` if anything changed.
    pub fn prune_missing(&mut self) -> bool {
        let before = self.items.len();
        self.items.retain(|i| i.text.is_some() || i.pinned || i.path.exists());
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

/// The text's first line, flattened to one line and capped with an ellipsis.
fn single_line(text: &str, cap: usize) -> String {
    let flat: String = text.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
    let flat = flat.trim().to_owned();
    if flat.chars().count() <= cap {
        flat
    } else {
        let mut s: String = flat.chars().take(cap).collect();
        s.push('…');
        s
    }
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
    fn shelves_text_and_dedupes() {
        let mut t = Tray::new();
        assert!(t.add_text("  hello\nworld  ".into(), 5, 1));
        assert_eq!(t.items()[0].name, "hello world");
        assert_eq!(t.items()[0].text.as_deref(), Some("hello\nworld"));
        assert_eq!(t.items()[0].bytes, Some(11));
        // Whitespace-only is skipped.
        assert!(!t.add_text("   \n ".into(), 5, 2));
        // Same text moves to the front instead of duplicating.
        t.add_text("second".into(), 5, 3);
        assert_eq!(t.items()[0].name, "second");
        assert_eq!(t.items().len(), 2);
        assert!(t.add_text("hello\nworld".into(), 5, 4));
        assert_eq!(t.items().len(), 2);
        assert_eq!(t.items()[0].text.as_deref(), Some("hello\nworld"));
        // Links are recognized.
        assert!(Tray::is_link(t.items().first().filter(|_| false).unwrap_or(&TrayItem {
            id: TrayItemId(0),
            path: PathBuf::new(),
            name: "x".into(),
            is_dir: false,
            bytes: None,
            added_at_ms: 0,
            text: Some("https://example.com".into()),
            pinned: false,
        })));
        // Text items survive pruning (their "path" is empty).
        assert!(!t.prune_missing());
        assert_eq!(t.items().len(), 2);
    }

    #[test]
    fn text_names_are_capped() {
        let mut t = Tray::new();
        let long = "x".repeat(80);
        t.add_text(long.clone(), 3, 0);
        let name = &t.items()[0].name;
        assert!(name.chars().count() <= 49, "{name}");
        assert!(name.ends_with('…'));
        // Persisted round trip keeps text items.
        let d = tmpdir("text");
        let state = d.join("tray.json");
        t.save(&state).expect("save");
        let loaded = Tray::load(&state).expect("load");
        assert_eq!(loaded.items()[0].text.as_deref(), Some(long.as_str()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bytes_format() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1_500_000), "1.5 MB");
    }
}
