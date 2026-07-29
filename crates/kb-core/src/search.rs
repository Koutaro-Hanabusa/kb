//! Full-text search across notebooks.
//!
//! There is no index. Every search reads the tree and scans it with ripgrep's
//! matcher, which measures in tens of milliseconds at this scale — cheaper than
//! the bookkeeping an index would demand.

use anyhow::{Context, Result};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::SearcherBuilder;
use grep_searcher::sinks::UTF8;
use jiff::Zoned;
use serde::Serialize;

use crate::frontmatter::Document;
use crate::note::Note;
use crate::workspace::Workspace;

/// How many matching lines to keep per note before truncating.
pub const DEFAULT_MAX_MATCHES: usize = 3;

/// A search request.
#[derive(Debug, Clone, Default)]
pub struct Query {
    pub pattern: String,
    /// Treat the pattern as a literal rather than a regular expression.
    pub fixed_string: bool,
    /// Force case sensitivity. `None` means smart case: case-insensitive unless
    /// the pattern contains an uppercase letter.
    pub case_sensitive: Option<bool>,
    pub tags: Vec<String>,
    pub notebook: Option<String>,
    pub since: Option<Zoned>,
    pub limit: Option<usize>,
    pub max_matches_per_note: Option<usize>,
}

impl Query {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self { pattern: pattern.into(), ..Default::default() }
    }
}

/// One matching line within a note.
#[derive(Debug, Clone, Serialize)]
pub struct MatchLine {
    pub line: u64,
    pub text: String,
}

/// A note that matched, together with its matching lines.
#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub note: Note,
    pub matches: Vec<MatchLine>,
    /// Total matching lines, before truncation to `max_matches_per_note`.
    pub match_count: usize,
    /// Whether the title itself matched — these rank first.
    pub title_match: bool,
}

/// Run `query` against the workspace.
///
/// Results are ranked with title matches first, then most recently updated.
pub fn search(workspace: &Workspace, query: &Query) -> Result<Vec<Hit>> {
    let matcher = build_matcher(query)?;
    let mut searcher = SearcherBuilder::new().line_number(true).build();
    let max_matches = query.max_matches_per_note.unwrap_or(DEFAULT_MAX_MATCHES);

    let mut hits = Vec::new();
    for notebook in workspace.select(query.notebook.as_deref())? {
        for path in notebook.note_paths() {
            // Read once and reuse the text for both metadata and matching;
            // walking the tree twice would double the syscalls for no gain.
            let Ok(raw) = std::fs::read_to_string(&path) else { continue };
            let note = Note::parse(&raw, &path, &notebook.name, &notebook.relative(&path));
            if !passes_filters(&note, query) {
                continue;
            }

            // Search the body only: a `tags:` line should not turn every note in
            // a category into a hit for that category's name.
            let doc = Document::split(&raw);
            let line_offset = raw[..doc.body.as_ptr() as usize - raw.as_ptr() as usize]
                .bytes()
                .filter(|b| *b == b'\n')
                .count() as u64;

            let mut matches = Vec::new();
            let mut match_count = 0usize;
            searcher
                .search_slice(
                    &matcher,
                    doc.body.as_bytes(),
                    UTF8(|line, text| {
                        match_count += 1;
                        if matches.len() < max_matches {
                            matches.push(MatchLine {
                                line: line + line_offset,
                                text: text.trim_end().to_string(),
                            });
                        }
                        Ok(true)
                    }),
                )
                .with_context(|| format!("searching {}", path.display()))?;

            let title_match = matcher_hits(&matcher, note.title.as_bytes());
            if match_count > 0 || title_match {
                hits.push(Hit { note, matches, match_count, title_match });
            }
        }
    }

    hits.sort_by(|a, b| {
        b.title_match
            .cmp(&a.title_match)
            .then_with(|| b.note.sort_key().cmp(&a.note.sort_key()))
            .then_with(|| a.note.rel_path.cmp(&b.note.rel_path))
    });
    if let Some(limit) = query.limit {
        hits.truncate(limit);
    }
    Ok(hits)
}

/// Notes matching the metadata filters, ignoring the pattern entirely.
///
/// This backs `kb ls`, where the filters are the whole query.
pub fn filter_notes(workspace: &Workspace, query: &Query) -> Result<Vec<Note>> {
    let mut notes: Vec<Note> = workspace
        .notes(query.notebook.as_deref())?
        .into_iter()
        .filter(|note| passes_filters(note, query))
        .collect();

    notes.sort_by(|a, b| {
        b.sort_key().cmp(&a.sort_key()).then_with(|| a.rel_path.cmp(&b.rel_path))
    });
    if let Some(limit) = query.limit {
        notes.truncate(limit);
    }
    Ok(notes)
}

fn passes_filters(note: &Note, query: &Query) -> bool {
    if !query.tags.is_empty() && !query.tags.iter().all(|tag| note.has_tag(tag)) {
        return false;
    }
    if let Some(since) = &query.since {
        match note.sort_key() {
            Some(ts) if ts >= since => {}
            _ => return false,
        }
    }
    true
}

