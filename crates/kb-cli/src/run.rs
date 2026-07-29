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
    let (positional_filename, positional_content) = match &args.target {
        Some(target) if looks_like_path(target) => {
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
            // Piped input with no arguments is content too — but an empty pipe
            // is not content, it is the absence of it.
            None if !std::io::stdin().is_terminal() => {
                let text = read_stdin()?;
                (!text.trim().is_empty()).then_some(text)
            }
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

    // Encryption replaces the plaintext file, so it happens before indexing —
    // the index must name the file that actually exists.
    let path = if args.encrypt {
        let tool = encryption_tool()?;
        let password = resolve_password(args.password.as_deref(), true)?;
        let encrypted = kb_core::encrypt::encrypted_path(&path);
        kb_core::encrypt::encrypt(tool, &path, &encrypted, &password)?;
        std::fs::remove_file(&path)
            .with_context(|| format!("removing {}", path.display()))?;
        encrypted
    } else {
        path
    };

    let dir = path.parent().context("no parent directory")?;
    let mut index = Index::load(dir)?;
    index.add(&file_name(&path));
    index.save(dir)?;

    writeln!(ctx.out, "{}", path.display())?;

    // Content means the note is already written; an editor would only be in the
    // way of a script. `--edit` asks for it anyway.
    if args.edit || (!has_content && !args.no_edit) {
        ctx.out.flush()?;
        shell::launch(&editor_for(ctx, None), &path)?;
    }
    Ok(())
}

/// Whether a bare argument to `add` names a path rather than content.
///
/// A slash settles it — `knowledge/` is a folder and `knowledge/noext` a file
/// in it, regardless of extension. Only without one does `nb` fall back to
/// looking for a file extension, treating anything else as the note's content.
fn looks_like_path(value: &str) -> bool {
    value.contains('/') || Path::new(value).extension().is_some()
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

        // A folder has no single thing to show. `show` lists it, as `nb` does;
        // `open` and `peek` hand it to the picker, which is what `nb` reaches
        // for a file browser to do — except this one previews the notes.
        Resolved::Folder { path, .. } | Resolved::Notebook { root: path, .. } => {
            if mode.browses_folders() && !args.opts.print {
                let Some(chosen) = pick_within(ctx, path)? else {
                    return Ok(()); // the picker was dismissed
                };
                ctx.out.flush()?;
                return match mode {
                    // `open` means open it to work on; `peek` means just look.
                    ViewMode::Open => shell::launch(&editor_for(ctx, None), &chosen),
                    _ => shell::page(&chosen),
                };
            }
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

    // An encrypted item is decrypted to a temporary file that is removed as soon
    // as the viewer exits, so plaintext never lands in the notebook.
    if kb_core::encrypt::is_encrypted(&path) {
        let tool = encryption_tool()?;
        let password = resolve_password(args.opts.password.as_deref(), false)?;
        let plain = temp_path(&path);
        kb_core::encrypt::decrypt(tool, &path, &plain, &password)?;

        let result = if args.opts.print {
            std::fs::read_to_string(&plain)
                .with_context(|| format!("reading {}", plain.display()))
                .and_then(|raw| write!(ctx.out, "{raw}").map_err(Into::into))
        } else {
            ctx.out.flush()?;
            match mode {
                ViewMode::Show | ViewMode::Peek => shell::page(&plain),
                ViewMode::Open => shell::open_externally(&plain),
            }
        };
        let _ = std::fs::remove_file(&plain);
        return result;
    }

    if args.opts.print {
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        write!(ctx.out, "{raw}")?;
        return Ok(());
    }

    ctx.out.flush()?;
    match mode {
        ViewMode::Show | ViewMode::Peek => shell::page(&path),
        ViewMode::Open => shell::open_externally(&path),
    }
}

/// The configured encryption tool.
fn encryption_tool() -> Result<kb_core::encrypt::Tool> {
    let settings = kb_core::settings::Settings::load()?;
    kb_core::encrypt::Tool::from_setting(settings.get("encryption_tool").as_deref())
}

/// The editor to use, honouring the settings file alongside the environment.
fn editor_for<W: Write>(_ctx: &Ctx<'_, W>, override_with: Option<&str>) -> String {
    resolve_editor(override_with)
}

/// Work out the editor the way `nb` does, including the rc file's own logic.
///
/// The rc file is a shell script and may decide the editor at run time — mine
/// picks `cat` inside an automated session so nothing blocks on an editor that
/// will never be closed. Honouring that means sourcing the file, exactly as `nb`
/// does, rather than reading `export` lines out of it.
fn resolve_editor(override_with: Option<&str>) -> String {
    if let Some(editor) = override_with {
        return editor.to_string();
    }

    // An environment variable is the caller being explicit; nothing to work out.
    if std::env::var_os("KB_EDITOR").is_some() || std::env::var_os("NB_EDITOR").is_some() {
        return shell::editor(None, None);
    }

    // Otherwise ask the rc file by running it, not by reading it. It is a shell
    // script and may well branch — reading the text would see both sides of an
    // `if` and take whichever came last, which is how a guard meant to keep an
    // editor out of an automated session silently stops working.
    let from_rc = kb_core::settings::shell_environment();
    let rc_editor = ["KB_EDITOR", "NB_EDITOR", "EDITOR", "VISUAL"]
        .iter()
        .find_map(|key| from_rc.get(*key))
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty());

    shell::editor(None, rc_editor)
}

/// Use the given password, or ask for one.
fn resolve_password(given: Option<&str>, confirm: bool) -> Result<String> {
    match given {
        Some(password) => Ok(password.to_string()),
        None => shell::prompt_password(confirm),
    }
}

/// A scratch path beside the encrypted file, for the decrypted copy.
fn temp_path(encrypted: &Path) -> PathBuf {
    let dir = encrypted.parent().unwrap_or(Path::new("."));
    let stem = kb_core::encrypt::decrypted_path(encrypted);
    let name = stem.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    dir.join(format!(".kb-decrypted-{}-{name}", std::process::id()))
}

/// How `show`, `peek`, and `open` differ once the item is resolved.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// `show` — render in the terminal; a folder is listed.
    Show,
    /// `peek` — render in the terminal; a folder opens the picker.
    Peek,
    /// `open` — hand to the system; a folder opens the picker.
    Open,
}

