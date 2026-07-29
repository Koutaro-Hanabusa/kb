//! Command implementations.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use kb_core::selector::{self, Resolved};
use kb_core::{Index, Note, Notebook, Query, Selector, Workspace, git, items, migrate, search};

use crate::commands::*;
use crate::output::{self, Style};
use crate::shell;
use crate::since;

/// Everything a command needs to run.
pub struct Ctx<'a, W: Write> {
    pub workspace: &'a Workspace,
    pub style: Style,
    pub out: &'a mut W,
}

impl<W: Write> Ctx<'_, W> {
    fn notebook(&self, name: Option<&str>) -> Result<&Notebook> {
        match name {
            Some(name) => self
                .workspace
                .notebook(name)
                .with_context(|| format!("unknown notebook `{name}`")),
            None => self.workspace.default_notebook(),
        }
    }

    /// Resolve a selector to a single item.
    fn resolve(&self, input: &str) -> Result<Resolved> {
        selector::resolve(self.workspace, &Selector::parse(input))
    }

    /// Resolve a selector that must name a note.
    fn resolve_note(&self, input: &str) -> Result<PathBuf> {
        match self.resolve(input)? {
            Resolved::Note { path, .. } => Ok(path),
            other => bail!("not a note: {}", other.path().display()),
        }
    }

    fn notebook_of(&self, path: &Path) -> Option<&Notebook> {
        self.workspace.notebooks.iter().find(|nb| path.starts_with(&nb.root))
    }

    fn read_note(&self, path: &Path) -> Result<Note> {
        let notebook = self
            .notebook_of(path)
            .ok_or_else(|| anyhow!("{} is outside the knowledge base", path.display()))?;
        notebook.read(path)
    }
}

// ─────────────────────────── add ───────────────────────────

pub fn add<W: Write>(ctx: &mut Ctx<'_, W>, args: &AddArgs) -> Result<()> {
    // `nb add` treats a bare argument as a filename when it has an extension
    // and as content when it does not.
    let (positional_filename, positional_content) = match &args.target {
        Some(target) if has_extension(target) => {
            (Some(target.clone()), args.content_arg.clone())
        }
        Some(target) => (None, Some(target.clone())),
        None => (None, args.content_arg.clone()),
    };

    // A filename argument may carry a notebook and folder: `work:knowledge/a.md`.
    let scoped = positional_filename.as_deref().map(Selector::parse);
    let notebook_name = scoped
        .as_ref()
        .and_then(|s| s.notebook.clone())
        .or_else(|| args.folder.as_deref().and_then(|f| Selector::parse(f).notebook));
    let notebook = ctx.notebook(notebook_name.as_deref())?;

    let folder = match (&args.folder, &scoped) {
        (Some(folder), _) => Selector::parse(folder).folder_path(),
        (None, Some(scoped)) => scoped.folder_path(),
        (None, None) => PathBuf::new(),
    };

    let filename = args.filename.clone().or_else(|| {
        scoped.as_ref().and_then(|s| match &s.target {
            Some(kb_core::Target::Name(name)) => Some(name.clone()),
            _ => None,
        })
    });

    let body = match args.content.as_deref() {
        Some("-") => Some(read_stdin()?),
        Some(text) => Some(text.to_string()),
        None => match positional_content {
            Some(text) => Some(text),
            // Piped input with no arguments is content too.
            None if !std::io::stdin().is_terminal() => Some(read_stdin()?),
            None => None,
        },
    };

    let spec = kb_core::NewNote {
        title: args.title.clone(),
        dir: folder.to_string_lossy().into_owned(),
        tags: args.tags.clone(),
        body,
        filename,
        extension: args.r#type.clone(),
    };

    let has_content = spec.body.is_some();
    let path = kb_core::create::create(notebook, &spec, &jiff::Zoned::now())?;

    let mut index = Index::load(path.parent().unwrap())?;
    index.add(&file_name(&path));
    index.save(path.parent().unwrap())?;

    writeln!(ctx.out, "{}", path.display())?;

    // Content means the note is already written; an editor would only be in the
    // way of a script. `--edit` asks for it anyway.
    if args.edit || (!has_content && !args.no_edit) {
        ctx.out.flush()?;
        shell::launch(&shell::editor(None), &path)?;
    }
    Ok(())
}

