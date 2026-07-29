//! Terminal formatting helpers.

use std::io::IsTerminal;

use kb_core::{Hit, Note};

/// ANSI styling, switched off when stdout is not a terminal.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    enabled: bool,
    theme: kb_core::theme::Theme,
}

impl Style {
    /// Decide styling from the terminal and the configured theme.
    pub fn detect() -> Self {
        // Honour the de facto standard for opting out of colour.
        let forced_off = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let settings = kb_core::settings::Settings::load().ok();
        let read = |key: &str| settings.as_ref().and_then(|s| s.get(key));

        Self {
            enabled: std::io::stdout().is_terminal() && !forced_off,
            theme: kb_core::theme::resolve(
                read("color_theme").as_deref(),
                read("color_primary").as_deref(),
                read("color_secondary").as_deref(),
            ),
        }
    }

    #[cfg(test)]
    pub fn plain() -> Self {
        Self {
            enabled: false,
            theme: kb_core::theme::DEFAULT,
        }
    }

    /// Styling with colour forced on, for tests that have no terminal.
    #[cfg(test)]
    pub fn themed(theme: kb_core::theme::Theme) -> Self {
        Self {
            enabled: true,
            theme,
        }
    }

    fn wrap(&self, code: &str, text: &str) -> String {
        if self.enabled {
            format!("\u{1b}[{code}m{text}\u{1b}[0m")
        } else {
            text.to_string()
        }
    }

    fn coloured(&self, colour: u8, text: &str) -> String {
        self.wrap(&format!("38;5;{colour}"), text)
    }

    /// The colour to scan for: paths and notebook names.
    pub fn path(&self, text: &str) -> String {
        self.coloured(self.theme.primary, text)
    }

    pub fn title(&self, text: &str) -> String {
        self.wrap("1", text)
    }

    pub fn dim(&self, text: &str) -> String {
        self.wrap("2", text)
    }

    /// Supporting detail: line numbers and other secondary marks.
    pub fn line_number(&self, text: &str) -> String {
        self.coloured(self.theme.secondary, text)
    }
}

/// `notebook/relative/path.md` — unique across the whole knowledge base.
pub fn qualified_path(note: &Note) -> String {
    format!("{}/{}", note.notebook, note.rel_path.display())
}

pub fn date(note: &Note) -> String {
    note.sort_key()
        .map(|ts| ts.strftime("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "─".repeat(10))
}

/// Render search results in the style of a grep tool: a header per file, then
/// its matching lines.
pub fn render_hits(hits: &[Hit], style: Style) -> String {
    let mut out = String::new();
    for (index, hit) in hits.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&style.path(&qualified_path(&hit.note)));
        if hit.note.title
            != hit
                .note
                .rel_path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
        {
            out.push_str(&style.dim(" — "));
            out.push_str(&style.title(&hit.note.title));
        }
        out.push('\n');

        for m in &hit.matches {
            out.push_str(&format!(
                "  {}: {}\n",
                style.line_number(&m.line.to_string()),
                truncate(&m.text, 200)
            ));
        }
        if hit.match_count > hit.matches.len() {
            out.push_str(&style.dim(&format!(
                "  … {} more match(es)\n",
                hit.match_count - hit.matches.len()
            )));
        }
    }
    out
}

/// Render a note listing as aligned columns: date, path, title.
pub fn render_notes(notes: &[Note], style: Style) -> String {
    const MAX_PATH_WIDTH: usize = 56;
    let width = notes
        .iter()
        .map(|n| display_width(&qualified_path(n)))
        .max()
        .unwrap_or(0)
        .min(MAX_PATH_WIDTH);

    let mut out = String::new();
    for note in notes {
        let path = qualified_path(note);
        let padding = width.saturating_sub(display_width(&path));
        out.push_str(&format!(
            "{}  {}{}  {}\n",
            style.dim(&date(note)),
            style.path(&path),
            " ".repeat(padding),
            style.title(&note.title),
        ));
    }
    out
}

/// Approximate display width, counting CJK characters as two columns.
///
/// Enough to keep columns aligned for a mix of Japanese and ASCII paths without
/// pulling in a full Unicode width table.
fn display_width(text: &str) -> usize {
    text.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum()
}

fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF
        | 0xFE30..=0xFE6F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x20000..=0x3FFFD)
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn note(title: &str, rel: &str) -> Note {
        Note {
            path: PathBuf::from("/nb/home").join(rel),
            notebook: "home".into(),
            rel_path: PathBuf::from(rel),
            title: title.into(),
            tags: vec![],
            created: None,
            updated: None,
            has_frontmatter: false,
        }
    }

    #[test]
    fn qualifies_paths_with_the_notebook() {
        assert_eq!(
            qualified_path(&note("T", "knowledge/a.md")),
            "home/knowledge/a.md"
        );
    }

    #[test]
    fn lists_without_ansi_when_styling_is_off() {
        let rendered = render_notes(&[note("Title", "knowledge/a.md")], Style::plain());
        assert!(!rendered.contains('\u{1b}'));
        assert!(rendered.contains("home/knowledge/a.md"));
        assert!(rendered.contains("Title"));
    }

    #[test]
    fn omits_a_title_that_merely_repeats_the_filename() {
        let hit = Hit {
            note: note("alpha", "knowledge/alpha.md"),
            matches: vec![],
            match_count: 0,
            title_match: true,
        };
        let rendered = render_hits(&[hit], Style::plain());
        assert_eq!(rendered.trim(), "home/knowledge/alpha.md");
    }

    #[test]
    fn reports_matches_beyond_the_shown_ones() {
        let hit = Hit {
            note: note("T", "a.md"),
            matches: vec![kb_core::MatchLine {
                line: 1,
                text: "x".into(),
            }],
            match_count: 4,
            title_match: false,
        };
        assert!(render_hits(&[hit], Style::plain()).contains("3 more match"));
    }

    /// The theme has to reach the actual output, not just `settings colors`.
    #[test]
    fn the_theme_decides_the_colours() {
        let ocean = kb_core::theme::Theme::by_name("ocean").unwrap();
        let style = Style::themed(ocean);

        assert_eq!(style.path("x"), "\u{1b}[38;5;75mx\u{1b}[0m");
        assert_eq!(style.line_number("1"), "\u{1b}[38;5;26m1\u{1b}[0m");

        let rendered = render_notes(&[note("Title", "knowledge/a.md")], style);
        assert!(rendered.contains("\u{1b}[38;5;75m"), "{rendered:?}");
    }

    #[test]
    fn styling_off_emits_no_escapes_whatever_the_theme() {
        let rendered = render_notes(&[note("Title", "knowledge/a.md")], Style::plain());
        assert!(!rendered.contains('\u{1b}'));
    }

    #[test]
    fn counts_cjk_as_double_width() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("日本語"), 6);
        assert_eq!(display_width("a日"), 3);
    }
}