impl ViewMode {
    /// Whether a folder should be browsed rather than listed.
    fn browses_folders(self) -> bool {
        matches!(self, Self::Peek | Self::Open)
    }
}

/// Let the user choose a note from `dir` with fzf.
fn pick_within<W: Write>(ctx: &Ctx<'_, W>, dir: &Path) -> Result<Option<PathBuf>> {
    let mut notes = search::filter_notes(ctx.workspace, &Query::default())?;
    notes.retain(|note| note.path.starts_with(dir));
    if notes.is_empty() {
        bail!("no notes under {}", dir.display());
    }
    shell::pick(&notes, None)
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
        return shell::launch(&editor_for(ctx, args.editor.as_deref()), &path);
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

    let stamp = kb_core::note::format_timestamp(&jiff::Zoned::now());
    let updated = kb_core::frontmatter::touch_updated(&updated, &stamp);
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
        let configured = kb_core::settings::Settings::load()
            .ok()
            .and_then(|settings| settings.get("editor"));
        shell::launch(&shell::editor(None, configured.as_deref()), &path)
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

// ─────────────────────────── bookmark ───────────────────────────

pub fn bookmark<W: Write>(ctx: &mut Ctx<'_, W>, args: &BookmarkArgs) -> Result<()> {
    match &args.command {
        Some(BookmarkCommand::List(filters)) => return list_bookmarks(ctx, filters),
        Some(BookmarkCommand::Url(target)) => {
            let path = bookmark_path(ctx, target)?;
            let raw = std::fs::read_to_string(&path)?;
            let url = kb_core::bookmark::url_of(&raw).context("no URL in that bookmark")?;
            writeln!(ctx.out, "{url}")?;
            return Ok(());
        }
        Some(BookmarkCommand::Open(target)) => {
            let path = bookmark_path(ctx, target)?;
            let raw = std::fs::read_to_string(&path)?;
            let url = kb_core::bookmark::url_of(&raw).context("no URL in that bookmark")?;
            ctx.out.flush()?;
            return shell::open_url(&url);
        }
        Some(BookmarkCommand::Peek(target)) => {
            let path = bookmark_path(ctx, target)?;
            ctx.out.flush()?;
            return shell::page(&path);
        }
        Some(BookmarkCommand::Edit(edit_args)) => return edit(ctx, edit_args),
        Some(BookmarkCommand::Delete(delete_args)) => return delete(ctx, delete_args),
        Some(BookmarkCommand::Search(search_args)) => {
            // Same search, narrowed to bookmarks.
            let hits = {
                let query = Query {
                    pattern: search_args.pattern.clone(),
                    fixed_string: search_args.fixed_string,
                    max_matches_per_note: Some(search_args.max_matches),
                    ..to_query(&search_args.filters)?
                };
                let mut hits = search::search(ctx.workspace, &query)?;
                hits.retain(|hit| kb_core::bookmark::is_bookmark(&hit.note.path));
                hits
            };
            write!(ctx.out, "{}", output::render_hits(&hits, ctx.style))?;
            return Ok(());
        }
        None => {}
    }

    if args.urls.is_empty() {
        return list_bookmarks(ctx, &FilterArgs::default());
    }

    let folder = args.opts.folder.clone().unwrap_or_default();
    let parsed_folder = Selector::parse(&folder);
    let notebook = ctx.notebook(parsed_folder.notebook.as_deref())?.clone();
    let dir = parsed_folder.folder_path().to_string_lossy().into_owned();

    for url in &args.urls {
        let spec = kb_core::bookmark::NewBookmark {
            url: url.clone(),
            title: args.opts.title.clone(),
            comment: args.opts.comment.clone(),
            quote: args.opts.quote.clone(),
            tags: args.opts.tags.clone(),
            related: args.opts.related.clone(),
            filename: args.opts.filename.clone(),
            no_request: args.opts.no_request,
            save_source: args.opts.save_source,
        };
        let (path, source) = kb_core::bookmark::create(&notebook, &dir, &spec, &jiff::Zoned::now())?;
        writeln!(ctx.out, "{}", path.display())?;
        if let Some(source) = source {
            writeln!(ctx.out, "{}", source.display())?;
        }
    }
    Ok(())
}

fn list_bookmarks<W: Write>(ctx: &mut Ctx<'_, W>, filters: &FilterArgs) -> Result<()> {
    let mut notes = search::filter_notes(ctx.workspace, &to_query(filters)?)?;
    notes.retain(|note| kb_core::bookmark::is_bookmark(&note.path));
    write!(ctx.out, "{}", output::render_notes(&notes, ctx.style))?;
    Ok(())
}

fn bookmark_path<W: Write>(ctx: &Ctx<'_, W>, target: &SelectorArgs) -> Result<PathBuf> {
    let input = target.selector.as_deref().context("no bookmark given")?;
    ctx.resolve_note(input)
}

// ─────────────────────────── browse ───────────────────────────

pub fn browse<W: Write>(ctx: &mut Ctx<'_, W>, args: &BrowseArgs) -> Result<()> {
    let url_path = browse_path(ctx, args)?;

    // `--print` renders one page and exits; no server, no browser.
    if args.print {
        let html = kb_core::browse::handle(ctx.workspace, &url_path, None)?;
        write!(ctx.out, "{html}")?;
        return Ok(());
    }

    let address = format!("http://localhost:{}{url_path}", args.port);
    writeln!(ctx.out, "{}", ctx.style.path(&address))?;
    writeln!(ctx.out, "{}", ctx.style.dim("Press Ctrl-C to stop."))?;
    ctx.out.flush()?;

    if args.gui {
        // Opening before the server is up would race; the browser retries, but
        // starting the listener first makes it a non-issue.
        let workspace = ctx.workspace.clone();
        let port = args.port;
        let opened = address.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            let _ = shell::open_url(&opened);
        });
        return kb_core::browse::serve(&workspace, port);
    }
    kb_core::browse::serve(ctx.workspace, args.port)
}

