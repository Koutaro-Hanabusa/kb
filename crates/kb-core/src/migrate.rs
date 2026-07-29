//! Backfilling frontmatter onto notes that predate it.
//!
//! The guiding constraint is that migration must never rewrite prose. Existing
//! frontmatter lines are left byte-for-byte intact and only missing keys are
//! added, so a diff over the whole knowledge base shows nothing but insertions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use jiff::Zoned;

use crate::frontmatter::{Document, yaml_scalar, yaml_tags};
use crate::git;
use crate::note::{Note, format_timestamp, timestamp_from_filename};
use crate::workspace::{Notebook, Workspace};

/// Frontmatter keys this migration manages, in the order they are written.
pub const MANAGED_KEYS: [&str; 4] = ["title", "tags", "created", "updated"];

/// A pending change to one note.
#[derive(Debug, Clone)]
pub struct Plan {
    pub path: PathBuf,
    pub notebook: String,
    pub rel_path: PathBuf,
    /// Keys being added, as rendered YAML lines.
    pub added: Vec<(String, String)>,
    /// Whether the note is gaining a frontmatter block it did not have.
    pub creates_block: bool,
    /// The full contents to write.
    pub contents: String,
}

/// Work out what every note in the selected notebooks needs.
///
/// Notes that already carry all four keys produce no plan.
pub fn plan(workspace: &Workspace, notebook: Option<&str>) -> Result<Vec<Plan>> {
    let mut plans = Vec::new();
    for nb in workspace.select(notebook)? {
        let history = if git::is_repository(&nb.root) {
            git::history(&nb.root).unwrap_or_default()
        } else {
            HashMap::new()
        };
        for path in nb.note_paths() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let rel = nb.relative(&path);
            if let Some(plan) = plan_note(&raw, &path, nb, &rel, history.get(&rel)) {
                plans.push(plan);
            }
        }
    }
    Ok(plans)
}

fn plan_note(
    raw: &str,
    path: &Path,
    notebook: &Notebook,
    rel_path: &Path,
    history: Option<&git::FileHistory>,
) -> Option<Plan> {
    let doc = Document::split(raw);
    let note = Note::parse(raw, path, &notebook.name, rel_path);
    let existing = doc.frontmatter.as_ref();
    let has = |key: &str| existing.is_some_and(|fm| fm.has(key));

    let mut added: Vec<(String, String)> = Vec::new();
    if !has("title") {
        added.push(("title".into(), yaml_scalar(&note.title)));
    }
    if !has("tags") {
        added.push(("tags".into(), yaml_tags(&derive_tags(rel_path))));
    }
    if !has("created")
        && let Some(created) = derive_created(path, history)
    {
        added.push(("created".into(), format_timestamp(&created)));
    }
    if !has("updated")
        && let Some(updated) = derive_updated(path, history)
    {
        added.push(("updated".into(), format_timestamp(&updated)));
    }
    if added.is_empty() {
        return None;
    }

    let lines: String = added
        .iter()
        .map(|(key, value)| format!("{key}: {value}\n"))
        .collect();
    let (contents, creates_block) = match &doc.span {
        Some(span) => (insert_into_block(raw, span.clone(), &lines), false),
        None => (
            format!("---\n{lines}---\n\n{}", raw.trim_start_matches('\n')),
            true,
        ),
    };

    Some(Plan {
        path: path.to_path_buf(),
        notebook: notebook.name.clone(),
        rel_path: rel_path.to_path_buf(),
        added,
        creates_block,
        contents,
    })
}

/// Splice `lines` in just above the block's closing delimiter.
fn insert_into_block(raw: &str, span: std::ops::Range<usize>, lines: &str) -> String {
    let block = &raw[span.start..span.end];
    let trimmed = block.trim_end_matches(['\n', '\r']);
    let closing = trimmed.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let at = span.start + closing;
    format!("{}{lines}{}", &raw[..at], &raw[at..])
}

/// The note's directory becomes its initial tag; notes at the notebook root get
/// none.
fn derive_tags(rel_path: &Path) -> Vec<String> {
    rel_path
        .parent()
        .and_then(|parent| parent.components().next())
        .map(|first| first.as_os_str().to_string_lossy().into_owned())
        .filter(|tag| !tag.is_empty())
        .into_iter()
        .collect()
}

/// Creation time, preferring the filename because `nb` encoded it there and git
/// only knows when the note was committed.
fn derive_created(path: &Path, history: Option<&git::FileHistory>) -> Option<Zoned> {
    timestamp_from_filename(path).or_else(|| history.map(|h| h.created.clone()))
}

fn derive_updated(path: &Path, history: Option<&git::FileHistory>) -> Option<Zoned> {
    history
        .map(|h| h.updated.clone())
        .or_else(|| timestamp_from_filename(path))
}

