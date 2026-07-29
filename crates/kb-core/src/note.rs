//! The `Note` type and the rules for deriving its metadata.

use std::path::{Path, PathBuf};

use jiff::civil;
use jiff::tz::TimeZone;
use jiff::Zoned;
use serde::Serialize;

use crate::frontmatter::Document;

/// A single Markdown note.
#[derive(Debug, Clone, Serialize)]
pub struct Note {
    pub path: PathBuf,
    pub notebook: String,
    /// Path relative to the notebook root — this is the note's identity, and
    /// doubles as its object key once synced to R2.
    pub rel_path: PathBuf,
    pub title: String,
    pub tags: Vec<String>,
    pub created: Option<Zoned>,
    pub updated: Option<Zoned>,
    /// Whether the source file already carried a frontmatter block.
    pub has_frontmatter: bool,
}

impl Note {
    /// Build a note from its source text.
    ///
    /// Metadata falls back through frontmatter, then the document itself, then
    /// the filename, so a note with no frontmatter at all still gets a usable
    /// title.
    pub fn parse(raw: &str, path: &Path, notebook: &str, rel_path: &Path) -> Self {
        let doc = Document::split(raw);
        let fm = doc.frontmatter.as_ref();

        let title = fm
            .and_then(|f| f.title.clone())
            .filter(|t| !t.trim().is_empty())
            .or_else(|| derive_title(doc.body))
            .unwrap_or_else(|| stem(path));

        let tags = fm.map(|f| f.tags.clone()).unwrap_or_default();
        let created = fm.and_then(|f| f.created.as_deref()).and_then(parse_timestamp);
        let updated = fm.and_then(|f| f.updated.as_deref()).and_then(parse_timestamp);

        Self {
            path: path.to_path_buf(),
            notebook: notebook.to_string(),
            rel_path: rel_path.to_path_buf(),
            title,
            tags,
            created,
            updated,
            has_frontmatter: fm.is_some(),
        }
    }

    /// The timestamp used for sorting and `--since` filtering.
    pub fn sort_key(&self) -> Option<&Zoned> {
        self.updated.as_ref().or(self.created.as_ref())
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t.eq_ignore_ascii_case(tag))
    }
}

/// Longest title kept verbatim before it gets condensed.
const MAX_DERIVED_TITLE: usize = 80;

/// Derive a title from the first meaningful line of `body`.
///
/// Notes here open in every style — `#`, `##`, or plain prose — so keying on
/// level-1 headings alone would leave most of them titled after a bare
/// timestamp. Taking the first real line covers all three and stays predictable.
fn derive_title(body: &str) -> Option<String> {
    let mut fenced = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced || trimmed.is_empty() || is_rule(trimmed) {
            continue;
        }
        let text = strip_markup(trimmed);
        if text.is_empty() {
            continue;
        }
        return Some(condense(&text));
    }
    None
}

/// Shorten an overlong opening line into something title-sized.
///
/// Notes captured programmatically tend to arrive as one unbroken paragraph
/// under a bare timestamp filename, so a trimmed first sentence is the only
/// useful title they will ever have.
fn condense(text: &str) -> String {
    if text.chars().count() <= MAX_DERIVED_TITLE {
        return text.to_string();
    }
    if let Some(end) = sentence_break(text) {
        return text[..end].trim_end().to_string();
    }
    let kept: String = text.chars().take(MAX_DERIVED_TITLE).collect();
    format!("{}…", kept.trim_end())
}

/// Byte index of the first sentence-ending punctuation within the length limit.
///
/// ASCII punctuation only counts when followed by a space — otherwise every
/// hostname and filename in the text would look like the end of a sentence.
fn sentence_break(text: &str) -> Option<usize> {
    for (count, (idx, ch)) in text.char_indices().enumerate() {
        if count >= MAX_DERIVED_TITLE {
            return None;
        }
        if matches!(ch, '。' | '！' | '？' | '：' | '；') {
            return (idx > 0).then_some(idx);
        }
        if matches!(ch, ':' | '.' | '!' | '?')
            && text[idx + ch.len_utf8()..].starts_with(' ')
            && idx > 0
        {
            return Some(idx);
        }
    }
    None
}

/// A horizontal rule carries no text worth titling a note with.
fn is_rule(line: &str) -> bool {
    let bare = line.replace([' ', '\t'], "");
    bare.len() >= 3
        && (bare.chars().all(|c| c == '-')
            || bare.chars().all(|c| c == '*')
            || bare.chars().all(|c| c == '_'))
}

