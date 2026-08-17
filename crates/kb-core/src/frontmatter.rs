//! YAML frontmatter parsing and rendering.
//!
//! Parsing is deliberately forgiving: a note whose frontmatter fails to parse is
//! treated as having none at all, rather than as an error. Notes are handwritten
//! and a malformed header should never make a note invisible to search.

use std::collections::BTreeSet;
use std::ops::Range;

use serde_yaml_ng::Value;

/// The recognised frontmatter fields of a note.
///
/// `keys` records every top-level key present in the source, including ones this
/// struct does not model. [`crate::migrate`] uses it to add only what is missing
/// and leave everything else untouched.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Frontmatter {
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub keys: BTreeSet<String>,
}

impl Frontmatter {
    pub fn has(&self, key: &str) -> bool {
        self.keys.contains(key)
    }
}

/// A Markdown note split into its frontmatter and body.
#[derive(Debug, Clone)]
pub struct Document<'a> {
    /// Parsed frontmatter, if the note opened with a well-formed YAML block.
    pub frontmatter: Option<Frontmatter>,
    /// Byte range of the whole block, delimiters included, ending just past the
    /// newline that follows the closing `---`.
    pub span: Option<Range<usize>>,
    /// Everything after the frontmatter block.
    pub body: &'a str,
}

impl<'a> Document<'a> {
    /// Split `raw` into frontmatter and body.
    pub fn split(raw: &'a str) -> Self {
        let text = raw.strip_prefix('\u{feff}').unwrap_or(raw);
        let offset = raw.len() - text.len();

        let Some((yaml, span)) = locate_block(text) else {
            return Self {
                frontmatter: None,
                span: None,
                body: raw,
            };
        };
        // A body that opens with a horizontal rule or a setext heading can look
        // like a frontmatter block. Only YAML that actually parses as a mapping
        // counts, which rules those out.
        let Some(frontmatter) = parse(yaml) else {
            return Self {
                frontmatter: None,
                span: None,
                body: raw,
            };
        };

        Self {
            frontmatter: Some(frontmatter),
            span: Some(offset + span.start..offset + span.end),
            body: &raw[offset + span.end..],
        }
    }
}

/// Find the frontmatter block, returning its YAML payload and the byte range of
/// the block as a whole.
fn locate_block(text: &str) -> Option<(&str, Range<usize>)> {
    let after_open = strip_delimiter_line(text, "---")?;
    let mut cursor = after_open;

    loop {
        let line_end = text[cursor..]
            .find('\n')
            .map(|i| cursor + i + 1)
            .unwrap_or(text.len());
        let line = text[cursor..line_end].trim_end_matches(['\n', '\r']);
        if line.trim_end() == "---" || line.trim_end() == "..." {
            return Some((&text[after_open..cursor], 0..line_end));
        }
        if line_end >= text.len() {
            return None; // unterminated
        }
        cursor = line_end;
    }
}

/// If `text` opens with exactly `delim` on its own line, return the byte offset
/// just past that line.
fn strip_delimiter_line(text: &str, delim: &str) -> Option<usize> {
    let rest = text.strip_prefix(delim)?;
    let rest = rest.strip_prefix('\r').unwrap_or(rest);
    let rest = rest.strip_prefix('\n')?;
    Some(text.len() - rest.len())
}

fn parse(yaml: &str) -> Option<Frontmatter> {
    let value: Value = serde_yaml_ng::from_str(yaml).ok()?;
    let mapping = value.as_mapping()?;

    let mut fm = Frontmatter::default();
    for key in mapping.keys() {
        if let Some(key) = key.as_str() {
            fm.keys.insert(key.to_string());
        }
    }
    fm.title = mapping.get("title").and_then(scalar_to_string);
    fm.created = mapping.get("created").and_then(scalar_to_string);
    fm.updated = mapping.get("updated").and_then(scalar_to_string);
    fm.tags = mapping.get("tags").map(value_to_tags).unwrap_or_default();
    Some(fm)
}

fn scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Accept both `tags: [a, b]` and a bare `tags: a`.
fn value_to_tags(value: &Value) -> Vec<String> {
    match value {
        Value::Sequence(items) => items.iter().filter_map(scalar_to_string).collect(),
        other => scalar_to_string(other).into_iter().collect(),
    }
}

