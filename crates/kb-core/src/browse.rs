//! The `browse` web application.
//!
//! A small synchronous HTTP server that renders notes, resolves `[[wiki]]`
//! links and `#tags` between them, and offers add / edit / delete views. It is
//! deliberately one thread and no framework: it serves one person on localhost.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use tiny_http::{Header, Response, Server};

use crate::index::Index;
use crate::note::Note;
use crate::render::{self, LinkBase};
use crate::selector::{self, Resolved};
use crate::workspace::Workspace;
use crate::{Query, Selector, search};

/// The port `nb browse` listens on.
pub const DEFAULT_PORT: u16 = 6789;

/// A parsed request: the selector from the path, plus query parameters.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub selector: String,
    pub query: Option<String>,
    pub edit: bool,
    pub add: bool,
    pub delete: bool,
    pub original: bool,
    pub notebooks: bool,
}

impl Request {
    /// Parse a request URL like `/home:2?--edit`.
    pub fn parse(url: &str) -> Self {
        let (path, query) = url.split_once('?').unwrap_or((url, ""));
        let path = path.trim_start_matches('/');

        let mut request = Request {
            selector: render::url_decode(path.trim_start_matches("--original/")),
            original: path.starts_with("--original/"),
            ..Default::default()
        };

        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            match key {
                "--query" | "-q" => request.query = Some(render::url_decode(value)),
                "--edit" => request.edit = true,
                "--add" => request.add = true,
                "--delete" => request.delete = true,
                "--notebooks" => request.notebooks = true,
                _ => {}
            }
        }
        request
    }
}

/// Serve the knowledge base until the process is stopped.
pub fn serve(workspace: &Workspace, port: u16) -> Result<()> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let server = Server::http(address)
        .map_err(|e| anyhow::anyhow!("starting the server on {address}: {e}"))?;

    for mut request in server.incoming_requests() {
        let url = request.url().to_string();
        let is_post = request.method().as_str() == "POST";

        let mut body = String::new();
        if is_post {
            let _ = request.as_reader().read_to_string(&mut body);
        }

        let (status, html) = match handle(workspace, &url, is_post.then_some(body.as_str())) {
            Ok(html) => (200, html),
            Err(error) => (404, page("Not found", &format!("<p class=\"error\">{}</p>", render::escape(&error.to_string())), workspace, "")),
        };

        let header = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
            .expect("valid header");
        let response = Response::from_string(html).with_status_code(status).with_header(header);
        let _ = request.respond(response);
    }
    Ok(())
}

/// Build the page for one request.
pub fn handle(workspace: &Workspace, url: &str, post_body: Option<&str>) -> Result<String> {
    let request = Request::parse(url);

    if let Some(body) = post_body {
        return handle_post(workspace, &request, body);
    }
    if request.notebooks || request.selector.is_empty() && request.query.is_none() {
        return Ok(render_index(workspace));
    }
    if let Some(query) = &request.query {
        return Ok(render_search(workspace, query));
    }

    let parsed = Selector::parse(&request.selector);
    let resolved = selector::resolve(workspace, &parsed)?;

    match resolved {
        Resolved::Note { path, .. } => {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            if request.original {
                return Ok(raw);
            }
            if request.edit {
                return Ok(render_edit(workspace, &request.selector, &raw));
            }
            if request.delete {
                return Ok(render_delete(workspace, &request.selector, &path));
            }
            Ok(render_note(workspace, &request.selector, &path, &raw))
        }
        Resolved::Folder { path, .. } => Ok(render_listing(workspace, &request.selector, &path)),
        Resolved::Notebook { name, root } => {
            if request.add {
                return Ok(render_add(workspace, &name));
            }
            Ok(render_listing(workspace, &request.selector, &root))
        }
    }
}

