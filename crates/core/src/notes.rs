//! Quick notes: a small diary of plain-text pages, stored as one JSON file in the data folder.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    /// Unix milliseconds
    pub created: u64,
    pub updated: u64,
    pub text: String,
}

/// All notes, newest first, persisted to `notes.json`.
pub struct Notebook {
    path: PathBuf,
    notes: Vec<Note>,
}

impl Notebook {
    pub fn load(path: PathBuf) -> Self {
        let mut notes: Vec<Note> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        notes.sort_by_key(|n| std::cmp::Reverse(n.updated));
        Self { path, notes }
    }

    pub fn notes(&self) -> &[Note] {
        &self.notes
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.notes).unwrap_or_else(|_| "[]".into())
    }

    /// Create or update a note. Saving an empty page deletes it, so blank pages never pile up.
    pub fn save(&mut self, id: &str, created: u64, text: &str, now: u64) -> std::io::Result<()> {
        if text.trim().is_empty() {
            return self.delete(id);
        }
        match self.notes.iter_mut().find(|n| n.id == id) {
            Some(n) if n.text == text => return Ok(()),
            Some(n) => {
                n.text = text.to_string();
                n.updated = now;
            }
            None => self.notes.push(Note {
                id: id.to_string(),
                created,
                updated: now,
                text: text.to_string(),
            }),
        }
        self.notes.sort_by_key(|n| std::cmp::Reverse(n.updated));
        self.write()
    }

    pub fn delete(&mut self, id: &str) -> std::io::Result<()> {
        let before = self.notes.len();
        self.notes.retain(|n| n.id != id);
        if self.notes.len() == before {
            return Ok(());
        }
        self.write()
    }

    fn write(&self) -> std::io::Result<()> {
        write_atomic(&self.path, &self.to_json())
    }
}

fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_update_delete_and_reload() {
        let dir = std::env::temp_dir().join(format!("shoji_notes_{}", std::process::id()));
        let path = dir.join("notes.json");
        let mut book = Notebook::load(path.clone());
        assert!(book.notes().is_empty());

        book.save("a", 1, "first", 10).unwrap();
        book.save("b", 2, "second", 20).unwrap();
        book.save("a", 1, "first, edited", 30).unwrap();
        assert_eq!(book.notes()[0].id, "a", "most recently edited first");

        book.save("b", 2, "   ", 40).unwrap();
        assert_eq!(book.notes().len(), 1, "blank page is removed");

        let reloaded = Notebook::load(path);
        assert_eq!(reloaded.notes()[0].text, "first, edited");
        let _ = std::fs::remove_dir_all(dir);
    }
}