fn build_matcher(query: &Query) -> Result<grep_regex::RegexMatcher> {
    let mut builder = RegexMatcherBuilder::new();
    match query.case_sensitive {
        Some(true) => {}
        Some(false) => {
            builder.case_insensitive(true);
        }
        None => {
            builder.case_smart(true);
        }
    }
    builder.fixed_strings(query.fixed_string);
    builder
        .build(&query.pattern)
        .with_context(|| format!("invalid search pattern `{}`", query.pattern))
}

fn matcher_hits(matcher: &grep_regex::RegexMatcher, haystack: &[u8]) -> bool {
    use grep_matcher::Matcher;
    matcher.find(haystack).ok().flatten().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("kb-search-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, contents) in files {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, contents).unwrap();
        }
        root
    }

    #[test]
    fn finds_matches_in_the_body() {
        let root = fixture("body", &[
            ("home/a.md", "# Alpha\n\nnix flakes are useful\n"),
            ("home/b.md", "# Beta\n\nnothing here\n"),
        ]);
        let ws = Workspace::open(&root).unwrap();
        let hits = search(&ws, &Query::new("nix")).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].note.title, "Alpha");
        assert_eq!(hits[0].matches[0].text, "nix flakes are useful");
        assert_eq!(hits[0].matches[0].line, 3);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn line_numbers_account_for_frontmatter() {
        let root = fixture("lines", &[(
            "home/a.md",
            "---\ntitle: Alpha\ntags: [x]\n---\n# Alpha\n\ntarget line\n",
        )]);
        let ws = Workspace::open(&root).unwrap();
        let hits = search(&ws, &Query::new("target")).unwrap();
        assert_eq!(hits[0].matches[0].line, 7);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn frontmatter_is_not_searched() {
        let root = fixture("fm", &[(
            "home/a.md",
            "---\ntitle: Alpha\ntags: [knowledge]\n---\n\nbody text\n",
        )]);
        let ws = Workspace::open(&root).unwrap();
        // `knowledge` appears only as a tag, so it must not register as a body hit.
        assert!(search(&ws, &Query::new("knowledge")).unwrap().is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_title_match_counts_and_ranks_first() {
        let root = fixture("title", &[
            ("home/a.md", "# Nix flakes\n\nunrelated body\n"),
            ("home/b.md", "# Other\n\nnix appears in the body\n"),
        ]);
        let ws = Workspace::open(&root).unwrap();
        let hits = search(&ws, &Query::new("nix")).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].note.title, "Nix flakes");
        assert!(hits[0].title_match);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn smart_case_is_the_default() {
        let root = fixture("case", &[("home/a.md", "# T\n\nNixOS is here\n")]);
        let ws = Workspace::open(&root).unwrap();
        assert_eq!(search(&ws, &Query::new("nixos")).unwrap().len(), 1);
        assert!(search(&ws, &Query::new("NIXOS")).unwrap().is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn truncates_matches_but_reports_the_full_count() {
        let root = fixture("trunc", &[("home/a.md", "# T\n\nx\nx\nx\nx\nx\n")]);
        let ws = Workspace::open(&root).unwrap();
        let hits = search(&ws, &Query { max_matches_per_note: Some(2), ..Query::new("x") }).unwrap();
        assert_eq!(hits[0].matches.len(), 2);
        assert_eq!(hits[0].match_count, 5);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn filters_by_tag() {
        let root = fixture("tag", &[
            ("home/a.md", "---\ntags: [nix]\n---\n# A\n\nterm\n"),
            ("home/b.md", "---\ntags: [other]\n---\n# B\n\nterm\n"),
        ]);
        let ws = Workspace::open(&root).unwrap();
        let query = Query { tags: vec!["nix".into()], ..Query::new("term") };
        let hits = search(&ws, &query).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].note.title, "A");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_fixed_string_pattern_skips_regex_syntax() {
        let root = fixture("fixed", &[("home/a.md", "# T\n\nliteral a.c here\n")]);
        let ws = Workspace::open(&root).unwrap();
        let query = Query { fixed_string: true, ..Query::new("a.c") };
        assert_eq!(search(&ws, &query).unwrap().len(), 1);
        let query = Query { fixed_string: true, ..Query::new("abc") };
        assert!(search(&ws, &query).unwrap().is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn listing_sorts_newest_first() {
        let root = fixture("ls", &[
            ("home/old.md", "---\nupdated: 2026-01-01\n---\n# Old\n"),
            ("home/new.md", "---\nupdated: 2026-07-01\n---\n# New\n"),
        ]);
        let ws = Workspace::open(&root).unwrap();
        let notes = filter_notes(&ws, &Query::default()).unwrap();
        assert_eq!(notes[0].title, "New");
        assert_eq!(notes[1].title, "Old");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