fn has_extension(value: &str) -> bool {
    Path::new(value).extension().is_some()
}

fn read_stdin() -> Result<String> {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).context("reading standard input")?;
    Ok(text)
}

// ─────────────────────────── list ───────────────────────────

pub fn list<W: Write>(ctx: &mut Ctx<'_, W>, args: &ListArgs) -> Result<()> {
    let mut query = to_query(&args.filters)?;

    // A selector narrows the listing to one notebook or folder.
    let mut folder: Option<PathBuf> = None;
    if let Some(input) = &args.selector {
        let parsed = Selector::parse(input);
        if let Some(notebook) = &parsed.notebook {
            query.notebook = Some(notebook.clone());
        }
        if !parsed.folder.is_empty() || parsed.target.is_some() {
            match selector::resolve(ctx.workspace, &parsed)? {
                Resolved::Folder { path, .. } => folder = Some(path),
                Resolved::Note { path, .. } => {
                    let note = ctx.read_note(&path)?;
                    write!(ctx.out, "{}", output::render_notes(&[note], ctx.style))?;
                    return Ok(());
                }
                Resolved::Notebook { name, .. } => query.notebook = Some(name),
            }
        }
    }

    let mut notes = search::filter_notes(ctx.workspace, &query)?;
    if let Some(folder) = &folder {
        notes.retain(|note| note.path.starts_with(folder));
    }

    if args.json {
        writeln!(ctx.out, "{}", serde_json::to_string_pretty(&notes)?)?;
    } else if args.paths_only {
        for note in &notes {
            writeln!(ctx.out, "{}", note.path.display())?;
        }
    } else {
        write!(ctx.out, "{}", output::render_notes(&notes, ctx.style))?;
    }
    Ok(())
}

// ─────────────────────────── search ───────────────────────────

pub fn search_notes<W: Write>(ctx: &mut Ctx<'_, W>, args: &SearchArgs) -> Result<()> {
    let query = Query {
        pattern: args.pattern.clone(),
        fixed_string: args.fixed_string,
        case_sensitive: match (args.case_sensitive, args.ignore_case) {
            (true, _) => Some(true),
            (_, true) => Some(false),
            _ => None,
        },
        max_matches_per_note: Some(args.max_matches),
        ..to_query(&args.filters)?
    };

    let hits = search::search(ctx.workspace, &query)?;
    if args.json {
        writeln!(ctx.out, "{}", serde_json::to_string_pretty(&hits)?)?;
    } else if args.files_with_matches {
        for hit in &hits {
            writeln!(ctx.out, "{}", hit.note.path.display())?;
        }
    } else {
        write!(ctx.out, "{}", output::render_hits(&hits, ctx.style))?;
    }
    Ok(())
}

// ─────────────────────────── show / peek / open ───────────────────────────

pub fn show<W: Write>(ctx: &mut Ctx<'_, W>, args: &ShowArgs, mode: ViewMode) -> Result<()> {
    let input = args.selector.as_deref().unwrap_or("");
    let resolved = ctx.resolve(input)?;

    let (path, id) = match &resolved {
        Resolved::Note { path, id } => (path.clone(), *id),
        // Showing a folder or notebook lists it, as `nb` does.
        Resolved::Folder { .. } | Resolved::Notebook { .. } => {
            return list(ctx, &ListArgs {
                selector: Some(input.to_string()),
                filters: FilterArgs::default(),
                paths_only: false,
                json: false,
            });
        }
    };

    if let Some(text) = metadata_field(ctx, args, &path, id)? {
        writeln!(ctx.out, "{text}")?;
        return Ok(());
    }

    if args.opts.print {
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        write!(ctx.out, "{raw}")?;
        return Ok(());
    }

    ctx.out.flush()?;
    match mode {
        ViewMode::Page => shell::page(&path),
        ViewMode::Open => shell::open_externally(&path),
    }
}

