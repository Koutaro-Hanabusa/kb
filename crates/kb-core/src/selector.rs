//! Parsing and resolving `nb`-style item references.
//!
//! Almost every command takes the same shape of argument:
//!
//! ```text
//! [<notebook>:][<folder-path>/][<id> | <filename> | <title>]
//! ```
//!
//! so parsing lives here once rather than in each command.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};

use crate::index::Index;
use crate::note::Note;
use crate::workspace::{Notebook, Workspace};

/// What the trailing part of a selector names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A bare number — an index id.
    Id(usize),
    /// Anything else: matched against filenames first, then titles.
    Name(String),
}

/// A parsed reference to an item, folder, or notebook.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selector {
    pub notebook: Option<String>,
    /// Folder components between the notebook and the target.
    pub folder: Vec<String>,
    pub target: Option<Target>,
}

impl Selector {
    /// Parse a selector.
    ///
    /// The notebook prefix ends at the first colon, so `home:knowledge/3` splits
    /// into notebook `home`, folder `knowledge`, target `3`. A trailing slash
    /// means the whole path is folders and there is no target.
    pub fn parse(input: &str) -> Self {
        let input = input.trim();
        if input.is_empty() {
            return Self::default();
        }

        let (notebook, rest) = match input.split_once(':') {
            // A Windows-style drive letter or a URL scheme is not a notebook;
            // requiring a non-empty name on both sides keeps those out.
            Some((name, rest)) if !name.is_empty() && !rest.starts_with("//") => {
                (Some(name.to_string()), rest)
            }
            _ => (None, input),
        };

        let trailing_slash = rest.ends_with('/');
        let mut parts: Vec<String> =
            rest.split('/').filter(|part| !part.is_empty()).map(str::to_string).collect();

        let target = if trailing_slash { None } else { parts.pop().map(Self::classify) };

        Self { notebook, folder: parts, target }
    }

    fn classify(part: String) -> Target {
        match part.parse::<usize>() {
            Ok(id) if !part.is_empty() => Target::Id(id),
            _ => Target::Name(part),
        }
    }

    /// Whether this names only a notebook, e.g. `home:`.
    pub fn is_notebook_only(&self) -> bool {
        self.notebook.is_some() && self.folder.is_empty() && self.target.is_none()
    }

    /// The folder path relative to the notebook root.
    pub fn folder_path(&self) -> PathBuf {
        self.folder.iter().collect()
    }
}

/// What a selector resolved to.
#[derive(Debug, Clone)]
pub enum Resolved {
    Note { path: PathBuf, id: Option<usize> },
    Folder { path: PathBuf, id: Option<usize> },
    Notebook { name: String, root: PathBuf },
}

impl Resolved {
    pub fn path(&self) -> &Path {
        match self {
            Self::Note { path, .. } | Self::Folder { path, .. } => path,
            Self::Notebook { root, .. } => root,
        }
    }

    pub fn is_note(&self) -> bool {
        matches!(self, Self::Note { .. })
    }
}

/// Resolve `selector` against the workspace.
pub fn resolve(workspace: &Workspace, selector: &Selector) -> Result<Resolved> {
    let notebook = match &selector.notebook {
        Some(name) => workspace
            .notebook(name)
            .ok_or_else(|| anyhow!("notebook not found: {name}"))?,
        None => workspace.default_notebook()?,
    };

    if selector.is_notebook_only() {
        return Ok(Resolved::Notebook {
            name: notebook.name.clone(),
            root: notebook.root.clone(),
        });
    }

    let dir = notebook.root.join(selector.folder_path());
    if !dir.is_dir() {
        bail!("not found: {}", describe(selector));
    }

    let Some(target) = &selector.target else {
        return Ok(Resolved::Folder { path: dir, id: None });
    };

    let index = Index::load(&dir)?;
    let (name, id) = match target {
        Target::Id(id) => {
            let name = index
                .name_of(*id)
                .ok_or_else(|| anyhow!("not found: {}", describe(selector)))?;
            (name.to_string(), Some(*id))
        }
        Target::Name(name) => {
            let resolved = find_by_name(&dir, notebook, name)
                .ok_or_else(|| anyhow!("not found: {}", describe(selector)))?;
            let id = index.id_of(&resolved);
            (resolved, id)
        }
    };

    let path = dir.join(&name);
    if path.is_dir() {
        Ok(Resolved::Folder { path, id })
    } else {
        Ok(Resolved::Note { path, id })
    }
}

