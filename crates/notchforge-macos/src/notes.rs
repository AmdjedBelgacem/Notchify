//! Notes typed into the Planner page, kept on this Mac in `notes.json` beside
//! the app's other data. Nothing leaves the Mac.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub id: u64,
    pub text: String,
    pub updated_ms: i64,
}

impl Note {
    /// Its first line, as a title.
    pub fn title(&self) -> &str {
        self.text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Empty note")
    }
}

/// All notes, the most recently changed first.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notes {
    pub notes: Vec<Note>,
}

impl Notes {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("notes.json")
    }

    pub fn load(data_dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(data_dir))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) {
        let path = Self::path(data_dir);
        let tmp = path.with_extension("json.tmp");
        let Ok(text) = serde_json::to_string_pretty(self) else { return };
        if std::fs::write(&tmp, text).is_ok() {
            if let Err(e) = std::fs::rename(&tmp, &path) {
                tracing::warn!(error = %e, "could not save notes");
            }
        }
    }

    /// Start a new, empty note at the top and return its id.
    pub fn create(&mut self, now_ms: i64) -> u64 {
        let id = self.notes.iter().map(|n| n.id).max().unwrap_or(0) + 1;
        self.notes.insert(0, Note { id, text: String::new(), updated_ms: now_ms });
        id
    }

    /// Change a note's text; it moves to the top. An emptied note is removed,
    /// unless it's the only one.
    pub fn set_text(&mut self, id: u64, text: &str, now_ms: i64) {
        let Some(i) = self.notes.iter().position(|n| n.id == id) else { return };
        let mut note = self.notes.remove(i);
        if text.trim().is_empty() && !self.notes.is_empty() {
            return;
        }
        note.text = text.to_owned();
        note.updated_ms = now_ms;
        self.notes.insert(0, note);
    }

    /// Bring a note to the top, to edit it.
    pub fn open(&mut self, id: u64) {
        if let Some(i) = self.notes.iter().position(|n| n.id == id) {
            let note = self.notes.remove(i);
            self.notes.insert(0, note);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_keep_the_latest_on_top_and_survive_a_reload() {
        let dir = std::env::temp_dir().join(format!("notchforge-notes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut n = Notes::default();
        let a = n.create(1);
        n.set_text(a, "Release checklist\n- tag", 2);
        let b = n.create(3);
        n.set_text(b, "Ideas", 4);
        assert_eq!(n.notes[0].title(), "Ideas");
        n.open(a);
        assert_eq!(n.notes[0].title(), "Release checklist");
        n.save(&dir);
        assert_eq!(Notes::load(&dir), n);
        n.set_text(b, "  ", 5);
        assert_eq!(n.notes.len(), 1, "an emptied note goes");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