/// Remove the leading markup of a heading, list item, or block quote.
fn strip_markup(line: &str) -> String {
    let mut text = line;
    if let Some(rest) = text.strip_prefix('#') {
        text = rest.trim_start_matches('#').trim_start();
    } else if let Some(rest) = text.strip_prefix("> ") {
        text = rest.trim_start();
    } else if let Some(rest) = text.strip_prefix("- ").or_else(|| text.strip_prefix("* ")) {
        text = rest.trim_start();
    }
    text.trim().trim_end_matches('#').trim_end().to_string()
}

fn stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Parse an ISO 8601 timestamp, tolerating a date-only or timezone-less value.
pub fn parse_timestamp(text: &str) -> Option<Zoned> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(zoned) = text.parse::<Zoned>() {
        return Some(zoned);
    }
    if let Ok(dt) = text.parse::<civil::DateTime>() {
        return dt.to_zoned(TimeZone::system()).ok();
    }
    if let Ok(date) = text.parse::<civil::Date>() {
        return date.to_zoned(TimeZone::system()).ok();
    }
    None
}

/// Render a timestamp the way frontmatter stores it.
pub fn format_timestamp(ts: &Zoned) -> String {
    ts.strftime("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

/// Recover a creation time from an `nb`-style filename.
///
/// `nb` named notes after their creation time, so two thirds of the existing
/// notes carry their date in the filename and nowhere else. Both the full
/// `20260108203523` form and a `20260720-some-title` date prefix are recognised.
pub fn timestamp_from_filename(path: &Path) -> Option<Zoned> {
    let stem = path.file_stem()?.to_str()?;
    let digits: String = stem.chars().take_while(|c| c.is_ascii_digit()).collect();
    let rest = &stem[digits.len()..];

    let (date, time) = match digits.len() {
        14 => (&digits[..8], Some(&digits[8..14])),
        // A date prefix only counts when the name breaks after it; a bare run of
        // eight digits inside a longer number is not a date.
        8 if rest.is_empty() || rest.starts_with(['-', '_']) => (&digits[..8], None),
        _ => return None,
    };

    let year: i16 = date[0..4].parse().ok()?;
    let month: i8 = date[4..6].parse().ok()?;
    let day: i8 = date[6..8].parse().ok()?;
    let (hour, minute, second) = match time {
        Some(t) => (t[0..2].parse().ok()?, t[2..4].parse().ok()?, t[4..6].parse().ok()?),
        None => (0, 0, 0),
    };

    let dt = civil::DateTime::new(year, month, day, hour, minute, second, 0).ok()?;
    dt.to_zoned(TimeZone::system()).ok()
}

/// Turn a title into a filename stem.
///
/// Non-ASCII text is kept as-is — plenty of existing notes are titled in
/// Japanese, and transliterating would make them harder to recognise, not
/// easier.
pub fn slugify(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut pending_dash = false;
    for ch in title.chars() {
        let mapped = match ch {
            c if c.is_ascii_alphanumeric() => Some(c.to_ascii_lowercase()),
            '-' | '_' => Some('-'),
            c if c.is_whitespace() || matches!(c, '/' | '\\' | ':' | '.' | ',') => None,
            c if c.is_ascii_punctuation() => None,
            c => Some(c),
        };
        match mapped {
            Some(c) => {
                if pending_dash && !out.is_empty() {
                    out.push('-');
                }
                pending_dash = false;
                out.push(c);
            }
            None => pending_dash = !out.is_empty(),
        }
    }
    let slug = out.trim_matches('-').to_string();
    if slug.is_empty() { "untitled".to_string() } else { slug }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(raw: &str, name: &str) -> Note {
        Note::parse(raw, Path::new(name), "home", Path::new(name))
    }

    #[test]
    fn prefers_the_frontmatter_title() {
        let n = note("---\ntitle: From frontmatter\n---\n# From heading\n", "file.md");
        assert_eq!(n.title, "From frontmatter");
        assert!(n.has_frontmatter);
    }

    #[test]
    fn falls_back_to_the_first_heading() {
        let n = note("# From heading\n\ntext\n", "file.md");
        assert_eq!(n.title, "From heading");
        assert!(!n.has_frontmatter);
    }

    #[test]
    fn takes_a_heading_of_any_level() {
        assert_eq!(note("## Second level\n\ntext\n", "f.md").title, "Second level");
        assert_eq!(note("#### Fourth level\n", "f.md").title, "Fourth level");
    }

    #[test]
    fn takes_the_first_line_of_prose_when_there_is_no_heading() {
        let n = note("my first memo from nvim\n\nmore text\n", "20260108203523.md");
        assert_eq!(n.title, "my first memo from nvim");
    }

    #[test]
    fn strips_list_and_quote_markers() {
        assert_eq!(note("- a bullet point\n", "f.md").title, "a bullet point");
        assert_eq!(note("> a quotation\n", "f.md").title, "a quotation");
        assert_eq!(note("## closed atx ##\n", "f.md").title, "closed atx");
    }

    #[test]
    fn skips_horizontal_rules() {
        assert_eq!(note("---\n\n***\n\nreal content\n", "f.md").title, "real content");
    }

    #[test]
    fn condenses_a_long_opening_line_at_the_first_sentence() {
        let raw = "burio.com apps/web の SNS アイコンを自前 SVG コンポーネント化した。\
                   SocialLink 型を LucideIcon 固定から汎用化したことで依存を切れた。\
                   さらに長く続く本文がここにある。";
        let n = note(raw, "20260421224448.md");
        assert_eq!(n.title, "burio.com apps/web の SNS アイコンを自前 SVG コンポーネント化した");
    }

    #[test]
    fn breaks_on_an_ascii_colon_only_when_a_space_follows() {
        let raw = format!("cameraman.8122.jp Upload 納品の調査: 原因は {}", "詳細".repeat(60));
        let n = note(&raw, "20260608120623.md");
        // The dots inside the hostname must not read as sentence ends.
        assert_eq!(n.title, "cameraman.8122.jp Upload 納品の調査");
    }

    #[test]
    fn truncates_when_a_long_line_has_no_sentence_break() {
        let n = note(&"x".repeat(200), "20260108203523.md");
        assert_eq!(n.title.chars().count(), 81);
        assert!(n.title.ends_with('…'));
    }

    #[test]
    fn falls_back_to_the_filename_when_there_is_no_content() {
        assert_eq!(note("\n\n", "20260108203523.md").title, "20260108203523");
    }

    #[test]
    fn ignores_headings_inside_code_fences() {
        let n = note("```sh\n# not a title\n```\n\n# real title\n", "file.md");
        assert_eq!(n.title, "real title");
    }

    #[test]
    fn an_empty_frontmatter_title_falls_through() {
        let n = note("---\ntitle: \"\"\n---\n# real title\n", "file.md");
        assert_eq!(n.title, "real title");
    }

    #[test]
    fn reads_timestamps_from_nb_filenames() {
        let ts = timestamp_from_filename(Path::new("20260108203523.md")).expect("timestamp");
        assert_eq!(ts.date().to_string(), "2026-01-08");
        assert_eq!(ts.time().to_string(), "20:35:23");
    }

    #[test]
    fn reads_a_date_prefix() {
        let ts = timestamp_from_filename(Path::new("20260720-tanstack-review.md")).expect("date");
        assert_eq!(ts.date().to_string(), "2026-07-20");
        assert_eq!(ts.time().to_string(), "00:00:00");
    }

    #[test]
    fn rejects_names_that_only_look_like_dates() {
        assert!(timestamp_from_filename(Path::new("kebab-case-note.md")).is_none());
        assert!(timestamp_from_filename(Path::new("8122-partner-audit.md")).is_none());
        assert!(timestamp_from_filename(Path::new("202601082035231.md")).is_none());
        assert!(timestamp_from_filename(Path::new("20261332203523.md")).is_none());
    }

    #[test]
    fn parses_the_timestamp_forms_frontmatter_may_carry() {
        assert!(parse_timestamp("2026-04-20T13:34:09+09:00").is_some());
        assert!(parse_timestamp("2026-04-20T13:34:09").is_some());
        assert!(parse_timestamp("2026-04-20").is_some());
        assert!(parse_timestamp("not a date").is_none());
        assert!(parse_timestamp("").is_none());
    }

    #[test]
    fn slugifies_titles() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("nb: 遅い理由"), "nb-遅い理由");
        assert_eq!(slugify("Cloudflare で RAG を構築する"), "cloudflare-で-rag-を構築する");
        assert_eq!(slugify("  spaced  out  "), "spaced-out");
        assert_eq!(slugify("!!!"), "untitled");
    }
}
