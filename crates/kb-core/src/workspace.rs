//! Notebook discovery and note enumeration.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;

use crate::note::Note;

/// Environment variable that overrides the knowledge base location.
pub const ROOT_ENV: &str = "KB_ROOT";

/// Environment variable that overrides which notebook new notes go to.
pub const NOTEBOOK_ENV: &str = "KB_NOTEBOOK";

/// Marker file in the home directory that identifies a work machine.
const WORK_MARKER: &str = ".is_work_pc";

/// File at the knowledge base root naming the selected notebook.
pub const CURRENT_FILE: &str = ".current";

/// A single notebook — one directory of Markdown, normally one git repository.
#[derive(Debug, Clone)]
pub struct Notebook {
    pub name: String,
    pub root: PathBuf,
}

impl Notebook {
    /// Absolute paths of every Markdown file in the notebook.
    pub fn note_paths(&self) -> Vec<PathBuf> {
        WalkBuilder::new(&self.root)
            .hidden(true)
            .git_ignore(true)
            .build()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
            .map(ignore::DirEntry::into_path)
            .filter(|path| is_markdown(path))
            .collect()
    }

    /// Read and parse every note. Unreadable files are skipped rather than
    /// failing the whole walk.
    pub fn notes(&self) -> Vec<Note> {
        self.note_paths().into_iter().filter_map(|path| self.read(&path).ok()).collect()
    }

    pub fn read(&self, path: &Path) -> Result<Note> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        Ok(Note::parse(&raw, path, &self.name, &self.relative(path)))
    }

    pub fn relative(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.root).unwrap_or(path).to_path_buf()
    }
}

/// The whole knowledge base: every notebook under one root.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
    pub notebooks: Vec<Notebook>,
}

impl Workspace {
    /// Locate the knowledge base, honouring `$KB_ROOT` and falling back to
    /// `~/.nb`.
    pub fn discover() -> Result<Self> {
        let root = match std::env::var_os(ROOT_ENV) {
            Some(value) => PathBuf::from(value),
            None => home_dir().context("cannot determine the home directory")?.join(".nb"),
        };
        Self::open(root)
    }

    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        if !root.is_dir() {
            bail!("knowledge base not found at {}", root.display());
        }

