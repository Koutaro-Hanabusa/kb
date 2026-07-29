//! The `.index` file that gives every item in a directory a stable id.
//!
//! Each directory carries a `.index` listing its entries one per line. An item's
//! id is its line number, counting from one. Deleting an item blanks its line
//! rather than removing it, so ids are never reused and a reference written down
//! last year still points at the same note — or at nothing.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Filename of the per-directory index.
pub const INDEX_FILE: &str = ".index";

/// A directory's index: entry names by id, with gaps where items were removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Index {
    entries: Vec<Option<String>>,
}

impl Index {
    /// Read the index of `dir`, or an empty one when the file is absent.
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(INDEX_FILE);
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Ok(Self::default());
        };
        Ok(Self::parse(&raw))
    }

    pub fn parse(raw: &str) -> Self {
        let mut entries: Vec<Option<String>> = raw
            .lines()
            .map(|line| {
                let name = line.trim();
                (!name.is_empty()).then(|| name.to_string())
            })
            .collect();
        // A trailing run of blanks carries no ids worth preserving.
        while matches!(entries.last(), Some(None)) {
            entries.pop();
        }
        Self { entries }
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for entry in &self.entries {
            out.push_str(entry.as_deref().unwrap_or(""));
            out.push('\n');
        }
        out
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        let path = dir.join(INDEX_FILE);
        std::fs::write(&path, self.render()).with_context(|| format!("writing {}", path.display()))
    }

    /// The entry name for `id`, if that id is live.
    pub fn name_of(&self, id: usize) -> Option<&str> {
        if id == 0 {
            return None;
        }
        self.entries.get(id - 1)?.as_deref()
    }

    /// The id of `name`, if present.
    pub fn id_of(&self, name: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.as_deref() == Some(name))
            .map(|position| position + 1)
    }

    /// Live entries as `(id, name)`, in id order.
    pub fn entries(&self) -> impl Iterator<Item = (usize, &str)> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(position, entry)| entry.as_deref().map(|name| (position + 1, name)))
    }

    pub fn len(&self) -> usize {
        self.entries.iter().filter(|entry| entry.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Append `name` and return its new id. An existing entry keeps its id.
    pub fn add(&mut self, name: &str) -> usize {
        if let Some(id) = self.id_of(name) {
            return id;
        }
        self.entries.push(Some(name.to_string()));
        self.entries.len()
    }

    /// Blank the line holding `name`, retiring its id for good.
    pub fn remove(&mut self, name: &str) -> Option<usize> {
        let id = self.id_of(name)?;
        self.entries[id - 1] = None;
        while matches!(self.entries.last(), Some(None)) {
            self.entries.pop();
        }
        Some(id)
    }

    /// Point `name`'s id at `new_name`, preserving the id across a rename.
    pub fn rename(&mut self, name: &str, new_name: &str) -> Option<usize> {
        let id = self.id_of(name)?;
        self.entries[id - 1] = Some(new_name.to_string());
        Some(id)
    }

    /// Add anything on disk that the index does not list yet.
    ///
    /// Existing ids are left alone: reconciling must never renumber notes that
    /// were already indexed.
    pub fn reconcile(&mut self, dir: &Path) -> Result<bool> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| !is_hidden(name))
            .collect();
        names.sort();

        let mut changed = false;
        for name in names {
            if self.id_of(&name).is_none() {
                self.add(&name);
                changed = true;
            }
        }
        Ok(changed)
    }

    /// Entries whose files no longer exist.
    pub fn missing(&self, dir: &Path) -> Vec<(usize, String)> {
        self.entries()
            .filter(|(_, name)| !dir.join(name).exists())
            .map(|(id, name)| (id, name.to_string()))
            .collect()
    }
}

/// Whether a directory entry is one `nb` keeps out of the index.
fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// The index directory for a path inside a notebook.
pub fn index_path(dir: &Path) -> PathBuf {
    dir.join(INDEX_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_line_numbers() {
        let index = Index::parse("a.md\nb.md\nc.md\n");
        assert_eq!(index.name_of(1), Some("a.md"));
        assert_eq!(index.name_of(3), Some("c.md"));
        assert_eq!(index.id_of("b.md"), Some(2));
    }

    #[test]
    fn a_blank_line_is_a_retired_id() {
        // This is the shape of a real notebook: `knowledge/.index` opens with a
        // blank line where note 1 used to be.
        let index = Index::parse("\nb.md\nc.md\n");
        assert_eq!(index.name_of(1), None);
        assert_eq!(index.name_of(2), Some("b.md"));
        assert_eq!(index.len(), 2);
    }

    #[test]
    fn id_zero_never_resolves() {
        let index = Index::parse("a.md\n");
        assert_eq!(index.name_of(0), None);
        assert_eq!(index.name_of(2), None);
    }

    #[test]
    fn removing_retires_the_id_without_shifting_others() {
        let mut index = Index::parse("a.md\nb.md\nc.md\n");
        assert_eq!(index.remove("b.md"), Some(2));
        assert_eq!(index.name_of(2), None);
        // c.md must still answer to 3, or every id ever written down breaks.
        assert_eq!(index.name_of(3), Some("c.md"));
        assert_eq!(index.render(), "a.md\n\nc.md\n");
    }

    #[test]
    fn a_new_entry_never_reuses_a_retired_id() {
        let mut index = Index::parse("a.md\nb.md\nc.md\n");
        index.remove("b.md");
        assert_eq!(index.add("d.md"), 4);
        assert_eq!(index.name_of(2), None);
    }

    #[test]
    fn adding_an_existing_entry_keeps_its_id() {
        let mut index = Index::parse("a.md\nb.md\n");
        assert_eq!(index.add("a.md"), 1);
        assert_eq!(index.len(), 2);
    }

    #[test]
    fn renaming_preserves_the_id() {
        let mut index = Index::parse("a.md\nb.md\n");
        assert_eq!(index.rename("a.md", "renamed.md"), Some(1));
        assert_eq!(index.name_of(1), Some("renamed.md"));
        assert_eq!(index.id_of("a.md"), None);
    }

    #[test]
    fn round_trips_through_render() {
        let raw = "\na.md\n\nfolder\nc.md\n";
        assert_eq!(Index::parse(raw).render(), raw);
    }

    #[test]
    fn trailing_blanks_are_dropped() {
        let index = Index::parse("a.md\n\n\n");
        assert_eq!(index.render(), "a.md\n");
    }

    #[test]
    fn reconcile_adds_only_what_is_missing() {
        let dir = std::env::temp_dir().join(format!("kb-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a.md", "b.md", ".hidden.md"] {
            std::fs::write(dir.join(name), "x").unwrap();
        }

        let mut index = Index::parse("a.md\n");
        assert!(index.reconcile(&dir).unwrap());
        assert_eq!(index.id_of("a.md"), Some(1)); // untouched
        assert_eq!(index.id_of("b.md"), Some(2));
        assert_eq!(index.id_of(".hidden.md"), None);
        assert!(!index.reconcile(&dir).unwrap()); // idempotent

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_reports_entries_without_files() {
        let dir = std::env::temp_dir().join(format!("kb-index-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.md"), "x").unwrap();

        let index = Index::parse("a.md\ngone.md\n");
        assert_eq!(index.missing(&dir), vec![(2, "gone.md".to_string())]);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
