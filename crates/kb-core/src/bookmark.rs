//! Bookmarks: notes that record a URL, saved as `*.bookmark.md`.
//!
//! The rendered shape is `nb`'s, verified by running it: a heading of
//! `<title> (<domain>)`, the URL on its own line, then Quote, Comment, Related,
//! and Tags sections in that order, each present only when it has content.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use jiff::Zoned;

use crate::index::Index;
use crate::note::timestamp_stem;
use crate::workspace::Notebook;

/// Extension marking a note as a bookmark.
pub const BOOKMARK_EXT: &str = "bookmark.md";

/// What to record about a URL.
#[derive(Debug, Clone, Default)]
pub struct NewBookmark {
    pub url: String,
    pub title: Option<String>,
    pub comment: Option<String>,
    pub quote: Option<String>,
    pub tags: Vec<String>,
    pub related: Vec<String>,
    pub filename: Option<String>,
    /// Skip fetching the page.
    pub no_request: bool,
    /// Save the fetched HTML alongside the bookmark.
    pub save_source: bool,
}

/// Whether a path is a bookmark.
pub fn is_bookmark(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        name.to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(BOOKMARK_EXT)
    })
}

/// The heading text: `<title> (<domain>)`.
pub fn heading(spec: &NewBookmark, title: &str) -> String {
    match domain_of(&spec.url) {
        Some(domain) => format!("{title} ({domain})"),
        None => title.to_string(),
    }
}

/// Render the bookmark body.
pub fn render(spec: &NewBookmark, title: &str) -> String {
    let mut out = format!("# {}\n", heading(spec, title));
    out.push_str(&format!("\n<{}>\n", spec.url));

    if let Some(quote) = non_empty(&spec.quote) {
        out.push_str("\n## Quote\n\n");
        for line in quote.lines() {
            out.push_str(&format!("> {line}\n"));
        }
    }
    if let Some(comment) = non_empty(&spec.comment) {
        out.push_str(&format!("\n## Comment\n\n{comment}\n"));
    }
    if !spec.related.is_empty() {
        out.push_str("\n## Related\n\n");
        for related in &spec.related {
            // A bare selector like `home:3` is not a URL and stays unbracketed.
            if related.contains("://") {
                out.push_str(&format!("- <{related}>\n"));
            } else {
                out.push_str(&format!("- {related}\n"));
            }
        }
    }
    if !spec.tags.is_empty() {
        let tags: Vec<String> = spec
            .tags
            .iter()
            .map(|tag| format!("#{}", tag.trim_start_matches('#')))
            .collect();
        out.push_str(&format!("\n## Tags\n\n{}\n", tags.join(" ")));
    }
    out
}

/// The host part of a URL, as the heading shows it.
pub fn domain_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit_once('@').map(|(_, host)| host).unwrap_or(host);
    let host = host.split_once(':').map(|(host, _)| host).unwrap_or(host);
    (!host.is_empty()).then(|| host.trim_start_matches("www.").to_string())
}

/// Fetch a page and return its body.
pub fn fetch(url: &str) -> Result<String> {
    let mut response = ureq::get(url)
        .header("User-Agent", "kb")
        .call()
        .with_context(|| format!("requesting {url}"))?;
    response
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading {url}"))
}

/// Extract the contents of the HTML `<title>` element.
///
/// A full HTML parse is more machinery than one element warrants, and a
/// malformed page should degrade to "no title", not an error.
pub fn html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = lower.find("<title")?;
    let start = lower[open..].find('>')? + open + 1;
    let end = lower[start..].find("</title>")? + start;

    let title = decode_entities(html[start..end].trim());
    (!title.is_empty()).then_some(title)
}

/// Decode the handful of entities that show up in titles.
fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Write a bookmark, fetching the page for its title unless told not to.
pub fn create(
    notebook: &Notebook,
    dir: &str,
    spec: &NewBookmark,
    now: &Zoned,
) -> Result<(PathBuf, Option<PathBuf>)> {
    let directory = notebook.root.join(dir.trim_matches('/'));
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("creating {}", directory.display()))?;

    let source = (!spec.no_request).then(|| fetch(&spec.url)).transpose()?;
    let title = spec
        .title
        .clone()
        .or_else(|| source.as_deref().and_then(html_title))
        .unwrap_or_else(|| spec.url.clone());

    let stem = match &spec.filename {
        Some(name) => name
            .trim_end_matches(".md")
            .trim_end_matches(".bookmark")
            .to_string(),
        None => timestamp_stem(now),
    };
    let path = available_path(&directory, &stem);
    // The body is `nb`'s byte for byte; the frontmatter sits above it so
    // bookmarks list and filter alongside every other note.
    let stamp = crate::note::format_timestamp(now);
    let tags = if spec.tags.is_empty() {
        vec!["bookmark".to_string()]
    } else {
        spec.tags.clone()
    };
    let contents = format!(
        "{}\n{}",
        crate::frontmatter::render_block("Bookmark", &heading(spec, &title), &tags, &stamp),
        render(spec, &title),
    );
    std::fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;

    let mut index = Index::load(&directory)?;
    index.add(&file_name(&path));

    let source_path = match (&source, spec.save_source) {
        (Some(html), true) => {
            let path = directory.join(format!("{stem}.html"));
            std::fs::write(&path, html).with_context(|| format!("writing {}", path.display()))?;
            index.add(&file_name(&path));
            Some(path)
        }
        _ => None,
    };
    index.save(&directory)?;

    Ok((path, source_path))
}

