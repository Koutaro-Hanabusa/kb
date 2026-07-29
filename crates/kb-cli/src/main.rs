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
        Some(Command::Show(args)) => run::show(&mut ctx, args, ViewMode::Page)?,
        Some(Command::Peek(args)) => run::show(&mut ctx, args, ViewMode::Page)?,
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
        Some(Command::Init(_)) | Some(Command::Pick(_)) => unreachable!("handled above"),

        // No subcommand: a selector shows that item, and nothing lists the
        // current notebook — the same defaults `nb` has.
        None => match &cli.selector {
            Some(selector) => run::show(
                &mut ctx,
                &ShowArgs { selector: Some(selector.clone()), opts: cli.show },
                ViewMode::Page,
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

fn workspace_root(cli: &Cli) -> std::path::PathBuf {
    cli.root.clone().unwrap_or_else(|| {
        std::env::var_os(kb_core::workspace::ROOT_ENV)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".nb")
            })
    })
}