/// Apply an add, edit, or delete submitted from the browser.
fn handle_post(workspace: &Workspace, request: &Request, body: &str) -> Result<String> {
    let fields = parse_form(body);
    let get = |key: &str| fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());

    if request.delete {
        let resolved = selector::resolve(workspace, &Selector::parse(&request.selector))?;
        let path = resolved.path().to_path_buf();
        crate::items::delete(&path)?;
        return Ok(render_index(workspace));
    }

    if request.add {
        let notebook_name = Selector::parse(&request.selector).notebook;
        let notebook = match &notebook_name {
            Some(name) => workspace.notebook(name).context("unknown notebook")?,
            None => workspace.default_notebook()?,
        };
        let spec = crate::create::NewNote {
            title: get("title").filter(|t| !t.trim().is_empty()),
            dir: String::new(),
            tags: get("tags")
                .map(|tags| tags.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect())
                .unwrap_or_default(),
            body: get("content"),
            filename: None,
            extension: None,
        };
        let path = crate::create::create(notebook, &spec, &jiff::Zoned::now())?;
        let dir = path.parent().context("no parent")?;
        let mut index = Index::load(dir)?;
        index.add(&file_name(&path));
        index.save(dir)?;

        let raw = std::fs::read_to_string(&path)?;
        return Ok(render_note(workspace, &request.selector, &path, &raw));
    }

    // Edit: replace the note's contents wholesale.
    let resolved = selector::resolve(workspace, &Selector::parse(&request.selector))?;
    let path = resolved.path().to_path_buf();
    let content = get("content").unwrap_or_default();
    let stamp = crate::note::format_timestamp(&jiff::Zoned::now());
    let content = crate::frontmatter::touch_updated(&content, &stamp);
    std::fs::write(&path, &content).with_context(|| format!("writing {}", path.display()))?;
    Ok(render_note(workspace, &request.selector, &path, &content))
}

fn parse_form(body: &str) -> Vec<(String, String)> {
    body.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (render::url_decode(key), render::url_decode(value))
        })
        .collect()
}

// ─────────────────────────── views ───────────────────────────

fn render_note(workspace: &Workspace, selector: &str, path: &PathBuf, raw: &str) -> String {
    let notebook = notebook_of(workspace, selector);
    let base = LinkBase::new("", &notebook);
    let doc = crate::Document::split(raw);
    let body = render::to_html(doc.body, &base);

    let note = Note::parse(raw, path, &notebook, path);
    let title = render::escape(&note.title);
    page(&note.title, &format!("<article>{body}</article>"), workspace, &crumbs(selector, &title))
}

fn render_listing(workspace: &Workspace, selector: &str, dir: &PathBuf) -> String {
    let index = Index::load(dir).unwrap_or_default();
    let notebook = notebook_of(workspace, selector);

    let mut rows = String::new();
    for (id, name) in index.entries() {
        let path = dir.join(name);
        if !path.exists() {
            continue;
        }
        let is_dir = path.is_dir();
        let label = if is_dir {
            format!("{name}/")
        } else {
            std::fs::read_to_string(&path)
                .map(|raw| Note::parse(&raw, &path, &notebook, &path).title)
                .unwrap_or_else(|_| name.to_string())
        };
        let target = format!("{notebook}:{}", join_selector(selector, name));
        rows.push_str(&format!(
            "<li><a href=\"/{}\"><span class=\"id\">[{id}]</span> {}</a></li>",
            render::url_encode(&target),
            render::escape(&label)
        ));
    }

    let heading = if selector.is_empty() { notebook.clone() } else { selector.to_string() };
    let add = format!(
        "<p><a class=\"button\" href=\"/{}:?--add\">+ add</a></p>",
        render::url_encode(&notebook)
    );
    page(
        &heading,
        &format!("{add}<ul class=\"listing\">{rows}</ul>"),
        workspace,
        &crumbs(selector, &render::escape(&heading)),
    )
}

fn render_index(workspace: &Workspace) -> String {
    let mut rows = String::new();
    for notebook in &workspace.notebooks {
        let count = crate::items::count(&notebook.root).unwrap_or(0);
        rows.push_str(&format!(
            "<li><a href=\"/{}:\">{}</a> <span class=\"muted\">{count} items</span></li>",
            render::url_encode(&notebook.name),
            render::escape(&notebook.name)
        ));
    }
    page("notebooks", &format!("<ul class=\"listing\">{rows}</ul>"), workspace, "notebooks")
}

fn render_search(workspace: &Workspace, query: &str) -> String {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut rows = String::new();
    let mut push = |note: &Note, rows: &mut String| {
        if !seen.contains(&note.path) {
            seen.push(note.path.clone());
            rows.push_str(&result_row(note));
        }
    };

    // A `#tag` query matches both frontmatter tags and inline `#tag` mentions —
    // notes here carry them both ways, and finding only half would look broken.
    if let Some(tag) = query.strip_prefix('#') {
        let tagged = Query { tags: vec![tag.to_string()], ..Query::default() };
        for note in search::filter_notes(workspace, &tagged).unwrap_or_default() {
            push(&note, &mut rows);
        }
    }

    let text = Query { fixed_string: true, ..Query::new(query) };
    for hit in search::search(workspace, &text).unwrap_or_default() {
        push(&hit.note, &mut rows);
    }

    let heading = format!("search: {}", render::escape(query));
    page(&heading, &format!("<ul class=\"listing\">{rows}</ul>"), workspace, &heading)
}