/// Match a name against filenames, then titles.
///
/// `nb` accepts a filename with or without its extension, and falls back to
/// matching the note's title, so a reference can be whatever the user remembers.
fn find_by_name(dir: &Path, notebook: &Notebook, needle: &str) -> Option<String> {
    let entries: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .collect();

    // Exact filename.
    if let Some(hit) = entries.iter().find(|name| name.as_str() == needle) {
        return Some(hit.clone());
    }
    // Filename without its extension.
    if let Some(hit) = entries.iter().find(|name| {
        Path::new(name).file_stem().is_some_and(|stem| stem.to_string_lossy() == needle)
    }) {
        return Some(hit.clone());
    }
    // Title, exact then case-insensitive.
    let titled: Vec<(String, String)> = entries
        .iter()
        .filter(|name| !dir.join(name).is_dir())
        .filter_map(|name| {
            let path = dir.join(name);
            let raw = std::fs::read_to_string(&path).ok()?;
            let rel = path.strip_prefix(&notebook.root).unwrap_or(&path);
            Some((name.clone(), Note::parse(&raw, &path, &notebook.name, rel).title))
        })
        .collect();

    titled
        .iter()
        .find(|(_, title)| title == needle)
        .or_else(|| titled.iter().find(|(_, title)| title.eq_ignore_ascii_case(needle)))
        .map(|(name, _)| name.clone())
}

/// Render a selector back the way the user wrote it, for error messages.
pub fn describe(selector: &Selector) -> String {
    let mut out = String::new();
    if let Some(notebook) = &selector.notebook {
        out.push_str(notebook);
        out.push(':');
    }
    for folder in &selector.folder {
        out.push_str(folder);
        out.push('/');
    }
    match &selector.target {
        Some(Target::Id(id)) => out.push_str(&id.to_string()),
        Some(Target::Name(name)) => out.push_str(name),
        None => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(input: &str) -> Selector {
        Selector::parse(input)
    }

    #[test]
    fn parses_a_bare_id() {
        let s = sel("3");
        assert_eq!(s.notebook, None);
        assert!(s.folder.is_empty());
        assert_eq!(s.target, Some(Target::Id(3)));
    }

    #[test]
    fn parses_a_notebook_scope() {
        let s = sel("home:3");
        assert_eq!(s.notebook.as_deref(), Some("home"));
        assert_eq!(s.target, Some(Target::Id(3)));
    }

    #[test]
    fn parses_a_folder_path() {
        let s = sel("home:knowledge/3");
        assert_eq!(s.notebook.as_deref(), Some("home"));
        assert_eq!(s.folder, vec!["knowledge"]);
        assert_eq!(s.target, Some(Target::Id(3)));
    }

    #[test]
    fn parses_nested_folders() {
        let s = sel("work:a/b/c/12");
        assert_eq!(s.folder, vec!["a", "b", "c"]);
        assert_eq!(s.target, Some(Target::Id(12)));
    }

    #[test]
    fn a_trailing_slash_means_folder_only() {
        let s = sel("work:knowledge/");
        assert_eq!(s.notebook.as_deref(), Some("work"));
        assert_eq!(s.folder, vec!["knowledge"]);
        assert_eq!(s.target, None);
    }

    #[test]
    fn a_notebook_alone_has_no_target() {
        let s = sel("home:");
        assert!(s.is_notebook_only());
        assert_eq!(s.target, None);
    }

    #[test]
    fn a_filename_is_a_name_not_an_id() {
        assert_eq!(sel("note.md").target, Some(Target::Name("note.md".into())));
        // A timestamp filename must not be mistaken for an id.
        assert_eq!(
            sel("20260108203523.md").target,
            Some(Target::Name("20260108203523.md".into()))
        );
    }

    #[test]
    fn a_title_with_spaces_survives_parsing() {
        assert_eq!(sel("My Note Title").target, Some(Target::Name("My Note Title".into())));
    }

    #[test]
    fn a_url_is_not_a_notebook_scope() {
        let s = sel("https://example.com/page");
        assert_eq!(s.notebook, None);
        assert_eq!(s.target, Some(Target::Name("page".into())));
    }

    #[test]
    fn an_empty_selector_targets_nothing() {
        let s = sel("");
        assert_eq!(s, Selector::default());
        assert!(!s.is_notebook_only());
    }

    #[test]
    fn describe_round_trips() {
        for input in ["home:3", "home:knowledge/3", "work:a/b/12", "note.md"] {
            assert_eq!(describe(&sel(input)), input);
        }
    }
}