/// The URL path a browse invocation opens.
fn browse_path<W: Write>(ctx: &Ctx<'_, W>, args: &BrowseArgs) -> Result<String> {
    if args.notebooks {
        return Ok("/?--notebooks".to_string());
    }
    let notebook = ctx.notebook(None)?.name.clone();

    if let Some(tag) = &args.tag {
        let tag = tag.trim_start_matches('#');
        return Ok(format!("/{notebook}:?--query={}", kb_core::render::url_encode(&format!("#{tag}"))));
    }
    if let Some(query) = &args.query {
        return Ok(format!("/{notebook}:?--query={}", kb_core::render::url_encode(query)));
    }
    match &args.selector {
        Some(selector) => Ok(format!("/{}", kb_core::render::url_encode(selector))),
        None => Ok(format!("/{notebook}:")),
    }
}

// ─────────────────────────── todo / pin / archive ───────────────────────────

pub fn todo<W: Write>(ctx: &mut Ctx<'_, W>, args: &TodoArgs) -> Result<()> {
    match &args.command {
        Some(TodoCommand::Add(add)) => {
            let folder = add.folder.clone().unwrap_or_default();
            let parsed = Selector::parse(&folder);
            let notebook = ctx.notebook(parsed.notebook.as_deref())?.clone();
            let dir = notebook.root.join(parsed.folder_path());
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("creating {}", dir.display()))?;

            let task = add.task.join(" ");
            let now = jiff::Zoned::now();
            let stem = kb_core::note::timestamp_stem(&now);
            let path = unique_path(&dir, &stem, kb_core::todo::TODO_EXT);

            let stamp = kb_core::note::format_timestamp(&now);
            let tags =
                if add.tags.is_empty() { vec!["todo".to_string()] } else { add.tags.clone() };
            let contents = format!(
                "---\ntitle: {}\ntags: {}\ncreated: {stamp}\nupdated: {stamp}\n---\n\n{}",
                kb_core::frontmatter::yaml_scalar(&task),
                kb_core::frontmatter::yaml_tags(&tags),
                kb_core::todo::render(&task, &add.tags),
            );
            std::fs::write(&path, contents)
                .with_context(|| format!("writing {}", path.display()))?;

            let mut index = Index::load(&dir)?;
            index.add(&file_name(&path));
            index.save(&dir)?;

            writeln!(ctx.out, "{}", path.display())?;
            Ok(())
        }
        Some(TodoCommand::Do(target)) => set_todo(ctx, target, true),
        Some(TodoCommand::Undo(target)) => set_todo(ctx, target, false),
        Some(TodoCommand::Done(filters)) => list_todos(ctx, filters, TodoFilter::Done),
        Some(TodoCommand::Open(filters)) => list_todos(ctx, filters, TodoFilter::Open),
        Some(TodoCommand::List(list)) => list_todos(
            ctx,
            &list.filters,
            if list.all { TodoFilter::All } else { TodoFilter::Open },
        ),
        None => list_todos(
            ctx,
            &args.filters,
            if args.all { TodoFilter::All } else { TodoFilter::Open },
        ),
    }
}

