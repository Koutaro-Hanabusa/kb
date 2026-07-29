//! Format conversion and browser bookmark import.
//!
//! Conversion is delegated to `pandoc` when it is installed. Nothing here
//! requires it: callers check [`have_pandoc`] and fall back to copying the file
//! unchanged, so an import never fails just because pandoc is missing.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Whether `pandoc` is on `PATH`.
pub fn have_pandoc() -> bool {
    which("pandoc")
}

fn which(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join(program).is_file())
    })
}

/// Run pandoc over `source`, returning what it wrote to standard output.
pub fn pandoc(source: &Path, args: &[String]) -> Result<String> {
    if !have_pandoc() {
        bail!("pandoc is not installed");
    }
    let output = Command::new("pandoc")
        .arg(source)
        .args(args)
        .output()
        .context("running pandoc")?;

    if !output.status.success() {
        bail!("pandoc failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Convert a file to Markdown, writing the result to `destination`.
pub fn to_markdown(source: &Path, destination: &Path) -> Result<()> {
    let markdown = pandoc(source, &["--to".into(), "markdown".into()])?;
    std::fs::write(destination, markdown)
        .with_context(|| format!("writing {}", destination.display()))
}

/// Whether a path looks like something worth converting to Markdown.
pub fn is_convertible(path: &Path) -> bool {
    path.extension().is_some_and(|ext| {
        let ext = ext.to_string_lossy().to_ascii_lowercase();
        matches!(
            ext.as_str(),
            "html" | "htm" | "docx" | "odt" | "epub" | "rst" | "textile" | "org" | "tex"
        )
    })
}

/// A bookmark read out of a browser export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedBookmark {
    pub title: String,
    pub url: String,
}

/// Extract bookmarks from a Netscape-format export file.
///
/// Chrome, Firefox, and Edge all export this same shape: `<A HREF="…">title</A>`
/// inside a nested list. Only those two pieces matter, so this scans for the
/// anchors rather than parsing the whole document.
pub fn parse_bookmarks(html: &str) -> Vec<ImportedBookmark> {
    let mut found = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut cursor = 0;

    while let Some(offset) = lower[cursor..].find("<a ") {
        let start = cursor + offset;
        let Some(tag_end) = lower[start..].find('>').map(|i| start + i) else { break };
        let tag = &html[start..tag_end];
        cursor = tag_end + 1;

        let Some(url) = attribute(tag, "href") else { continue };
        if !url.contains("://") {
            continue; // skip javascript: and place: entries
        }
        let Some(close) = lower[cursor..].find("</a>").map(|i| cursor + i) else { continue };
        let title = strip_tags(&html[cursor..close]);

        found.push(ImportedBookmark {
            title: if title.is_empty() { url.clone() } else { title },
            url,
        });
        cursor = close + 4;
    }
    found
}

/// Read an attribute out of a tag, quoted or not.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let at = lower.find(&format!("{name}="))? + name.len() + 1;
    let rest = &tag[at..];

    let value = match rest.chars().next()? {
        quote @ ('"' | '\'') => rest[1..].split(quote).next()?,
        _ => rest.split_whitespace().next()?,
    };
    Some(decode_entities(value))
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for ch in html.chars() {
        match ch {
            '<' => inside = true,
            '>' => inside = false,
            c if !inside => out.push(c),
            _ => {}
        }
    }
    decode_entities(out.trim())
}

fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape Chrome, Firefox, and Edge all export.
    #[test]
    fn reads_a_netscape_bookmark_file() {
        let html = r#"
<!DOCTYPE NETSCAPE-Bookmark-file-1>
<DL><p>
    <DT><H3>Folder</H3>
    <DL><p>
        <DT><A HREF="https://example.com/a" ADD_DATE="1700000000">Example &amp; Co</A>
        <DT><A HREF="https://example.com/b">Second</A>
    </DL><p>
</DL><p>
"#;
        let bookmarks = parse_bookmarks(html);
        assert_eq!(bookmarks.len(), 2);
        assert_eq!(bookmarks[0].url, "https://example.com/a");
        assert_eq!(bookmarks[0].title, "Example & Co");
        assert_eq!(bookmarks[1].title, "Second");
    }

    #[test]
    fn skips_entries_that_are_not_links() {
        let html = r#"<A HREF="javascript:void(0)">bookmarklet</A>
                      <A HREF="place:type=6">smart folder</A>
                      <A HREF="https://example.com">real</A>"#;
        let bookmarks = parse_bookmarks(html);
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(bookmarks[0].url, "https://example.com");
    }

    #[test]
    fn falls_back_to_the_url_when_a_title_is_empty() {
        let bookmarks = parse_bookmarks(r#"<A HREF="https://example.com"></A>"#);
        assert_eq!(bookmarks[0].title, "https://example.com");
    }

    #[test]
    fn handles_single_quotes_and_nested_markup() {
        let bookmarks = parse_bookmarks("<A HREF='https://example.com'><B>Bold</B> title</A>");
        assert_eq!(bookmarks[0].url, "https://example.com");
        assert_eq!(bookmarks[0].title, "Bold title");
    }

    #[test]
    fn an_empty_document_yields_nothing() {
        assert!(parse_bookmarks("").is_empty());
        assert!(parse_bookmarks("<html><body>no links</body></html>").is_empty());
    }

    #[test]
    fn recognises_what_is_worth_converting() {
        assert!(is_convertible(Path::new("a.html")));
        assert!(is_convertible(Path::new("a.DOCX")));
        assert!(!is_convertible(Path::new("a.md")));
        assert!(!is_convertible(Path::new("a.png")));
        assert!(!is_convertible(Path::new("noext")));
    }

    #[test]
    fn converts_html_to_markdown_when_pandoc_is_available() {
        if !have_pandoc() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("kb-convert-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let source = dir.join("page.html");
        std::fs::write(&source, "<h1>Heading</h1><p>Body <em>text</em>.</p>").unwrap();
        let destination = dir.join("page.md");
        to_markdown(&source, &destination).unwrap();

        let markdown = std::fs::read_to_string(&destination).unwrap();
        assert!(markdown.contains("Heading"), "{markdown}");
        assert!(markdown.contains('#'), "{markdown}");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
