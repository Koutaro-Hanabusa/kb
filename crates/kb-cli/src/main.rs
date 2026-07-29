//! `kb` — a fast Markdown knowledge base.

mod output;
mod since;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use kb_core::{Query, Workspace, git, migrate, search};

use output::Style;

#[derive(Parser)]
#[command(name = "kb", version, about = "A fast Markdown knowledge base")]
struct Cli {
    /// Knowledge base root (defaults to $KB_ROOT, then ~/.nb)
    #[arg(long, global = true, value_name = "DIR")]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Search note bodies
    Search(SearchArgs),
    /// List notes
    #[command(alias = "list")]
    Ls(ListArgs),
    /// Show tags and how many notes carry them
    Tags(FilterArgs),
    /// Create a note and open it in $EDITOR
    New(NewArgs),
    /// Pick a note with fzf and read it
    Open(OpenArgs),
    /// Commit note changes, then pull and push
    Sync(SyncArgs),
    /// Add missing frontmatter to existing notes
    Migrate(MigrateArgs),
}

#[derive(Args)]
struct NewArgs {
    /// Title of the new note
    title: String,

    /// Notebook to write to (defaults to work on a work machine, else home)
    #[arg(short = 'n', long, value_name = "NAME")]
    notebook: Option<String>,

    /// Directory within the notebook
    #[arg(short, long, default_value = "knowledge")]
    dir: String,

    /// Tag the note; repeat for several (defaults to the directory name)
    #[arg(short, long, value_name = "TAG")]
    tag: Vec<String>,

    /// Body text, or `-` to read it from stdin (implies --no-edit)
    #[arg(short, long, value_name = "TEXT")]
    content: Option<String>,

    /// Print the path instead of opening an editor
    #[arg(long)]
    no_edit: bool,
}

#[derive(Args)]
struct OpenArgs {
    #[command(flatten)]
    filters: FilterArgs,

    /// Initial fzf query
    #[arg(value_name = "QUERY")]
    query: Option<String>,

    /// Open the selection in $EDITOR instead of the pager
    #[arg(short, long)]
    edit: bool,
}

#[derive(Args)]
struct SyncArgs {
    /// Limit to one notebook
    #[arg(short = 'n', long, value_name = "NAME")]
    notebook: Option<String>,

    /// Commit message (default: generated from the staged files)
    #[arg(short, long, value_name = "TEXT")]
    message: Option<String>,

    /// Stage every change, not just Markdown
    #[arg(long)]
    all: bool,
}

/// Filters shared by every command that selects notes.
#[derive(Args, Clone, Default)]
struct FilterArgs {
    /// Limit to one notebook
    #[arg(short = 'n', long, value_name = "NAME")]
    notebook: Option<String>,

    /// Require a tag; repeat to require several
    #[arg(short, long, value_name = "TAG")]
    tag: Vec<String>,

    /// Only notes touched since a date or duration (`7d`, `3w`, `2026-01-01`)
    #[arg(short, long, value_name = "WHEN")]
    since: Option<String>,

    /// Maximum results
    // No short flag: `-l` belongs to `--files-with-matches`, as in grep.
    #[arg(long, value_name = "N")]
    limit: Option<usize>,
}

impl FilterArgs {
    fn to_query(&self) -> Result<Query> {
        Ok(Query {
            tags: self.tag.clone(),
            notebook: self.notebook.clone(),
            since: self.since.as_deref().map(since::parse).transpose()?,
            limit: self.limit,
            ..Query::default()
        })
    }
}

#[derive(Args)]
struct SearchArgs {
    /// Regular expression to search for
    pattern: String,

    #[command(flatten)]
    filters: FilterArgs,

    /// Match the pattern literally instead of as a regular expression
    #[arg(short = 'F', long)]
    fixed_string: bool,

    /// Match case-sensitively (default: smart case)
    #[arg(short = 's', long, conflicts_with = "ignore_case")]
    case_sensitive: bool,

    /// Match case-insensitively
    #[arg(short = 'i', long)]
    ignore_case: bool,

    /// Matching lines to show per note
    #[arg(short = 'm', long, value_name = "N", default_value_t = kb_core::search::DEFAULT_MAX_MATCHES)]
    max_matches: usize,

    /// Print only the paths of matching notes
    #[arg(short = 'l', long)]
    files_with_matches: bool,

    /// Print results as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct MigrateArgs {
    /// Limit to one notebook
    #[arg(short = 'n', long, value_name = "NAME")]
    notebook: Option<String>,

    /// Write the changes; without this the run only reports what it would do
    #[arg(long)]
    apply: bool,

    /// Show the value of every key being added
    #[arg(short, long)]
    verbose: bool,