enum TodoFilter {
    Open,
    Done,
    All,
}

fn list_todos<W: Write>(
    ctx: &mut Ctx<'_, W>,
    filters: &FilterArgs,
    which: TodoFilter,
) -> Result<()> {
    let notes = search::filter_notes(ctx.workspace, &to_query(filters)?)?;

    for note in notes.iter().filter(|note| kb_core::todo::is_todo(&note.path)) {
        let raw = std::fs::read_to_string(&note.path)?;
        let done = kb_core::todo::is_done(&raw);
        let keep = match which {
            TodoFilter::Open => !done,
            TodoFilter::Done => done,
            TodoFilter::All => true,
        };
        if !keep {
            continue;
        }
        let task = kb_core::todo::task_of(&raw).unwrap_or_else(|| note.title.clone());
        let mark = if done { "[x]" } else { "[ ]" };
        let id = item_id(&note.path).map(|id| id.to_string()).unwrap_or_else(|| "-".into());
        writeln!(ctx.out, "[{id}] {mark} {task}")?;
    }
    Ok(())
}

fn set_todo<W: Write>(ctx: &mut Ctx<'_, W>, target: &SelectorArgs, done: bool) -> Result<()> {
    let input = target.selector.as_deref().context("no todo given")?;
    let path = ctx.resolve_note(input)?;
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;

    let updated = kb_core::todo::set_done(&raw, done)
        .with_context(|| format!("{} has no checkbox to change", path.display()))?;
    std::fs::write(&path, &updated)
        .with_context(|| format!("writing {}", path.display()))?;

    let task = kb_core::todo::task_of(&updated).unwrap_or_default();
    writeln!(ctx.out, "[{}] {task}", if done { "x" } else { " " })?;
    Ok(())
}