/// How `show`, `peek`, and `open` differ once the item is resolved.
#[derive(Clone, Copy)]
pub enum ViewMode {
    /// Render in the terminal.
    Page,
    /// Hand to the system's preferred application.
    Open,
}

/// The `--path` / `--title` / … family: print one field and stop.
fn metadata_field<W: Write>(
    ctx: &Ctx<'_, W>,
    args: &ShowArgs,
    path: &Path,
    id: Option<usize>,
) -> Result<Option<String>> {
    if args.opts.path {
        return Ok(Some(path.display().to_string()));
    }
    if args.opts.filename {
        return Ok(Some(file_name(path)));
    }
    if args.opts.id {
        return Ok(Some(id.map(|id| id.to_string()).unwrap_or_default()));
    }
    if args.opts.relative_path {
        let notebook = ctx.notebook_of(path).context("item is outside the knowledge base")?;
        return Ok(Some(notebook.relative(path).display().to_string()));
    }
    if args.opts.r#type {
        return Ok(Some(
            path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default(),
        ));
    }
    if args.opts.title {
        return Ok(Some(ctx.read_note(path)?.title));
    }
    if args.opts.info_line {
        let note = ctx.read_note(path)?;
        let id = id.map(|id| id.to_string()).unwrap_or_else(|| "-".into());
        return Ok(Some(format!("[{id}] {} · {}", file_name(path), note.title)));
    }
    if args.opts.added || args.opts.updated {
        let note = ctx.read_note(path)?;
        let stamp = if args.opts.added { note.created } else { note.updated };
        return Ok(Some(
            stamp.map(|ts| kb_core::note::format_timestamp(&ts)).unwrap_or_default(),
        ));
    }
    Ok(None)
}

// ─────────────────────────── edit ───────────────────────────

pub fn edit<W: Write>(ctx: &mut Ctx<'_, W>, args: &EditArgs) -> Result<()> {
    let path = if args.last {
        last_modified(ctx)?
    } else {
        let input = args.selector.as_deref().context("no item given")?;
        ctx.resolve_note(input)?
    };

    let content = match args.content.as_deref() {
        Some("-") => Some(read_stdin()?),
        Some(text) => Some(text.to_string()),
        None if !std::io::stdin().is_terminal() => Some(read_stdin()?),
        None => None,
    };

    let Some(content) = content else {
        ctx.out.flush()?;
        return shell::launch(&shell::editor(args.editor.as_deref()), &path);
    };

    let existing = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let updated = if args.overwrite {
        content
    } else if args.prepend {
        // Keep the frontmatter on top: prepending goes before the body, not
        // before the header that describes it.
        let doc = kb_core::Document::split(&existing);
        match doc.span {
            Some(span) => format!("{}{}\n{}", &existing[..span.end], content.trim_end(), doc.body),
            None => format!("{}\n{existing}", content.trim_end()),
        }
    } else {
        format!("{}\n{}\n", existing.trim_end(), content.trim_end())
    };

    std::fs::write(&path, updated).with_context(|| format!("writing {}", path.display()))?;
    writeln!(ctx.out, "{}", path.display())?;
    Ok(())
}

fn last_modified<W: Write>(ctx: &Ctx<'_, W>) -> Result<PathBuf> {
    let notes = search::filter_notes(ctx.workspace, &Query::default())?;
    notes
        .into_iter()
        .next()
        .map(|note| note.path)
        .context("no notes to edit")
}

// ─────────────────────────── delete ───────────────────────────

pub fn delete<W: Write>(ctx: &mut Ctx<'_, W>, args: &DeleteArgs) -> Result<()> {
    let mut targets = Vec::new();
    for input in &args.selectors {
        targets.push((input.clone(), ctx.resolve(input)?));
    }

    if !args.force {
        for (input, resolved) in &targets {
            writeln!(ctx.out, "{input} → {}", resolved.path().display())?;
        }
        ctx.out.flush()?;
        if !shell::confirm(&format!("Delete {} item(s)?", targets.len()))? {
            writeln!(ctx.out, "Cancelled.")?;
            return Ok(());
        }
    }

    for (_, resolved) in &targets {
        items::delete(resolved.path())?;
        writeln!(ctx.out, "Deleted {}", resolved.path().display())?;
    }
    Ok(())
}