    /// Migrate even when a notebook has uncommitted changes
    #[arg(long)]
    allow_dirty: bool,
}

#[derive(Args)]
struct ListArgs {
    #[command(flatten)]
    filters: FilterArgs,

    /// Print only paths
    #[arg(short = 'l', long)]
    paths_only: bool,

    /// Print results as JSON
    #[arg(long)]
    json: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let workspace = match &cli.root {
        Some(root) => Workspace::open(root)?,
        None => Workspace::discover()?,
    };
    let style = Style::detect();

    // Write through a locked, buffered handle: `kb ls` prints hundreds of lines
    // and a write syscall per line is pure waste.
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    match cli.command {
        Command::Search(args) => run_search(&workspace, &args, style, &mut out)?,
        Command::Ls(args) => run_list(&workspace, &args, style, &mut out)?,
        Command::Tags(filters) => run_tags(&workspace, &filters, style, &mut out)?,
        Command::Migrate(args) => run_migrate(&workspace, &args, style, &mut out)?,
        Command::New(args) => run_new(&workspace, &args, &mut out)?,
        Command::Open(args) => {
            // fzf and the pager need the real terminal, so flush what is buffered
            // before handing stdout over.
            out.flush()?;
            return run_open(&workspace, &args);
        }
        Command::Sync(args) => run_sync(&workspace, &args, style, &mut out)?,
    }
    out.flush()?;
    Ok(())
}

fn run_search(
    workspace: &Workspace,
    args: &SearchArgs,
    style: Style,
    out: &mut impl Write,
) -> Result<()> {
    let query = Query {
        pattern: args.pattern.clone(),
        fixed_string: args.fixed_string,
        case_sensitive: match (args.case_sensitive, args.ignore_case) {
            (true, _) => Some(true),
            (_, true) => Some(false),
            _ => None,
        },
        max_matches_per_note: Some(args.max_matches),
        ..args.filters.to_query()?
    };

    let hits = search::search(workspace, &query)?;
    if args.json {
        writeln!(out, "{}", serde_json::to_string_pretty(&hits)?)?;
    } else if args.files_with_matches {
        for hit in &hits {
            writeln!(out, "{}", hit.note.path.display())?;
        }
    } else {
        write!(out, "{}", output::render_hits(&hits, style))?;
    }
    Ok(())
}

fn run_list(
    workspace: &Workspace,
    args: &ListArgs,
    style: Style,
    out: &mut impl Write,
) -> Result<()> {
    let notes = search::filter_notes(workspace, &args.filters.to_query()?)?;
    if args.json {
        writeln!(out, "{}", serde_json::to_string_pretty(&notes)?)?;
    } else if args.paths_only {
        for note in &notes {
            writeln!(out, "{}", note.path.display())?;
        }
    } else {
        write!(out, "{}", output::render_notes(&notes, style))?;
    }
    Ok(())
}

fn run_new(workspace: &Workspace, args: &NewArgs, out: &mut impl Write) -> Result<()> {
    let notebook = match &args.notebook {
        Some(name) => {
            workspace.notebook(name).with_context(|| format!("unknown notebook `{name}`"))?
        }
        None => workspace.default_notebook()?,
    };

    let body = match args.content.as_deref() {
        Some("-") => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
                .context("reading the note body from stdin")?;
            Some(text)
        }
        other => other.map(str::to_string),
    };

    let spec = kb_core::NewNote {
        title: args.title.clone(),
        dir: args.dir.clone(),
        tags: args.tag.clone(),
        body,
    };
    let path = kb_core::create::create(notebook, &spec, &jiff::Zoned::now())?;
    writeln!(out, "{}", path.display())?;

    // Supplying content means the note is already written; opening an editor
    // would just get in the way of a script.
    if !args.no_edit && args.content.is_none() {
        out.flush()?;
        launch(&editor(), &path)?;
    }
    Ok(())
}

fn run_open(workspace: &Workspace, args: &OpenArgs) -> Result<()> {
    let notes = search::filter_notes(workspace, &args.filters.to_query()?)?;
    if notes.is_empty() {
        bail!("no notes match those filters");
    }
    let Some(path) = pick(&notes, args.query.as_deref())? else {
        return Ok(()); // the picker was dismissed
    };

    let viewer = if args.edit {
        editor()
    } else if has_command("glow") {
        // `-p` pages the rendered Markdown, matching the old `nbo` behaviour.
        return launch_with_args("glow", &["-p"], &path);
    } else {
        "less".to_string()
    };
    launch(&viewer, &path)
}