pub fn pin<W: Write>(ctx: &mut Ctx<'_, W>, target: &SelectorArgs, pinned: bool) -> Result<()> {
    let input = target.selector.as_deref().context("no item given")?;
    let path = ctx.resolve(input)?.path().to_path_buf();
    let dir = path.parent().context("item has no parent directory")?;
    let name = file_name(&path);

    if pinned {
        kb_core::todo::pin(dir, &name)?;
    } else {
        kb_core::todo::unpin(dir, &name)?;
    }
    writeln!(ctx.out, "{} {}", if pinned { "Pinned" } else { "Unpinned" }, path.display())?;
    Ok(())
}

pub fn archive<W: Write>(
    ctx: &mut Ctx<'_, W>,
    args: &NotebookArgs,
    archived: bool,
) -> Result<()> {
    let notebook = ctx.notebook(args.notebook.as_deref())?.clone();
    if archived {
        kb_core::todo::archive(&notebook.root)?;
    } else {
        kb_core::todo::unarchive(&notebook.root)?;
    }
    writeln!(
        ctx.out,
        "{} {}",
        notebook.name,
        if archived { "archived" } else { "unarchived" }
    )?;
    Ok(())
}

/// The index id of an item, if its directory has one.
fn item_id(path: &Path) -> Option<usize> {
    let dir = path.parent()?;
    Index::load(dir).ok()?.id_of(&file_name(path))
}

fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2u32..)
        .map(|n| dir.join(format!("{stem}-{n}.{ext}")))
        .find(|candidate| !candidate.exists())
        .expect("an unused filename exists")
}

// ─────────────────────────── plugins ───────────────────────────

pub fn plugins<W: Write>(ctx: &mut Ctx<'_, W>, args: &PluginsArgs) -> Result<()> {
    let root = ctx.workspace.root.clone();

    match &args.command {
        Some(PluginsCommand::Install(install)) => {
            let plugin = kb_core::plugins::install(&root, &install.path, install.force)?;
            writeln!(ctx.out, "Installed {} → {}", plugin.name, plugin.path.display())?;
        }
        Some(PluginsCommand::Uninstall(uninstall)) => {
            if !uninstall.force {
                ctx.out.flush()?;
                if !shell::confirm(&format!("Uninstall plugin {}?", uninstall.name))? {
                    writeln!(ctx.out, "Cancelled.")?;
                    return Ok(());
                }
            }
            let plugin = kb_core::plugins::uninstall(&root, &uninstall.name)?;
            writeln!(ctx.out, "Uninstalled {}", plugin.name)?;
        }
        None => {
            let installed = kb_core::plugins::installed(&root);
            let matching = installed
                .iter()
                .filter(|plugin| args.name.as_ref().is_none_or(|name| &plugin.name == name));

            for plugin in matching {
                if args.paths {
                    writeln!(ctx.out, "{}", plugin.path.display())?;
                } else {
                    let kind = if plugin.is_theme() { " (theme)" } else { "" };
                    writeln!(ctx.out, "{}{kind}", plugin.name)?;
                }
            }
        }
    }
    Ok(())
}

// ─────────────────────────── settings ───────────────────────────