// ─────────────────────────── move / copy ───────────────────────────

pub fn move_item<W: Write>(ctx: &mut Ctx<'_, W>, args: &MoveArgs) -> Result<()> {
    let path = ctx.resolve_note(&args.selector)?;

    let moved = if args.to_title {
        let title = ctx.read_note(&path)?.title;
        items::rename_to_title(&path, &title)?
    } else if args.reset {
        let stamp = kb_core::note::timestamp_stem(&jiff::Zoned::now());
        let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        items::rename(&path, &path.with_file_name(format!("{stamp}{ext}")))?
    } else {
        let destination = args.destination.as_deref().context("no destination given")?;
        items::rename(&path, &destination_path(ctx, destination, &path)?)?
    };

    writeln!(ctx.out, "{} → {}", path.display(), moved.display())?;
    Ok(())
}

pub fn copy_item<W: Write>(ctx: &mut Ctx<'_, W>, args: &CopyArgs) -> Result<()> {
    let path = ctx.resolve_note(&args.selector)?;
    let destination = match &args.destination {
        Some(destination) => destination_path(ctx, destination, &path)?,
        None => path.clone(),
    };
    let copied = items::copy(&path, &destination)?;
    writeln!(ctx.out, "{} → {}", path.display(), copied.display())?;
    Ok(())
}

/// Turn a destination selector into a path, whether it names a notebook, a
/// folder, or a filename.
fn destination_path<W: Write>(
    ctx: &Ctx<'_, W>,
    destination: &str,
    source: &Path,
) -> Result<PathBuf> {
    let parsed = Selector::parse(destination);
    let notebook = ctx.notebook(parsed.notebook.as_deref())?;
    let dir = notebook.root.join(parsed.folder_path());

    match &parsed.target {
        Some(kb_core::Target::Name(name)) => {
            let candidate = dir.join(name);
            // A name that is an existing directory means "move into it".
            Ok(if candidate.is_dir() { candidate.join(file_name(source)) } else { candidate })
        }
        Some(kb_core::Target::Id(id)) => Ok(dir.join(id.to_string())),
        None => Ok(dir.join(file_name(source))),
    }
}

// ─────────────────────────── notebooks / use / count ───────────────────────────

pub fn count<W: Write>(ctx: &mut Ctx<'_, W>, args: &SelectorArgs) -> Result<()> {
    let dir = match &args.selector {
        Some(input) => ctx.resolve(input)?.path().to_path_buf(),
        None => ctx.notebook(None)?.root.clone(),
    };
    writeln!(ctx.out, "{}", items::count(&dir)?)?;
    Ok(())
}