/// Write a planned change to disk.
///
/// The new contents land in a sibling temporary file and are renamed into place,
/// so an interrupted run cannot leave a half-written note behind.
pub fn apply(plan: &Plan) -> Result<()> {
    let dir = plan.path.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(format!(
        ".kb-migrate-{}.tmp",
        plan.path.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&tmp, &plan.contents).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &plan.path)
        .with_context(|| format!("replacing {}", plan.path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notebook() -> Notebook {
        Notebook {
            name: "home".into(),
            root: PathBuf::from("/nb/home"),
        }
    }

    fn plan_for(raw: &str, rel: &str) -> Option<Plan> {
        let rel_path = PathBuf::from(rel);
        let path = PathBuf::from("/nb/home").join(&rel_path);
        plan_note(raw, &path, &notebook(), &rel_path, None)
    }

    #[test]
    fn adds_a_block_to_a_bare_note() {
        let plan = plan_for("# Alpha\n\nbody\n", "knowledge/alpha.md").expect("plan");
        assert!(plan.creates_block);
        assert_eq!(
            plan.contents,
            "---\ntitle: Alpha\ntags: [knowledge]\n---\n\n# Alpha\n\nbody\n"
        );
    }

    #[test]
    fn keeps_the_body_untouched() {
        let body = "# Alpha\n\nprose with --- a rule\n\n```\ncode\n```\n";
        let plan = plan_for(body, "knowledge/alpha.md").expect("plan");
        assert!(plan.contents.ends_with(body));
    }

    #[test]
    fn fills_only_the_missing_keys() {
        let raw = "---\ntitle: Kept\nstatus: draft\n---\n# Heading\n";
        let plan = plan_for(raw, "knowledge/a.md").expect("plan");
        assert!(!plan.creates_block);
        let keys: Vec<&str> = plan.added.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["tags"]);
        assert_eq!(
            plan.contents,
            "---\ntitle: Kept\nstatus: draft\ntags: [knowledge]\n---\n# Heading\n"
        );
    }

    #[test]
    fn a_complete_note_needs_no_change() {
        let raw = "---\ntitle: T\ntags: [x]\ncreated: 2026-01-01\nupdated: 2026-01-02\n---\nbody";
        assert!(plan_for(raw, "knowledge/a.md").is_none());
    }

    #[test]
    fn takes_created_from_an_nb_filename() {
        let plan = plan_for("no heading", "knowledge/20260108203523.md").expect("plan");
        let created = plan
            .added
            .iter()
            .find(|(k, _)| k == "created")
            .expect("created");
        assert!(created.1.starts_with("2026-01-08T20:35:23"));
    }

    #[test]
    fn prefers_git_history_for_updated() {
        let rel_path = PathBuf::from("knowledge/20260108203523.md");
        let path = PathBuf::from("/nb/home").join(&rel_path);
        let history = git::FileHistory {
            created: crate::note::parse_timestamp("2026-02-01T00:00:00+09:00").unwrap(),
            updated: crate::note::parse_timestamp("2026-07-17T09:12:44+09:00").unwrap(),
        };
        let plan = plan_note("body", &path, &notebook(), &rel_path, Some(&history)).expect("plan");
        let get = |key: &str| {
            plan.added
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        // The filename records when the note was written; git only knows when it
        // was committed, which is later.
        assert!(get("created").starts_with("2026-01-08T20:35:23"));
        assert!(get("updated").starts_with("2026-07-17T09:12:44"));
    }

    #[test]
    fn a_note_at_the_root_gets_no_tags() {
        let plan = plan_for("# A\n", "loose.md").expect("plan");
        let tags = plan.added.iter().find(|(k, _)| k == "tags").expect("tags");
        assert_eq!(tags.1, "[]");
    }

    #[test]
    fn nested_notes_are_tagged_by_their_top_directory() {
        let plan = plan_for("# A\n", "knowledge/deep/nested/a.md").expect("plan");
        let tags = plan.added.iter().find(|(k, _)| k == "tags").expect("tags");
        assert_eq!(tags.1, "[knowledge]");
    }

    /// The whole migration hinges on this: whatever is written must read back as
    /// the same metadata, or 787 notes get a header that only looks right.
    #[test]
    fn generated_frontmatter_parses_back() {
        let body = "# nb: 遅い理由\n\nbody\n";
        let plan = plan_for(body, "knowledge/20260108203523.md").expect("plan");

        let note = Note::parse(
            &plan.contents,
            &PathBuf::from("/nb/home/knowledge/20260108203523.md"),
            "home",
            &PathBuf::from("knowledge/20260108203523.md"),
        );
        assert_eq!(note.title, "nb: 遅い理由");
        assert_eq!(note.tags, vec!["knowledge"]);
        assert_eq!(
            note.created
                .expect("created")
                .strftime("%Y-%m-%dT%H:%M:%S")
                .to_string(),
            "2026-01-08T20:35:23"
        );
        assert!(note.has_frontmatter);
        assert_eq!(
            Document::split(&plan.contents)
                .body
                .trim_start_matches('\n'),
            body
        );
    }

    /// Re-running the migration must be a no-op, not a second round of keys.
    #[test]
    fn migration_is_idempotent() {
        let first = plan_for("# Alpha\n\nbody\n", "knowledge/20260108203523.md").expect("plan");
        assert!(plan_for(&first.contents, "knowledge/20260108203523.md").is_none());
    }

    #[test]
    fn quotes_a_title_that_yaml_would_misread() {
        let plan = plan_for("# nb: 遅い理由\n", "knowledge/a.md").expect("plan");
        let title = plan
            .added
            .iter()
            .find(|(k, _)| k == "title")
            .expect("title");
        assert_eq!(title.1, "\"nb: 遅い理由\"");
    }
}