/// Offer the notes to fzf and return whatever was chosen.
fn pick(notes: &[kb_core::Note], query: Option<&str>) -> Result<Option<PathBuf>> {
    if !has_command("fzf") {
        bail!("`kb open` needs fzf on PATH");
    }

    // Each line is `path \t label`; fzf shows the label and we read the path back.
    let mut input = String::new();
    for note in notes {
        input.push_str(&format!(
            "{}\t{}  {}\n",
            note.path.display(),
            output::qualified_path(note),
            note.title
        ));
    }

    let preview =
        if has_command("glow") { "glow -s dark {1}" } else { "cat {1}" };
    let mut command = std::process::Command::new("fzf");
    command.args([
        "--delimiter",
        "\t",
        "--with-nth",
        "2..",
        "--preview",
        preview,
        "--preview-window",
        "right:60%",
    ]);
    if let Some(query) = query {
        command.args(["--query", query]);
    }

    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .context("starting fzf")?;
    child.stdin.take().context("fzf stdin")?.write_all(input.as_bytes())?;
    let output = child.wait_with_output().context("running fzf")?;

    if !output.status.success() {
        return Ok(None); // dismissed with Esc or Ctrl-C
    }
    let selection = String::from_utf8_lossy(&output.stdout);
    Ok(selection.split('\t').next().filter(|s| !s.trim().is_empty()).map(PathBuf::from))
}

fn run_sync(
    workspace: &Workspace,
    args: &SyncArgs,
    style: Style,
    out: &mut impl Write,
) -> Result<()> {
    for notebook in workspace.select(args.notebook.as_deref())? {
        if !git::is_repository(&notebook.root) {
            writeln!(out, "{}", style.dim(&format!("{}: not a git repository", notebook.name)))?;
            continue;
        }

        let outcome = kb_core::sync::sync(notebook, args.message.as_deref(), args.all)?;
        let summary = match (&outcome.message, outcome.pushed) {
            (Some(message), true) => format!("{message} — pushed"),
            (Some(message), false) => format!("{message} — committed (no upstream)"),
            (None, true) => "nothing to commit — pulled and pushed".to_string(),
            (None, false) => "nothing to do".to_string(),
        };
        writeln!(out, "{}  {summary}", style.path(&outcome.notebook))?;
    }
    Ok(())
}

fn editor() -> String {
    std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string())
}

fn launch(program: &str, path: &std::path::Path) -> Result<()> {
    launch_with_args(program, &[], path)
}

/// Run an interactive program against `path`, inheriting the terminal.
fn launch_with_args(program: &str, args: &[&str], path: &std::path::Path) -> Result<()> {
    // $EDITOR may be a command line ("code -w"), so let the shell split it.
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(r#"{program} {} "$1""#, args.join(" ")))
        .arg("sh")
        .arg(path)
        .status()
        .with_context(|| format!("running {program}"))?;
    if !status.success() {
        bail!("{program} exited with {status}");
    }
    Ok(())
}

fn has_command(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join(name).is_file())
    })
}

fn run_migrate(
    workspace: &Workspace,
    args: &MigrateArgs,
    style: Style,
    out: &mut impl Write,
) -> Result<()> {
    let notebooks = workspace.select(args.notebook.as_deref())?;

    // Refuse to touch a dirty tree: the migration's safety net is that its diff
    // can be reviewed on its own, and unrelated edits mixed in would spoil that.
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
    let plans = migrate::plan(workspace, args.notebook.as_deref())?;

    for plan in &plans {
        let keys: Vec<&str> = plan.added.iter().map(|(key, _)| key.as_str()).collect();
        writeln!(
            out,
            "{}  {}",
            style.path(&format!("{}/{}", plan.notebook, plan.rel_path.display())),
            style.dim(&format!("+ {}", keys.join(", ")))
        )?;
        if args.verbose {
            for (key, value) in &plan.added {
                writeln!(out, "      {key}: {value}")?;
            }
        }
    }

    writeln!(out)?;
    writeln!(out, "{} of {total} notes need frontmatter.", plans.len())?;

    if !args.apply {
        writeln!(out, "{}", style.dim("Nothing written — pass --apply to write."))?;
        return Ok(());
    }

    for plan in &plans {
        migrate::apply(plan)?;
    }
    writeln!(out, "Wrote {} note(s).", plans.len())?;
    Ok(())
}

fn run_tags(
    workspace: &Workspace,
    filters: &FilterArgs,
    style: Style,
    out: &mut impl Write,
) -> Result<()> {
    let notes = search::filter_notes(workspace, &filters.to_query()?)?;

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
    // Most frequent first, alphabetical within a tie.
    let mut rows: Vec<(&String, &usize)> = counts.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

    for (tag, count) in rows {
        writeln!(out, "{tag:<width$}  {count}")?;
    }
    if untagged > 0 {
        writeln!(out, "{}", style.dim(&format!("{:<width$}  {}", "(untagged)", untagged)))?;
    }
    Ok(())
}