pub fn notebooks<W: Write>(ctx: &mut Ctx<'_, W>, args: &NotebooksArgs) -> Result<()> {
    match &args.command {
        Some(NotebooksCommand::Current(current)) => {
            let notebook = ctx.notebook(None)?;
            let text = if current.path {
                notebook.root.display().to_string()
            } else {
                notebook.name.clone()
            };
            writeln!(ctx.out, "{text}")?;
        }
        Some(NotebooksCommand::Add(add)) => {
            let path = ctx.workspace.root.join(&add.name);
            if path.exists() {
                bail!("already exists: {}", path.display());
            }
            std::fs::create_dir_all(&path)
                .with_context(|| format!("creating {}", path.display()))?;
            git::init(&path)?;
            writeln!(ctx.out, "Added {}", path.display())?;
        }
        Some(NotebooksCommand::Delete(delete)) => {
            let notebook = ctx.notebook(Some(&delete.name))?;
            let root = notebook.root.clone();
            if !delete.force {
                ctx.out.flush()?;
                if !shell::confirm(&format!("Delete notebook {} and everything in it?", delete.name))?
                {
                    writeln!(ctx.out, "Cancelled.")?;
                    return Ok(());
                }
            }
            std::fs::remove_dir_all(&root)
                .with_context(|| format!("removing {}", root.display()))?;
            writeln!(ctx.out, "Deleted {}", root.display())?;
        }
        Some(NotebooksCommand::Rename(rename)) => {
            let notebook = ctx.notebook(Some(&rename.old))?;
            let from = notebook.root.clone();
            let to = ctx.workspace.root.join(&rename.new);
            if to.exists() {
                bail!("already exists: {}", to.display());
            }
            std::fs::rename(&from, &to)
                .with_context(|| format!("renaming {} to {}", from.display(), to.display()))?;
            if ctx.workspace.current().as_deref() == Some(rename.old.as_str()) {
                std::fs::write(
                    ctx.workspace.root.join(kb_core::workspace::CURRENT_FILE),
                    format!("{}\n", rename.new),
                )?;
            }
            writeln!(ctx.out, "{} → {}", rename.old, rename.new)?;
        }
        None => {
            let current = ctx.notebook(None)?.name.clone();
            for notebook in &ctx.workspace.notebooks {
                if args.paths {
                    writeln!(ctx.out, "{}", notebook.root.display())?;
                } else if args.names {
                    writeln!(ctx.out, "{}", notebook.name)?;
                } else {
                    let marker = if notebook.name == current { "*" } else { " " };
                    writeln!(ctx.out, "{marker} {}", notebook.name)?;
                }
            }
        }
    }
    Ok(())
}

pub fn use_notebook<W: Write>(ctx: &mut Ctx<'_, W>, args: &UseArgs) -> Result<()> {
    ctx.workspace.set_current(&args.notebook)?;
    writeln!(ctx.out, "{}", args.notebook)?;
    Ok(())
}

// ─────────────────────────── status / git / history ───────────────────────────

pub fn status<W: Write>(ctx: &mut Ctx<'_, W>, args: &NotebookArgs) -> Result<()> {
    for notebook in ctx.workspace.select(args.notebook.as_deref())? {
        writeln!(ctx.out, "{}", ctx.style.path(&notebook.name))?;
        if !git::is_repository(&notebook.root) {
            writeln!(ctx.out, "  not a git repository")?;
            continue;
        }
        let branch = git::current_branch(&notebook.root).unwrap_or_else(|_| "-".into());
        let remote = git::remote_url(&notebook.root).unwrap_or_else(|_| "(none)".into());
        let dirty = !git::is_clean(&notebook.root)?;
        writeln!(ctx.out, "  path    {}", notebook.root.display())?;
        writeln!(ctx.out, "  branch  {branch}")?;
        writeln!(ctx.out, "  remote  {remote}")?;
        writeln!(ctx.out, "  status  {}", if dirty { "uncommitted changes" } else { "clean" })?;
        writeln!(ctx.out, "  items   {}", items::count(&notebook.root)?)?;
    }
    Ok(())
}

pub fn git_passthrough<W: Write>(ctx: &mut Ctx<'_, W>, args: &GitArgs) -> Result<()> {
    let root = ctx.notebook(args.notebook.as_deref())?.root.clone();
    let argv: Vec<&str> = args.args.iter().map(String::as_str).collect();
    ctx.out.flush()?;
    let output = git::run_raw(&root, &argv)?;
    write!(ctx.out, "{output}")?;
    Ok(())
}

pub fn history<W: Write>(ctx: &mut Ctx<'_, W>, args: &SelectorArgs) -> Result<()> {
    let (root, file) = match &args.selector {
        Some(input) => {
            let resolved = ctx.resolve(input)?;
            let path = resolved.path().to_path_buf();
            let notebook = ctx
                .notebook_of(&path)
                .ok_or_else(|| anyhow!("{} is outside the knowledge base", path.display()))?;
            (notebook.root.clone(), Some(notebook.relative(&path)))
        }
        None => (ctx.notebook(None)?.root.clone(), None),
    };

    let mut argv: Vec<String> =
        ["log", "--format=%h %ad %an — %s", "--date=short"].map(String::from).to_vec();
    if let Some(file) = &file {
        argv.push("--".into());
        argv.push(file.to_string_lossy().into_owned());
    }
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    write!(ctx.out, "{}", git::run_raw(&root, &argv)?)?;
    Ok(())
}

