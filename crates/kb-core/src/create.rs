//! Creating new notes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use jiff::Zoned;

use crate::frontmatter::{yaml_scalar, yaml_tags};
use crate::note::{Note, filename_stem, format_timestamp, timestamp_stem};
use crate::workspace::Notebook;

/// What to create.
#[derive(Debug, Clone, Default)]
pub struct NewNote {
    /// Title of the note. Without one, the filename is a timestamp and no
    /// heading is written — matching `nb add <content>`.
    pub title: Option<String>,
    /// Directory within the notebook, e.g. `knowledge`.
    pub dir: String,
    pub tags: Vec<String>,
    /// Body text. `None` writes just the heading, if there is a title.
    pub body: Option<String>,
    /// Explicit filename, overriding the one derived from the title.
    pub filename: Option<String>,
    /// File extension for the new note.
    pub extension: Option<String>,
}

impl NewNote {
    pub fn new(title: impl Into<String>, dir: impl Into<String>) -> Self {
        Self { title: Some(title.into()), dir: dir.into(), ..Default::default() }
    }

    /// A note with content but no title, as `nb add "some content"` creates.
    pub fn untitled(dir: impl Into<String>) -> Self {
        Self { dir: dir.into(), ..Default::default() }
    }

    /// Tags to write: the ones given, or the directory name as a default so new
    /// notes match what [`crate::migrate`] gave the existing ones.
    fn effective_tags(&self) -> Vec<String> {
        if !self.tags.is_empty() {
            return self.tags.clone();
        }
        match self.dir.trim_matches('/') {
            "" => Vec::new(),
            dir => vec![dir.split('/').next().unwrap_or(dir).to_string()],
        }
    }
}

/// Write a new note and return its path.
///
/// The filename comes from the title; a collision gets a numeric suffix rather
/// than overwriting anything.
pub fn create(notebook: &Notebook, spec: &NewNote, now: &Zoned) -> Result<PathBuf> {
    let dir = notebook.root.join(spec.dir.trim_matches('/'));
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;

    let path = available_path(&dir, &stem(spec, now), spec.extension.as_deref());
    let stamp = format_timestamp(now);

    // A supplied body is written as-is; it may already open with its own
    // heading, and second-guessing that would mangle piped-in content. Without
    // one, a titled note gets its heading and an untitled note stays empty.
    let body = match (spec.body.as_deref().map(str::trim), &spec.title) {
        (Some(text), _) if !text.is_empty() => format!("{text}\n"),
        (_, Some(title)) => format!("# {title}\n"),
        (_, None) => String::new(),
    };

    let title = match &spec.title {
        Some(title) => title.clone(),
        // Untitled notes still get a frontmatter title so listings have
        // something to show; derive it the same way reading a note would.
        None => derived_title(&body, &path),
    };

    let contents = format!(
        "---\ntitle: {}\ntags: {}\ncreated: {stamp}\nupdated: {stamp}\n---\n\n{body}",
        yaml_scalar(&title),
        yaml_tags(&spec.effective_tags()),
    );
    std::fs::write(&path, contents)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// The filename stem: an explicit filename, else the title, else a timestamp.
fn stem(spec: &NewNote, now: &Zoned) -> String {
    if let Some(filename) = &spec.filename {
        return Path::new(filename)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| filename.clone());
    }
    match spec.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(title) => filename_stem(title),
        None => timestamp_stem(now),
    }
}

fn derived_title(body: &str, path: &Path) -> String {
    Note::parse(body, path, "", path).title
}