/// The URL a bookmark points at.
pub fn url_of(raw: &str) -> Option<String> {
    raw.lines()
        .map(str::trim)
        .find_map(|line| {
            line.strip_prefix('<')?
                .strip_suffix('>')
                .map(str::to_string)
        })
        .filter(|url| url.contains("://"))
}

fn available_path(dir: &Path, stem: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{BOOKMARK_EXT}"));
    if !first.exists() {
        return first;
    }
    (2u32..)
        .map(|n| dir.join(format!("{stem}-{n}.{BOOKMARK_EXT}")))
        .find(|candidate| !candidate.exists())
        .expect("an unused filename exists")
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(url: &str) -> NewBookmark {
        NewBookmark {
            url: url.to_string(),
            ..Default::default()
        }
    }

    /// Byte-for-byte the output of
    /// `nb bookmark <url> --no-request --title Full -c COMMENT -q QUOTE -t a,b -r <url>`.
    #[test]
    fn renders_what_nb_renders() {
        let spec = NewBookmark {
            comment: Some("COMMENT".into()),
            quote: Some("QUOTE".into()),
            tags: vec!["a".into(), "b".into()],
            related: vec!["https://rel.example".into()],
            ..spec("https://example.com/x")
        };
        assert_eq!(
            render(&spec, "Full"),
            "# Full (example.com)\n\
             \n<https://example.com/x>\n\
             \n## Quote\n\n> QUOTE\n\
             \n## Comment\n\nCOMMENT\n\
             \n## Related\n\n- <https://rel.example>\n\
             \n## Tags\n\n#a #b\n"
        );
    }

    #[test]
    fn a_bare_bookmark_is_just_heading_and_url() {
        assert_eq!(
            render(&spec("https://example.com"), "Example Domain"),
            "# Example Domain (example.com)\n\n<https://example.com>\n"
        );
    }

    #[test]
    fn extracts_the_domain() {
        assert_eq!(
            domain_of("https://example.com/path").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            domain_of("http://www.example.com").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            domain_of("https://sub.example.com:8443/x").as_deref(),
            Some("sub.example.com")
        );
        assert_eq!(
            domain_of("https://user@example.com/x").as_deref(),
            Some("example.com")
        );
        assert_eq!(domain_of("not a url"), Some("not a url".to_string()));
    }

    #[test]
    fn reads_the_html_title() {
        assert_eq!(
            html_title("<html><head><title>Hi</title>").as_deref(),
            Some("Hi")
        );
        // Attributes on the element, and whitespace inside it.
        assert_eq!(
            html_title("<TITLE lang=\"en\">\n  Spaced  Out\n</TITLE>").as_deref(),
            Some("Spaced Out")
        );
        assert_eq!(
            html_title("<title>A &amp; B &#39;quoted&#39;</title>").as_deref(),
            Some("A & B 'quoted'")
        );
        assert_eq!(html_title("<title></title>"), None);
        assert_eq!(html_title("<html>no title here</html>"), None);
        assert_eq!(html_title("<title>unterminated"), None);
    }

    #[test]
    fn a_multiline_quote_is_quoted_per_line() {
        let spec = NewBookmark {
            quote: Some("one\ntwo".into()),
            ..spec("https://e.com")
        };
        assert!(render(&spec, "T").contains("> one\n> two\n"));
    }

    #[test]
    fn a_related_selector_is_not_bracketed_like_a_url() {
        let spec = NewBookmark {
            related: vec!["home:3".into()],
            ..spec("https://e.com")
        };
        assert!(render(&spec, "T").contains("- home:3\n"));
    }

    #[test]
    fn tags_are_not_double_hashed() {
        let spec = NewBookmark {
            tags: vec!["#a".into(), "b".into()],
            ..spec("https://e.com")
        };
        assert!(render(&spec, "T").ends_with("#a #b\n"));
    }

    #[test]
    fn recognises_bookmark_paths() {
        assert!(is_bookmark(Path::new("a/20260729131258.bookmark.md")));
        assert!(!is_bookmark(Path::new("a/note.md")));
        assert!(!is_bookmark(Path::new("a/bookmark.md.txt")));
    }

    #[test]
    fn reads_the_url_back_out() {
        let rendered = render(&spec("https://example.com/x"), "T");
        assert_eq!(url_of(&rendered).as_deref(), Some("https://example.com/x"));
        assert_eq!(url_of("# just a note\n\ntext"), None);
    }
}