// ─────────────────────────── folders ───────────────────────────

pub fn folders<W: Write>(ctx: &mut Ctx<'_, W>, args: &FoldersArgs) -> Result<()> {
    match &args.command {
        Some(FoldersCommand::Add(folder)) => {
            let parsed = Selector::parse(&folder.selector);
            let notebook = ctx.notebook(parsed.notebook.as_deref())?;
            let mut parent = notebook.root.join(parsed.folder_path());
            let name = match &parsed.target {
                Some(kb_core::Target::Name(name)) => name.clone(),
                Some(kb_core::Target::Id(id)) => id.to_string(),
                None => {
                    // A trailing slash puts the name in the folder path.
                    let name = parent
                        .file_name()
                        .context("no folder name given")?
                        .to_string_lossy()
                        .into_owned();
                    parent.pop();
                    name
                }
            };
            let created = items::create_folder(&parent, &name)?;
            writeln!(ctx.out, "{}", created.display())?;
        }
        Some(FoldersCommand::Delete(folder)) => {
            let resolved = ctx.resolve(&folder.selector)?;
            let path = resolved.path().to_path_buf();
            if !path.is_dir() {
                bail!("not a folder: {}", path.display());
            }
            if !folder.force {
                ctx.out.flush()?;
                if !shell::confirm(&format!("Delete folder {} and everything in it?", path.display()))?
                {
                    writeln!(ctx.out, "Cancelled.")?;
                    return Ok(());
                }
            }
            items::delete(&path)?;
            writeln!(ctx.out, "Deleted {}", path.display())?;
        }
        None => {
            let dir = match &args.selector {
                Some(input) => ctx.resolve(input)?.path().to_path_buf(),
                None => ctx.notebook(None)?.root.clone(),
            };
            let index = Index::load(&dir)?;
            for path in items::list_dir(&dir)?.into_iter().filter(|path| path.is_dir()) {
                let name = file_name(&path);
                let id = index.id_of(&name).map(|id| id.to_string()).unwrap_or_else(|| "-".into());
                writeln!(ctx.out, "[{id}] {name}/")?;
            }
        }
    }
    Ok(())
}

// ─────────────────────────── sync / init / reconcile ───────────────────────────

pub fn sync<W: Write>(ctx: &mut Ctx<'_, W>, args: &SyncArgs) -> Result<()> {
    for notebook in ctx.workspace.select(args.notebook.as_deref())? {
        if !git::is_repository(&notebook.root) {
            writeln!(
                ctx.out,
                "{}",
                ctx.style.dim(&format!("{}: not a git repository", notebook.name))
            )?;
            continue;
        }
        let outcome = kb_core::sync::sync(notebook, args.message.as_deref(), args.all)?;
        let summary = match (&outcome.message, outcome.pushed) {
            (Some(message), true) => format!("{message} — pushed"),
            (Some(message), false) => format!("{message} — committed (no upstream)"),
            (None, true) => "nothing to commit — pulled and pushed".to_string(),
            (None, false) => "nothing to do".to_string(),
        };
        writeln!(ctx.out, "{}  {summary}", ctx.style.path(&outcome.notebook))?;
    }
    Ok(())
}

pub fn init<W: Write>(workspace_root: &Path, args: &InitArgs, out: &mut W) -> Result<()> {
    std::fs::create_dir_all(workspace_root)
        .with_context(|| format!("creating {}", workspace_root.display()))?;
    let home = workspace_root.join("home");

    if home.exists() {
        bail!("already initialised: {}", home.display());
    }
    match &args.remote {
        Some(url) => git::clone(url, &home, args.branch.as_deref())?,
        None => {
            std::fs::create_dir_all(&home)?;
            git::init(&home)?;
        }
    }
    std::fs::write(workspace_root.join(kb_core::workspace::CURRENT_FILE), "home\n")?;
    writeln!(out, "Initialised {}", home.display())?;
    Ok(())
}

