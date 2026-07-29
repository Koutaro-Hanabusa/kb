//! `kb` — a fast Markdown knowledge base, compatible with `nb`'s command surface.

mod commands;
mod output;
mod run;
mod shell;
mod since;

use std::io::Write;

use anyhow::Result;
use clap::Parser;
use kb_core::Workspace;

use commands::{Cli, Command, FilterArgs, ListArgs, ShowArgs};
use output::Style;
use run::{Ctx, ViewMode};

fn main() -> Result<()> {
    restore_sigpipe();

    // A word that is not a built-in subcommand may name a plugin. This has to
    // be settled before clap parses, because the bare `kb <selector>` form
    // would otherwise swallow it.
    if let Some(result) = dispatch_plugin() {
        return result;
    }
    let cli = Cli::parse();

    // `kb init` runs before a knowledge base exists, so it cannot open one.
    if let Some(Command::Init(args)) = &cli.command {
        let root = workspace_root(&cli);
        let stdout = std::io::stdout();
        let mut out = std::io::BufWriter::new(stdout.lock());
        run::init(&root, args, &mut out)?;
        return out.flush().map_err(Into::into);
    }

    let workspace = match &cli.root {
        Some(root) => Workspace::open(root)?,
        None => Workspace::discover()?,
    };

    // fzf and interactive viewers need the real terminal, so these run outside
    // the buffered handle.
    if let Some(Command::Pick(args)) = &cli.command {
        return run::pick(&workspace, args);
    }

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let mut ctx = Ctx { workspace: &workspace, style: Style::detect(), out: &mut out };

    match &cli.command {
        Some(Command::Add(args)) => run::add(&mut ctx, args)?,
        Some(Command::List(args)) => run::list(&mut ctx, args)?,
        Some(Command::Search(args)) => run::search_notes(&mut ctx, args)?,
        Some(Command::Show(args)) => run::show(&mut ctx, args, ViewMode::Show)?,
        Some(Command::Peek(args)) => run::show(&mut ctx, args, ViewMode::Peek)?,
        Some(Command::Open(args)) => run::show(&mut ctx, args, ViewMode::Open)?,
        Some(Command::Edit(args)) => run::edit(&mut ctx, args)?,
        Some(Command::Delete(args)) => run::delete(&mut ctx, args)?,
        Some(Command::Move(args)) => run::move_item(&mut ctx, args)?,
        Some(Command::Copy(args)) => run::copy_item(&mut ctx, args)?,
        Some(Command::Count(args)) => run::count(&mut ctx, args)?,
        Some(Command::Notebooks(args)) => run::notebooks(&mut ctx, args)?,
        Some(Command::Use(args)) => run::use_notebook(&mut ctx, args)?,
        Some(Command::Status(args)) => run::status(&mut ctx, args)?,
        Some(Command::Git(args)) => run::git_passthrough(&mut ctx, args)?,
        Some(Command::History(args)) => run::history(&mut ctx, args)?,
        Some(Command::Folders(args)) => run::folders(&mut ctx, args)?,
        Some(Command::Sync(args)) => run::sync(&mut ctx, args)?,
        Some(Command::Reconcile(args)) => run::reconcile(&mut ctx, args)?,
        Some(Command::Tags(args)) => run::tags(&mut ctx, args)?,
        Some(Command::Migrate(args)) => run::migrate_notes(&mut ctx, args)?,
        Some(Command::Bookmark(args)) => run::bookmark(&mut ctx, args)?,
        Some(Command::Todo(args)) => run::todo(&mut ctx, args)?,
        Some(Command::Do(args)) => run::todo(&mut ctx, &todo_shortcut(args, true))?,
        Some(Command::Undo(args)) => run::todo(&mut ctx, &todo_shortcut(args, false))?,
        Some(Command::Browse(args)) => run::browse(&mut ctx, args)?,
        Some(Command::Settings(args)) => run::settings(&mut ctx, args)?,
        Some(Command::Set(args)) => run::set_setting(&mut ctx, args)?,
        Some(Command::Unset(args)) => run::unset_setting(&mut ctx, args)?,
        Some(Command::Remote(args)) => run::remote(&mut ctx, args)?,
        Some(Command::Run(args)) => run::run_in_notebook(&mut ctx, args)?,
        Some(Command::Shell(args)) => run::interactive_shell(&mut ctx, args)?,
        Some(Command::Import(args)) => run::import(&mut ctx, args)?,
        Some(Command::Export(args)) => run::export(&mut ctx, args)?,
        Some(Command::Env(args)) => run::env(&mut ctx, args)?,
        Some(Command::Plugins(args)) => run::plugins(&mut ctx, args)?,
        Some(Command::Subcommands) => {
            use clap::CommandFactory;
            for sub in Cli::command().get_subcommands() {
                writeln!(ctx.out, "{}", sub.get_name())?;
            }
        }
        Some(Command::Update) => {
            writeln!(ctx.out, "kb {}", env!("CARGO_PKG_VERSION"))?;
            writeln!(
                ctx.out,
                "kb is managed by Nix; update it with `nix flake update kb` in your dotfiles."
            )?;
        }
        Some(Command::Completions(args)) => {
            use clap::CommandFactory;
            clap_complete::generate(args.shell, &mut Cli::command(), "kb", &mut ctx.out);
        }
        Some(Command::Pin(args)) => run::pin(&mut ctx, args, true)?,
        Some(Command::Unpin(args)) => run::pin(&mut ctx, args, false)?,
        Some(Command::Archive(args)) => run::archive(&mut ctx, args, true)?,
        Some(Command::Unarchive(args)) => run::archive(&mut ctx, args, false)?,
        Some(Command::Init(_)) | Some(Command::Pick(_)) => unreachable!("handled above"),

        // No subcommand: a selector shows that item, and nothing lists the
        // current notebook — the same defaults `nb` has.
        None => match &cli.selector {
            Some(selector) => run::show(
                &mut ctx,
                &ShowArgs { selector: Some(selector.clone()), opts: cli.show },
                ViewMode::Show,
            )?,
            None => run::list(&mut ctx, &ListArgs {
                selector: None,
                filters: FilterArgs::default(),
                paths_only: false,
                json: false,
            })?,
        },
    }

    out.flush()?;
    Ok(())
}