        let mut notebooks: Vec<Notebook> = std::fs::read_dir(root)
            .with_context(|| format!("reading {}", root.display()))?
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                (!name.starts_with('.')).then(|| Notebook { name, root: entry.path() })
            })
            .collect();
        notebooks.sort_by(|a, b| a.name.cmp(&b.name));

        if notebooks.is_empty() {
            bail!("no notebooks found under {}", root.display());
        }
        Ok(Self { root: root.to_path_buf(), notebooks })
    }

    pub fn notebook(&self, name: &str) -> Option<&Notebook> {
        self.notebooks.iter().find(|nb| nb.name == name)
    }

    /// The notebooks to operate on: one by name, or all of them.
    pub fn select(&self, name: Option<&str>) -> Result<Vec<&Notebook>> {
        match name {
            Some(name) => {
                let notebook = self.notebook(name).with_context(|| {
                    format!(
                        "unknown notebook `{name}` (have: {})",
                        self.notebooks
                            .iter()
                            .map(|nb| nb.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?;
                Ok(vec![notebook])
            }
            None => Ok(self.notebooks.iter().collect()),
        }
    }

    /// The notebook commands act on when none is named.
    ///
    /// `$KB_NOTEBOOK` wins, then the notebook `kb use` selected, then the
    /// machine's own default — work and personal notes live in separate
    /// repositories, and putting one in the other is tedious to undo.
    pub fn default_notebook(&self) -> Result<&Notebook> {
        if let Some(name) = std::env::var_os(NOTEBOOK_ENV) {
            let name = name.to_string_lossy().into_owned();
            return self
                .notebook(&name)
                .with_context(|| format!("${NOTEBOOK_ENV} names an unknown notebook `{name}`"));
        }
        if let Some(selected) = self.current().and_then(|name| self.notebook(&name)) {
            return Ok(selected);
        }
        let preferred = if is_work_machine() { "work" } else { "home" };
        self.notebook(preferred)
            .or_else(|| self.notebooks.first())
            .context("no notebooks available")
    }

    /// The notebook name recorded by `kb use`, if it still exists.
    pub fn current(&self) -> Option<String> {
        let raw = std::fs::read_to_string(self.root.join(CURRENT_FILE)).ok()?;
        let name = raw.trim().to_string();
        (!name.is_empty()).then_some(name)
    }

    /// Record `name` as the selected notebook.
    pub fn set_current(&self, name: &str) -> Result<()> {
        self.notebook(name)
            .with_context(|| format!("unknown notebook `{name}`"))?;
        let path = self.root.join(CURRENT_FILE);
        std::fs::write(&path, format!("{name}\n"))
            .with_context(|| format!("writing {}", path.display()))
    }

    pub fn notes(&self, notebook: Option<&str>) -> Result<Vec<Note>> {
        Ok(self.select(notebook)?.into_iter().flat_map(Notebook::notes).collect())
    }
}

fn is_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|ext| {
        ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
    })
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn is_work_machine() -> bool {
    home_dir().is_some_and(|home| home.join(WORK_MARKER).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a throwaway knowledge base on disk.
    fn fixture(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("kb-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, contents) in files {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, contents).unwrap();
        }
        root
    }

    #[test]
    fn discovers_notebooks_and_skips_hidden_directories() {
        let root = fixture("discover", &[
            ("home/knowledge/a.md", "# A"),
            ("work/b.md", "# B"),
            (".cache/c.md", "# C"),
        ]);
        let ws = Workspace::open(&root).unwrap();
        let names: Vec<&str> = ws.notebooks.iter().map(|nb| nb.name.as_str()).collect();
        assert_eq!(names, vec!["home", "work"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn walks_markdown_and_ignores_everything_else() {
        let root = fixture("walk", &[
            ("home/a.md", "# A"),
            ("home/nested/b.markdown", "# B"),
            ("home/notes.txt", "not a note"),
            ("home/.hidden/c.md", "# hidden"),
        ]);
        let ws = Workspace::open(&root).unwrap();
        let mut titles: Vec<String> =
            ws.notes(Some("home")).unwrap().into_iter().map(|n| n.title).collect();
        titles.sort();
        assert_eq!(titles, vec!["A", "B"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn selecting_an_unknown_notebook_is_an_error() {
        let root = fixture("select", &[("home/a.md", "# A")]);
        let ws = Workspace::open(&root).unwrap();
        assert!(ws.select(Some("nope")).is_err());
        assert_eq!(ws.select(None).unwrap().len(), 1);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn use_selects_the_default_notebook() {
        let root = fixture("current", &[("home/a.md", "# A"), ("work/b.md", "# B")]);
        let ws = Workspace::open(&root).unwrap();

        // Written by `nb use`; kb reads the same file.
        std::fs::write(root.join(CURRENT_FILE), "work\n").unwrap();
        assert_eq!(ws.current().as_deref(), Some("work"));
        assert_eq!(ws.default_notebook().unwrap().name, "work");

        ws.set_current("home").unwrap();
        assert_eq!(ws.default_notebook().unwrap().name, "home");
        assert!(ws.set_current("nope").is_err());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_stale_current_falls_through_to_the_machine_default() {
        let root = fixture("stale", &[("home/a.md", "# A")]);
        std::fs::write(root.join(CURRENT_FILE), "deleted-notebook\n").unwrap();
        let ws = Workspace::open(&root).unwrap();
        assert_eq!(ws.default_notebook().unwrap().name, "home");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn notes_carry_their_notebook_relative_path() {
        let root = fixture("relpath", &[("home/knowledge/deep/a.md", "# A")]);
        let ws = Workspace::open(&root).unwrap();
        let notes = ws.notes(None).unwrap();
        assert_eq!(notes[0].rel_path, Path::new("knowledge/deep/a.md"));
        assert_eq!(notes[0].notebook, "home");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