pub fn settings<W: Write>(ctx: &mut Ctx<'_, W>, args: &SettingsArgs) -> Result<()> {

    let mut settings = kb_core::settings::Settings::load()?;

    match &args.command {
        Some(SettingsCommand::Get(name)) | Some(SettingsCommand::Show(name)) => {
            let name = kb_core::settings::resolve_name(&name.name)?;
            match settings.get(&name) {
                Some(value) => writeln!(ctx.out, "{value}")?,
                None => writeln!(ctx.out, "{}", ctx.style.dim("(unset)"))?,
            }
        }
        Some(SettingsCommand::Set(set)) => return set_setting(ctx, set),
        Some(SettingsCommand::Unset(name)) => {
            let name = kb_core::settings::resolve_name(&name.name)?;
            settings.unset(&name)?;
            writeln!(ctx.out, "unset {name}")?;
        }
        Some(SettingsCommand::Edit) => {
            let path = settings.path().to_path_buf();
            if !path.exists() {
                std::fs::write(&path, "")?;
            }
            ctx.out.flush()?;
            return shell::launch(&editor_for(ctx, None), &path);
        }
        Some(SettingsCommand::List(list)) => {
            for (number, name) in kb_core::settings::KNOWN.iter().enumerate() {
                if list.long {
                    let value = settings.get(name).unwrap_or_default();
                    writeln!(ctx.out, "[{}] {name:<20} {value}", number + 1)?;
                } else {
                    writeln!(ctx.out, "[{}] {name}", number + 1)?;
                }
            }
        }
        None => {
            for (number, name) in kb_core::settings::KNOWN.iter().enumerate() {
                let value = settings.get(name).unwrap_or_default();
                writeln!(ctx.out, "[{}] {name:<20} {value}", number + 1)?;
            }
        }
    }
    Ok(())
}

pub fn set_setting<W: Write>(ctx: &mut Ctx<'_, W>, args: &SetArgs) -> Result<()> {

    let mut settings = kb_core::settings::Settings::load()?;
    let name = kb_core::settings::resolve_name(&args.name)?;

    // `kb set <name>` with no value prints the current one, as `nb` does.
    let Some(value) = &args.value else {
        match settings.get(&name) {
            Some(value) => writeln!(ctx.out, "{value}")?,
            None => writeln!(ctx.out, "{}", ctx.style.dim("(unset)"))?,
        }
        return Ok(());
    };

    settings.set(&name, value)?;
    writeln!(ctx.out, "{name} = {value}")?;
    Ok(())
}

pub fn unset_setting<W: Write>(ctx: &mut Ctx<'_, W>, args: &SettingNameArgs) -> Result<()> {

    let mut settings = kb_core::settings::Settings::load()?;
    let name = kb_core::settings::resolve_name(&args.name)?;
    settings.unset(&name)?;
    writeln!(ctx.out, "unset {name}")?;
    Ok(())
}

// ─────────────────────────── remote / run / shell ───────────────────────────

pub fn remote<W: Write>(ctx: &mut Ctx<'_, W>, args: &RemoteArgs) -> Result<()> {
    match &args.command {
        Some(RemoteCommand::Set(set)) => {
            let root = ctx.notebook(set.notebook.as_deref())?.root.clone();
            git::set_remote(&root, &set.url)?;
            writeln!(ctx.out, "{}", set.url)?;
        }
        Some(RemoteCommand::Remove(remove)) => {
            let notebook = ctx.notebook(remove.notebook.as_deref())?.clone();
            if !remove.force {
                ctx.out.flush()?;
                if !shell::confirm(&format!("Remove the remote from {}?", notebook.name))? {
                    writeln!(ctx.out, "Cancelled.")?;
                    return Ok(());
                }
            }
            git::remove_remote(&notebook.root)?;
            writeln!(ctx.out, "removed")?;
        }
        None => {
            for notebook in ctx.workspace.select(args.notebook.as_deref())? {
                let url = git::remote_url(&notebook.root).unwrap_or_else(|_| "(none)".into());
                writeln!(ctx.out, "{}  {url}", ctx.style.path(&notebook.name))?;
            }
        }
    }
    Ok(())
}

pub fn run_in_notebook<W: Write>(ctx: &mut Ctx<'_, W>, args: &RunArgs) -> Result<()> {
    let root = ctx.notebook(args.notebook.as_deref())?.root.clone();
    ctx.out.flush()?;

    let status = std::process::Command::new(&args.command[0])
        .args(&args.command[1..])
        .current_dir(&root)
        .status()
        .with_context(|| format!("running {}", args.command[0]))?;
    if !status.success() {
        bail!("{} exited with {status}", args.command[0]);
    }
    Ok(())
}