/// Run a plugin if the first non-option argument names one.
///
/// Returns `None` when the argument is a built-in or no such plugin exists, so
/// the caller falls through to normal parsing.
fn dispatch_plugin() -> Option<Result<()>> {
    let mut args = std::env::args().skip(1).peekable();
    let mut root: Option<std::path::PathBuf> = None;

    // Step over global options to reach the first word.
    while let Some(arg) = args.peek() {
        if arg == "--root" {
            args.next();
            root = args.next().map(std::path::PathBuf::from);
            continue;
        }
        if let Some(value) = arg.strip_prefix("--root=") {
            root = Some(std::path::PathBuf::from(value));
            args.next();
            continue;
        }
        if arg.starts_with('-') {
            args.next();
            continue;
        }
        break;
    }

    let name = args.next()?;
    if is_builtin(&name) {
        return None;
    }

    let workspace = match root {
        Some(root) => Workspace::open(root),
        None => Workspace::discover(),
    }
    .ok()?;
    let plugin = kb_core::plugins::find(&workspace.root, &name)?;

    Some((|| {
        let rest: Vec<String> = args.collect();
        let notebook = workspace.default_notebook()?;
        let code = kb_core::plugins::execute(&plugin, &notebook.root, &rest)?;
        std::process::exit(code);
    })())
}

/// Whether `name` is a subcommand or alias `kb` already provides.
fn is_builtin(name: &str) -> bool {
    use clap::CommandFactory;
    Cli::command().get_subcommands().any(|sub| {
        sub.get_name() == name || sub.get_all_aliases().any(|alias| alias == name)
    })
}

/// Die quietly when a pipe closes, the way every other CLI does.
///
/// Rust ignores SIGPIPE so that writes fail with EPIPE instead, which turns
/// `kb ls | head` into an error — or a panic, from code that unwraps the write.
fn restore_sigpipe() {
    // SAFETY: setting a signal disposition to the default is always valid, and
    // this runs before any other thread exists.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// `kb do <id>` is shorthand for `kb todo do <id>`.
fn todo_shortcut(args: &commands::SelectorArgs, done: bool) -> commands::TodoArgs {
    let target = commands::SelectorArgs { selector: args.selector.clone() };
    commands::TodoArgs {
        command: Some(if done {
            commands::TodoCommand::Do(target)
        } else {
            commands::TodoCommand::Undo(target)
        }),
        filters: FilterArgs::default(),
        all: false,
    }
}

fn workspace_root(cli: &Cli) -> std::path::PathBuf {
    cli.root.clone().unwrap_or_else(|| {
        std::env::var_os(kb_core::workspace::ROOT_ENV)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".nb")
            })
    })
}