/// The first free `<stem>.<ext>`, `<stem>-2.<ext>`, … in `dir`.
fn available_path(dir: &Path, stem: &str, extension: Option<&str>) -> PathBuf {
    let ext = extension.map(|e| e.trim_start_matches('.')).unwrap_or("md");
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2u32..)
        .map(|n| dir.join(format!("{stem}-{n}.{ext}")))
        .find(|candidate| !candidate.exists())
        .expect("an unused filename exists")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::Note;

    fn notebook(name: &str) -> Notebook {
        let root = std::env::temp_dir().join(format!("kb-create-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Notebook { name: "home".into(), root }
    }

    #[test]
    fn writes_a_note_that_parses_back() {
        let nb = notebook("roundtrip");
        let spec = NewNote::new("Cloudflare で RAG を構築する", "knowledge");
        let now = Zoned::now();
        let path = create(&nb, &spec, &now).unwrap();

        assert_eq!(path, nb.root.join("knowledge/cloudflare_で_rag_を構築する.md"));
        let note = nb.read(&path).unwrap();
        assert_eq!(note.title, "Cloudflare で RAG を構築する");
        assert_eq!(note.tags, vec!["knowledge"]);
        assert!(note.created.is_some());
        assert!(note.has_frontmatter);
        std::fs::remove_dir_all(&nb.root).unwrap();
    }

    #[test]
    fn never_overwrites_an_existing_note() {
        let nb = notebook("collision");
        let spec = NewNote::new("Same Title", "knowledge");
        let now = Zoned::now();
        let first = create(&nb, &spec, &now).unwrap();
        let second = create(&nb, &spec, &now).unwrap();
        assert_ne!(first, second);
        assert!(second.to_string_lossy().ends_with("same_title-2.md"));
        assert!(first.exists());
        std::fs::remove_dir_all(&nb.root).unwrap();
    }

    #[test]
    fn writes_a_supplied_body_verbatim() {
        let nb = notebook("body");
        let spec = NewNote {
            body: Some("## 背景\n\n本文がここにある。\n".into()),
            ..NewNote::new("記録", "knowledge")
        };
        let path = create(&nb, &spec, &Zoned::now()).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();

        assert!(raw.ends_with("## 背景\n\n本文がここにある。\n"));
        // The frontmatter title stands on its own; no heading is injected above
        // a body that already has one.
        assert!(!raw.contains("# 記録\n"));
        assert_eq!(nb.read(&path).unwrap().title, "記録");
        std::fs::remove_dir_all(&nb.root).unwrap();
    }

    #[test]
    fn an_empty_body_falls_back_to_a_heading() {
        let nb = notebook("emptybody");
        let spec = NewNote { body: Some("   \n".into()), ..NewNote::new("見出しだけ", "knowledge") };
        let path = create(&nb, &spec, &Zoned::now()).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().ends_with("# 見出しだけ\n"));
        std::fs::remove_dir_all(&nb.root).unwrap();
    }

    /// `nb add "content"` — no title, so the filename is the timestamp and no
    /// heading is invented.
    #[test]
    fn an_untitled_note_is_named_for_the_time() {
        let nb = notebook("untitled");
        let now = crate::note::parse_timestamp("2026-07-29T12:57:42+09:00").unwrap();
        let spec = NewNote { body: Some("content only".into()), ..NewNote::untitled("knowledge") };
        let path = create(&nb, &spec, &now).unwrap();

        assert_eq!(path, nb.root.join("knowledge/20260729125742.md"));
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.ends_with("content only\n"));
        assert!(!raw.contains("# "));
        // The listing still needs something to show, taken from the body.
        assert_eq!(nb.read(&path).unwrap().title, "content only");
        std::fs::remove_dir_all(&nb.root).unwrap();
    }

    #[test]
    fn an_explicit_filename_wins_over_the_title() {
        let nb = notebook("explicit");
        let spec =
            NewNote { filename: Some("chosen.md".into()), ..NewNote::new("Ignored", "knowledge") };
        let path = create(&nb, &spec, &Zoned::now()).unwrap();
        assert_eq!(path, nb.root.join("knowledge/chosen.md"));
        assert_eq!(nb.read(&path).unwrap().title, "Ignored");
        std::fs::remove_dir_all(&nb.root).unwrap();
    }

    #[test]
    fn a_type_sets_the_extension() {
        let nb = notebook("ext");
        let spec = NewNote { extension: Some("org".into()), ..NewNote::new("Notes", "knowledge") };
        let path = create(&nb, &spec, &Zoned::now()).unwrap();
        assert_eq!(path, nb.root.join("knowledge/notes.org"));
        std::fs::remove_dir_all(&nb.root).unwrap();
    }

    #[test]
    fn explicit_tags_win_over_the_directory() {
        let nb = notebook("tags");
        let spec = NewNote { tags: vec!["nix".into()], ..NewNote::new("T", "knowledge") };
        let path = create(&nb, &spec, &Zoned::now()).unwrap();
        assert_eq!(nb.read(&path).unwrap().tags, vec!["nix"]);
        std::fs::remove_dir_all(&nb.root).unwrap();
    }

    #[test]
    fn a_note_at_the_root_gets_no_tags() {
        let nb = notebook("root");
        let path = create(&nb, &NewNote::new("T", ""), &Zoned::now()).unwrap();
        assert_eq!(path, nb.root.join("t.md"));
        assert!(nb.read(&path).unwrap().tags.is_empty());
        std::fs::remove_dir_all(&nb.root).unwrap();
    }

    #[test]
    fn quotes_a_title_yaml_would_misread() {
        let nb = notebook("quoting");
        let path = create(&nb, &NewNote::new("nb: 遅い理由", "knowledge"), &Zoned::now()).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("title: \"nb: 遅い理由\""));
        assert_eq!(Note::parse(&raw, &path, "home", Path::new("x.md")).title, "nb: 遅い理由");
        std::fs::remove_dir_all(&nb.root).unwrap();
    }
}