pub fn interactive_shell<W: Write>(ctx: &mut Ctx<'_, W>, args: &ShellArgs) -> Result<()> {
    let root = ctx.notebook(args.notebook.as_deref())?.root.clone();
    let shell_program = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());

    writeln!(ctx.out, "{}", ctx.style.dim(&format!("{} — type `exit` to leave", root.display())))?;
    ctx.out.flush()?;

    std::process::Command::new(&shell_program)
        .current_dir(&root)
        .status()
        .with_context(|| format!("running {shell_program}"))?;
    Ok(())
}

// ─────────────────────────── import / export ───────────────────────────

pub fn import<W: Write>(ctx: &mut Ctx<'_, W>, args: &ImportArgs) -> Result<()> {
    let target = args.to.clone().unwrap_or_default();
    let parsed = Selector::parse(&target);
    let notebook = ctx.notebook(parsed.notebook.as_deref())?.clone();
    let dir = notebook.root.join(parsed.folder_path());
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;

    for source in &args.paths {
        if !source.exists() {
            bail!("not found: {}", source.display());
        }
        let name = source
            .file_name()
            .with_context(|| format!("{} has no filename", source.display()))?;
        let destination = unique_beside(&dir.join(name));

        if args.move_files {
            std::fs::rename(source, &destination).or_else(|_| {
                // A rename across filesystems fails; fall back to copy + remove.
                std::fs::copy(source, &destination)
                    .and_then(|_| std::fs::remove_file(source))
                    .map(|_| ())
            })?;
        } else {
            std::fs::copy(source, &destination)
                .with_context(|| format!("copying {}", source.display()))?;
        }

        let mut index = Index::load(&dir)?;
        index.add(&file_name(&destination));
        index.save(&dir)?;
        writeln!(ctx.out, "{}", destination.display())?;
    }
    Ok(())
}

pub fn export<W: Write>(ctx: &mut Ctx<'_, W>, args: &ExportArgs) -> Result<()> {
    let source = ctx.resolve(&args.selector)?.path().to_path_buf();

    // A directory destination keeps the item's own filename.
    let destination = if args.path.is_dir() {
        args.path.join(source.file_name().context("item has no filename")?)
    } else {
        args.path.clone()
    };

    if destination.exists() && !args.force {
        ctx.out.flush()?;
        if !shell::confirm(&format!("Overwrite {}?", destination.display()))? {
            writeln!(ctx.out, "Cancelled.")?;
            return Ok(());
        }
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(&source, &destination)
        .with_context(|| format!("copying {} to {}", source.display(), destination.display()))?;
    writeln!(ctx.out, "{}", destination.display())?;
    Ok(())
}

// ─────────────────────────── env / subcommands / update ───────────────────────────

pub fn env<W: Write>(ctx: &mut Ctx<'_, W>, args: &EnvArgs) -> Result<()> {
    let notebook = ctx.notebook(None)?.name.clone();
    writeln!(ctx.out, "kb        {}", env!("CARGO_PKG_VERSION"))?;
    writeln!(ctx.out, "root      {}", ctx.workspace.root.display())?;
    writeln!(ctx.out, "notebook  {notebook}")?;
    // AGPL asks that users be able to find the source; say where it is.
    writeln!(ctx.out, "licence   AGPL-3.0-or-later")?;
    writeln!(ctx.out, "source    {}", env!("CARGO_PKG_REPOSITORY"))?;

    if args.long {
        let settings = kb_core::settings::Settings::load()?;
        writeln!(ctx.out, "config    {}", settings.path().display())?;
        writeln!(ctx.out, "editor    {}", editor_for(ctx, None))?;
        for tool in ["git", "fzf", "glow", "bat", "pandoc", "gpg", "openssl"] {
            let found = if shell::has_command(tool) { "yes" } else { "no" };
            writeln!(ctx.out, "{tool:<9} {found}")?;
        }
        for (name, value) in settings.entries() {
            writeln!(ctx.out, "set       {name} = {value}")?;
        }
    }
    Ok(())
}

/// A destination path that does not collide: `a.md` → `a-2.md`.
fn unique_beside(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2u32..)
        .map(|n| dir.join(format!("{stem}-{n}{ext}")))
        .find(|candidate| !candidate.exists())
        .expect("an unused filename exists")
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