/// Set the `updated` field to `stamp`, leaving everything else byte-identical.
///
/// A note without frontmatter, or without an `updated` key, is returned
/// unchanged — writing a header into a file that never had one would be a
/// bigger change than an edit asked for.
pub fn touch_updated(raw: &str, stamp: &str) -> String {
    let doc = Document::split(raw);
    let (Some(span), Some(fm)) = (&doc.span, &doc.frontmatter) else {
        return raw.to_string();
    };
    if !fm.has("updated") {
        return raw.to_string();
    }

    let block = &raw[span.start..span.end];
    let mut out = String::with_capacity(raw.len());
    out.push_str(&raw[..span.start]);
    for line in block.lines() {
        if line.trim_start().starts_with("updated:") {
            let indent = &line[..line.len() - line.trim_start().len()];
            out.push_str(&format!("{indent}updated: {stamp}\n"));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str(&raw[span.end..]);
    out
}

/// Render a scalar as YAML, quoting only when the plain form would be ambiguous.
///
/// Most titles here are Japanese prose that needs no quoting, so unconditional
/// quoting would add noise to 787 files for the sake of a handful.
pub fn yaml_scalar(value: &str) -> String {
    if needs_quoting(value) {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

fn needs_quoting(value: &str) -> bool {
    if value.is_empty() || value.trim() != value {
        return true;
    }
    if value.starts_with([
        '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@',
        '`',
    ]) {
        return true;
    }
    if value.contains(": ") || value.contains(" #") || value.ends_with(':') {
        return true;
    }
    // Anything YAML would read as a non-string scalar has to be quoted to stay a
    // string.
    matches!(
        value.to_ascii_lowercase().as_str(),
        "true" | "false" | "yes" | "no" | "on" | "off" | "null" | "~"
    ) || value.parse::<f64>().is_ok()
}

/// Render a `tags:` value as an inline YAML sequence.
pub fn yaml_tags(tags: &[String]) -> String {
    let items: Vec<String> = tags.iter().map(|t| yaml_scalar(t)).collect();
    format!("[{}]", items.join(", "))
}

/// Render the block every newly written note opens with, closing delimiter
/// included.
///
/// `kind` is the Open Knowledge Format `type` — the one key that format
/// requires, and the only thing that differs between a note, a bookmark and a
/// todo. Notes, bookmarks and todos each used to carry their own copy of this
/// format string, which is how the first `type` landed on only one of them.
pub fn render_block(kind: &str, title: &str, tags: &[String], stamp: &str) -> String {
    format!(
        "---\ntype: {kind}\ntitle: {}\ntags: {}\ncreated: {stamp}\nupdated: {stamp}\n---\n",
        yaml_scalar(title),
        yaml_tags(tags),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_well_formed_block() {
        let raw = "---\ntitle: Hello\ntags: [a, b]\n---\n# Body\n";
        let doc = Document::split(raw);
        let fm = doc.frontmatter.expect("frontmatter");
        assert_eq!(fm.title.as_deref(), Some("Hello"));
        assert_eq!(fm.tags, vec!["a", "b"]);
        assert_eq!(doc.body, "# Body\n");
        assert_eq!(doc.span, Some(0..raw.find("# Body").unwrap()));
    }

    #[test]
    fn reads_block_sequence_tags() {
        let raw = "---\ntags:\n  - one\n  - two\n---\nbody";
        let fm = Document::split(raw).frontmatter.expect("frontmatter");
        assert_eq!(fm.tags, vec!["one", "two"]);
    }

    #[test]
    fn reads_a_bare_scalar_tag() {
        let raw = "---\ntags: solo\n---\nbody";
        let fm = Document::split(raw).frontmatter.expect("frontmatter");
        assert_eq!(fm.tags, vec!["solo"]);
    }

    #[test]
    fn records_every_top_level_key() {
        let raw = "---\ntitle: T\nstatus: draft\ndescription: d\n---\nbody";
        let fm = Document::split(raw).frontmatter.expect("frontmatter");
        assert!(fm.has("status"));
        assert!(fm.has("description"));
        assert!(!fm.has("tags"));
    }

    #[test]
    fn a_note_without_frontmatter_is_all_body() {
        let raw = "# Just a heading\n\ntext\n";
        let doc = Document::split(raw);
        assert!(doc.frontmatter.is_none());
        assert_eq!(doc.body, raw);
    }

    #[test]
    fn a_leading_horizontal_rule_is_not_frontmatter() {
        let raw = "---\n\nthis is prose, not a mapping\n---\n";
        let doc = Document::split(raw);
        assert!(doc.frontmatter.is_none());
        assert_eq!(doc.body, raw);
    }

    #[test]
    fn an_unterminated_block_is_not_frontmatter() {
        let raw = "---\ntitle: T\n\nbody without a closing delimiter\n";
        let doc = Document::split(raw);
        assert!(doc.frontmatter.is_none());
        assert_eq!(doc.body, raw);
    }

    #[test]
    fn handles_crlf_and_a_bom() {
        let raw = "\u{feff}---\r\ntitle: T\r\n---\r\nbody";
        let doc = Document::split(raw);
        let fm = doc.frontmatter.expect("frontmatter");
        assert_eq!(fm.title.as_deref(), Some("T"));
        assert_eq!(doc.body, "body");
    }

    #[test]
    fn quotes_only_what_yaml_would_misread() {
        assert_eq!(
            yaml_scalar("Cloudflare で RAG を構築する"),
            "Cloudflare で RAG を構築する"
        );
        assert_eq!(yaml_scalar("nb: 遅い"), "\"nb: 遅い\"");
        assert_eq!(yaml_scalar("- leading dash"), "\"- leading dash\"");
        assert_eq!(yaml_scalar("true"), "\"true\"");
        assert_eq!(yaml_scalar("20260108"), "\"20260108\"");
        assert_eq!(yaml_scalar(""), "\"\"");
        // A quote inside the value is fine unquoted — YAML only treats one as a
        // delimiter when the scalar opens with it.
        assert_eq!(yaml_scalar("say \"hi\""), "say \"hi\"");
        assert_eq!(yaml_scalar("\"quoted\""), "\"\\\"quoted\\\"\"");
    }

    #[test]
    fn touching_updates_only_that_line() {
        let raw = "---\ntitle: T\ntags: [a]\ncreated: 2026-01-01T00:00:00+09:00\nupdated: 2026-01-01T00:00:00+09:00\n---\n\n# Body\n";
        let touched = touch_updated(raw, "2026-07-29T13:30:00+09:00");

        assert!(touched.contains("updated: 2026-07-29T13:30:00+09:00"));
        assert!(touched.contains("created: 2026-01-01T00:00:00+09:00"));
        assert!(touched.ends_with("\n# Body\n"));
        assert_eq!(touched.lines().count(), raw.lines().count());
    }

    #[test]
    fn touching_a_note_without_frontmatter_changes_nothing() {
        let raw = "# Just a heading\n\nbody\n";
        assert_eq!(touch_updated(raw, "2026-07-29T13:30:00+09:00"), raw);
    }

    #[test]
    fn touching_without_an_updated_key_changes_nothing() {
        let raw = "---\ntitle: T\n---\n\nbody\n";
        assert_eq!(touch_updated(raw, "2026-07-29T13:30:00+09:00"), raw);
    }

    /// Every writer of a new note goes through `render_block`, so this is the
    /// one place that has to keep the OKF-required `type` key.
    #[test]
    fn a_rendered_block_carries_its_type_and_parses_back() {
        let raw = format!(
            "{}\nbody\n",
            render_block(
                "Bookmark",
                "T",
                &["bookmark".into()],
                "2026-08-17T16:00:00+09:00"
            )
        );
        assert!(raw.starts_with("---\ntype: Bookmark\n"), "{raw}");

        let doc = Document::split(&raw);
        let fm = doc.frontmatter.expect("frontmatter");
        assert!(fm.has("type"));
        assert_eq!(fm.title.as_deref(), Some("T"));
        assert_eq!(fm.tags, vec!["bookmark"]);
        assert_eq!(doc.body, "\nbody\n");
    }

    #[test]
    fn renders_tags_inline() {
        assert_eq!(yaml_tags(&["knowledge".into()]), "[knowledge]");
        assert_eq!(yaml_tags(&["a".into(), "b".into()]), "[a, b]");
        assert_eq!(yaml_tags(&[]), "[]");
    }
}
