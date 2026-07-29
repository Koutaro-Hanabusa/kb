//! Rendering notes to HTML for `browse`.
//!
//! Two things happen before Markdown conversion: `[[selector]]` wiki links and
//! `#tag` mentions become internal links. Both are rewritten in the source text
//! rather than in the HTML, so the Markdown parser sees ordinary links and
//! handles escaping and code spans for us.

use pulldown_cmark::{Options, Parser, html};

/// Where internal links point.
#[derive(Debug, Clone)]
pub struct LinkBase {
    /// Host and port, e.g. `//localhost:6789`.
    pub prefix: String,
    /// Notebook that unqualified selectors belong to.
    pub notebook: String,
}

impl LinkBase {
    pub fn new(prefix: impl Into<String>, notebook: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            notebook: notebook.into(),
        }
    }

    fn item_url(&self, selector: &str) -> String {
        let scoped = if selector.contains(':') {
            selector.to_string()
        } else {
            format!("{}:{selector}", self.notebook)
        };
        format!("{}/{}", self.prefix, url_encode(&scoped))
    }

    fn tag_url(&self, tag: &str) -> String {
        format!(
            "{}/{}:?--query={}",
            self.prefix,
            self.notebook,
            url_encode(&format!("#{tag}"))
        )
    }
}

/// Convert note source to an HTML fragment.
pub fn to_html(markdown: &str, base: &LinkBase) -> String {
    let linked = linkify(markdown, base);
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;

    let mut out = String::new();
    html::push_html(&mut out, Parser::new_ext(&linked, options));
    out
}

/// Rewrite `[[selector]]` and `#tag` as Markdown links.
///
/// Fenced and inline code are left alone — a `#comment` in a shell snippet is
/// not a tag, and turning it into a link would corrupt the sample.
pub fn linkify(markdown: &str, base: &LinkBase) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut fenced = false;

    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if fenced {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        out.push_str(&linkify_line(line, base));
        out.push('\n');
    }

    if !markdown.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

fn linkify_line(line: &str, base: &LinkBase) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    let mut in_code = false;

    while !rest.is_empty() {
        // Inline code spans pass through untouched.
        if let Some(index) = rest.find('`') {
            let (before, after) = rest.split_at(index);
            if !in_code {
                out.push_str(&linkify_text(before, base));
            } else {
                out.push_str(before);
            }
            out.push('`');
            rest = &after[1..];
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.push_str(rest);
        } else {
            out.push_str(&linkify_text(rest, base));
        }
        break;
    }
    out
}

fn linkify_text(text: &str, base: &LinkBase) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < bytes.len() {
        // [[selector]]
        if bytes[i] == '['
            && bytes.get(i + 1) == Some(&'[')
            && let Some(end) = find_close(&bytes, i + 2)
        {
            let selector: String = bytes[i + 2..end].iter().collect();
            let trimmed = selector.trim();
            if !trimmed.is_empty() {
                // The visible text keeps its brackets, as `nb` renders it —
                // which means escaping them so they are not read as link
                // syntax themselves.
                out.push_str(&format!(r"[\[\[{trimmed}\]\]]({})", base.item_url(trimmed)));
                i = end + 2;
                continue;
            }
        }
        // #tag, but not a Markdown heading and not part of a word.
        if bytes[i] == '#' && i + 1 < bytes.len() && is_tag_start(bytes[i + 1]) {
            let starts_line = i == 0 || bytes[..i].iter().all(|c| c.is_whitespace());
            let after_space = i > 0 && bytes[i - 1].is_whitespace();
            if !starts_line && (after_space || i == 0) {
                let end = bytes[i + 1..]
                    .iter()
                    .position(|c| !is_tag_char(*c))
                    .map(|offset| i + 1 + offset)
                    .unwrap_or(bytes.len());
                let tag: String = bytes[i + 1..end].iter().collect();
                out.push_str(&format!("[#{tag}]({})", base.tag_url(&tag)));
                i = end;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn find_close(chars: &[char], from: usize) -> Option<usize> {
    (from..chars.len().saturating_sub(1)).find(|&i| chars[i] == ']' && chars[i + 1] == ']')
}

fn is_tag_start(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Percent-encode everything outside the unreserved set.
pub fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' | b'/' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Decode a percent-encoded string, treating `+` as a space.
pub fn url_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&value[i + 1..i + 3], 16) {
                Ok(byte) => {
                    out.push(byte);
                    i += 3;
                }
                Err(_) => {
                    out.push(bytes[i]);
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Escape text for inclusion in HTML.
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> LinkBase {
        LinkBase::new("//localhost:6789", "home")
    }

    #[test]
    fn wiki_links_become_internal_links() {
        let html = to_html("See [[1]] here.", &base());
        assert!(html.contains(r#"href="//localhost:6789/home:1""#), "{html}");
        // `nb` keeps the brackets in the link text.
        assert!(html.contains("[[1]]</a>"), "{html}");
    }

    #[test]
    fn a_scoped_wiki_link_keeps_its_notebook() {
        let html = to_html("[[work:knowledge/3]]", &base());
        assert!(
            html.contains("href=\"//localhost:6789/work:knowledge/3\""),
            "{html}"
        );
    }

    #[test]
    fn tags_become_search_links() {
        let html = to_html("See #mytag here.", &base());
        assert!(html.contains("--query=%23mytag"), "{html}");
        assert!(html.contains(">#mytag</a>"), "{html}");
    }

    #[test]
    fn a_heading_is_not_a_tag() {
        let html = to_html("# Heading\n\nbody", &base());
        assert!(html.contains("<h1>Heading</h1>"), "{html}");
        assert!(!html.contains("--query"), "{html}");
    }

    #[test]
    fn code_is_left_alone() {
        let markdown = "```sh\n# not a heading\ngrep #tag [[1]]\n```";
        let linked = linkify(markdown, &base());
        assert_eq!(linked.trim_end(), markdown);
    }

    #[test]
    fn inline_code_is_left_alone() {
        let linked = linkify("use `#tag` and `[[1]]` literally", &base());
        assert_eq!(linked.trim_end(), "use `#tag` and `[[1]]` literally");
    }

    #[test]
    fn a_hash_inside_a_word_is_not_a_tag() {
        let linked = linkify("issue-#12 and C#", &base());
        assert!(!linked.contains("--query"), "{linked}");
    }

    #[test]
    fn an_unclosed_wiki_link_is_left_as_text() {
        let linked = linkify("[[unclosed", &base());
        assert_eq!(linked.trim_end(), "[[unclosed");
    }

    #[test]
    fn urls_round_trip() {
        assert_eq!(url_encode("home:1"), "home:1");
        assert_eq!(url_encode("#tag"), "%23tag");
        assert_eq!(url_encode("a b"), "a%20b");
        assert_eq!(url_decode("%23tag"), "#tag");
        assert_eq!(url_decode("a+b"), "a b");
        assert_eq!(url_decode(&url_encode("日本語 #tag")), "日本語 #tag");
        // A stray percent must not panic or eat characters.
        assert_eq!(url_decode("100%"), "100%");
    }

    #[test]
    fn escapes_html() {
        assert_eq!(
            escape("<a href=\"x\">&</a>"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;"
        );
    }

    #[test]
    fn renders_tables_and_task_lists() {
        let html = to_html("| a | b |\n| - | - |\n| 1 | 2 |", &base());
        assert!(html.contains("<table>"), "{html}");
        let html = to_html("- [x] done\n- [ ] open", &base());
        assert!(html.contains("type=\"checkbox\""), "{html}");
    }
}