pub fn reconcile<W: Write>(ctx: &mut Ctx<'_, W>, args: &NotebookArgs) -> Result<()> {
    for notebook in ctx.workspace.select(args.notebook.as_deref())? {
        let updated = items::reconcile(notebook)?;
        writeln!(ctx.out, "{}  {updated} index file(s) updated", ctx.style.path(&notebook.name))?;
    }
    Ok(())
}

// ─────────────────────────── pick / tags / migrate ───────────────────────────

pub fn pick(workspace: &Workspace, args: &PickArgs) -> Result<()> {
    let notes = search::filter_notes(workspace, &to_query(&args.filters)?)?;
    if notes.is_empty() {
        bail!("no notes match those filters");
    }
    let Some(path) = shell::pick(&notes, args.query.as_deref())? else {
        return Ok(());
    };
    if args.edit {
        shell::launch(&shell::editor(None), &path)
    } else {
        shell::page(&path)
    }
}

pub fn tags<W: Write>(ctx: &mut Ctx<'_, W>, filters: &FilterArgs) -> Result<()> {
    let notes = search::filter_notes(ctx.workspace, &to_query(filters)?)?;

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut untagged = 0usize;
    for note in &notes {
        if note.tags.is_empty() {
            untagged += 1;
        }
        for tag in &note.tags {
            *counts.entry(tag.clone()).or_default() += 1;
        }
    }

    let width = counts.keys().map(|t| t.chars().count()).max().unwrap_or(0).max(10);
    let mut rows: Vec<(&String, &usize)> = counts.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

    for (tag, count) in rows {
        writeln!(ctx.out, "{tag:<width$}  {count}")?;
    }
    if untagged > 0 {
        writeln!(ctx.out, "{}", ctx.style.dim(&format!("{:<width$}  {}", "(untagged)", untagged)))?;
    }
    Ok(())
}

pub fn migrate_notes<W: Write>(ctx: &mut Ctx<'_, W>, args: &MigrateArgs) -> Result<()> {
    let notebooks = ctx.workspace.select(args.notebook.as_deref())?;

    // Refuse to touch a dirty tree: the migration's safety net is that its diff
    // can be reviewed on its own, and unrelated edits would spoil that.
    if args.apply && !args.allow_dirty {
        for nb in &notebooks {
            if git::is_repository(&nb.root) && !git::is_clean(&nb.root)? {
                bail!(
                    "notebook `{}` has uncommitted changes; commit them first or pass --allow-dirty",
                    nb.name
                );
            }
        }
    }

    let total: usize = notebooks.iter().map(|nb| nb.note_paths().len()).sum();
    let plans = migrate::plan(ctx.workspace, args.notebook.as_deref())?;

    for plan in &plans {
        let keys: Vec<&str> = plan.added.iter().map(|(key, _)| key.as_str()).collect();
        writeln!(
            ctx.out,
            "{}  {}",
            ctx.style.path(&format!("{}/{}", plan.notebook, plan.rel_path.display())),
            ctx.style.dim(&format!("+ {}", keys.join(", ")))
        )?;
        if args.verbose {
            for (key, value) in &plan.added {
                writeln!(ctx.out, "      {key}: {value}")?;
            }
        }
    }

    writeln!(ctx.out)?;
    writeln!(ctx.out, "{} of {total} notes need frontmatter.", plans.len())?;

    if !args.apply {
        writeln!(ctx.out, "{}", ctx.style.dim("Nothing written — pass --apply to write."))?;
        return Ok(());
    }
    for plan in &plans {
        migrate::apply(plan)?;
    }
    writeln!(ctx.out, "Wrote {} note(s).", plans.len())?;
    Ok(())
}

// ─────────────────────────── helpers ───────────────────────────

fn to_query(filters: &FilterArgs) -> Result<Query> {
    Ok(Query {
        tags: filters.tag.clone(),
        notebook: filters.notebook.clone(),
        since: filters.since.as_deref().map(since::parse).transpose()?,
        limit: filters.limit,
        ..Query::default()
    })
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}