fn result_row(note: &Note) -> String {
    let target = format!("{}:{}", note.notebook, note.rel_path.display());
    format!(
        "<li><a href=\"/{}\">{}</a> <span class=\"muted\">{}</span></li>",
        render::url_encode(&target),
        render::escape(&note.title),
        render::escape(&note.notebook)
    )
}

fn render_edit(workspace: &Workspace, selector: &str, raw: &str) -> String {
    let body = format!(
        "<form method=\"post\" action=\"/{}?--edit\">\
         <textarea name=\"content\" rows=\"24\">{}</textarea>\
         <p><button type=\"submit\">save</button> \
         <a class=\"button\" href=\"/{}\">cancel</a></p></form>",
        render::url_encode(selector),
        render::escape(raw),
        render::url_encode(selector)
    );
    page(&format!("edit {selector}"), &body, workspace, &format!("edit {}", render::escape(selector)))
}

fn render_add(workspace: &Workspace, notebook: &str) -> String {
    let body = format!(
        "<form method=\"post\" action=\"/{}:?--add\">\
         <p><input name=\"title\" placeholder=\"title\"></p>\
         <p><input name=\"tags\" placeholder=\"tags, comma, separated\"></p>\
         <textarea name=\"content\" rows=\"20\" placeholder=\"content\"></textarea>\
         <p><button type=\"submit\">add</button></p></form>",
        render::url_encode(notebook)
    );
    page("add", &body, workspace, "add")
}

fn render_delete(workspace: &Workspace, selector: &str, path: &PathBuf) -> String {
    let body = format!(
        "<p>Delete <code>{}</code>?</p>\
         <form method=\"post\" action=\"/{}?--delete\">\
         <button type=\"submit\">delete</button> \
         <a class=\"button\" href=\"/{}\">cancel</a></form>",
        render::escape(&path.display().to_string()),
        render::url_encode(selector),
        render::url_encode(selector)
    );
    page("delete", &body, workspace, &format!("delete {}", render::escape(selector)))
}

// ─────────────────────────── chrome ───────────────────────────

fn crumbs(selector: &str, title: &str) -> String {
    let (notebook, rest) = selector.split_once(':').unwrap_or(("", selector));
    let mut out = String::from("<a href=\"/\">❯ kb</a>");
    if !notebook.is_empty() {
        out.push_str(&format!(
            " <span class=\"muted\">·</span> <a href=\"/{}:\">{}</a>",
            render::url_encode(notebook),
            render::escape(notebook)
        ));
    }
    if !rest.is_empty() {
        out.push_str(&format!(
            " <span class=\"muted\">·</span> <span class=\"muted\">{}</span>",
            render::escape(rest)
        ));
        out.push_str(&format!(
            " <span class=\"muted\">·</span> <a href=\"/{}?--edit\">edit</a>",
            render::url_encode(selector)
        ));
        out.push_str(&format!(
            " <span class=\"muted\">|</span> <a href=\"/{}?--delete\">delete</a>",
            render::url_encode(selector)
        ));
    }
    let _ = title;
    out
}

/// Wrap a fragment in the full page, with the search box and styles.
fn page(title: &str, body: &str, workspace: &Workspace, crumbs: &str) -> String {
    let notebook = workspace
        .default_notebook()
        .map(|nb| nb.name.clone())
        .unwrap_or_else(|_| String::from("home"));

    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n\
         <meta charset=\"UTF-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n\
         <title>kb browse {}</title>\n<style>{STYLE}</style>\n</head>\n<body>\n\
         <nav class=\"crumbs\">{crumbs}</nav>\n\
         <form class=\"search\" method=\"get\" action=\"/{}:\">\
         <input name=\"--query\" placeholder=\"search\" value=\"\"></form>\n\
         <main>{body}</main>\n</body>\n</html>\n",
        render::escape(title),
        render::url_encode(&notebook),
    )
}

/// Dark, monospaced, and self-contained — the same shape `nb browse` presents.
const STYLE: &str = "\
html { background:#141418; color:#c5c4cc; font-size:16px; line-height:1.5; }
html, input, textarea, button { font-family: Menlo, Consolas, Monaco, monospace; }
body { margin:0; padding:1rem 1.25rem; word-wrap:break-word; }
a { color:#8fb8ff; text-decoration:none; }
a:hover { text-decoration:underline; }
.muted { color:#6b6a75; }
.error { color:#ff9494; }
.crumbs { padding-bottom:.5rem; border-bottom:1px solid #2a2a32; }
.search { margin:.75rem 0 1.25rem; }
.search input { width:100%; box-sizing:border-box; background:#1c1c22; color:inherit;
  border:1px solid #2a2a32; border-radius:3px; padding:.4rem .6rem; }
main { max-width:52rem; }
.listing { list-style:none; padding:0; }
.listing li { padding:.15rem 0; }
.id { color:#6b6a75; }
article h1, article h2, article h3 { line-height:1.25; }
article code { background:#1c1c22; padding:.1rem .3rem; border-radius:3px; }
article pre { background:#1c1c22; padding:.75rem; border-radius:4px; overflow-x:auto; }
article pre code { background:none; padding:0; }
article blockquote { margin:0; padding-left:.9rem; border-left:3px solid #2a2a32; color:#a3a2ad; }
article table { border-collapse:collapse; }
article th, article td { border:1px solid #2a2a32; padding:.3rem .6rem; }
textarea { width:100%; box-sizing:border-box; background:#1c1c22; color:inherit;
  border:1px solid #2a2a32; border-radius:3px; padding:.6rem; }
input { background:#1c1c22; color:inherit; border:1px solid #2a2a32;
  border-radius:3px; padding:.4rem .6rem; }
button, .button { display:inline-block; background:#242430; color:#c5c4cc;
  border:1px solid #2a2a32; border-radius:3px; padding:.35rem .8rem; cursor:pointer; }
button:hover, .button:hover { background:#2e2e3c; text-decoration:none; }
";

fn notebook_of(workspace: &Workspace, selector: &str) -> String {
    Selector::parse(selector)
        .notebook
        .or_else(|| workspace.default_notebook().ok().map(|nb| nb.name.clone()))
        .unwrap_or_else(|| String::from("home"))
}

fn join_selector(selector: &str, name: &str) -> String {
    let rest = selector.split_once(':').map(|(_, rest)| rest).unwrap_or(selector);
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() { name.to_string() } else { format!("{rest}/{name}") }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plain_selector() {
        let request = Request::parse("/home:2");
        assert_eq!(request.selector, "home:2");
        assert!(!request.edit);
    }

    #[test]
    fn parses_flags() {
        let request = Request::parse("/home:2?--columns=80&--edit");
        assert_eq!(request.selector, "home:2");
        assert!(request.edit);

        assert!(Request::parse("/home:?--add").add);
        assert!(Request::parse("/home:2?--delete").delete);
        assert!(Request::parse("/?--notebooks").notebooks);
    }

    #[test]
    fn parses_a_query() {
        let request = Request::parse("/home:?--query=%23mytag");
        assert_eq!(request.query.as_deref(), Some("#mytag"));
    }

    #[test]
    fn parses_the_original_prefix() {
        let request = Request::parse("/--original/home/linker.md");
        assert!(request.original);
        assert_eq!(request.selector, "home/linker.md");
    }

    #[test]
    fn decodes_a_percent_encoded_selector() {
        assert_eq!(Request::parse("/home:%E6%97%A5%E6%9C%AC").selector, "home:日本");
    }

    #[test]
    fn parses_form_bodies() {
        let fields = parse_form("title=Hello+World&content=a%3Db&tags=x%2Cy");
        assert_eq!(fields[0], ("title".into(), "Hello World".into()));
        assert_eq!(fields[1], ("content".into(), "a=b".into()));
        assert_eq!(fields[2], ("tags".into(), "x,y".into()));
    }

    #[test]
    fn joins_folder_selectors() {
        assert_eq!(join_selector("home:", "a.md"), "a.md");
        assert_eq!(join_selector("home:knowledge/", "a.md"), "knowledge/a.md");
        assert_eq!(join_selector("", "a.md"), "a.md");
    }
}
