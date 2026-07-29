//! Handing control to other programs: editors, pagers, fzf.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use kb_core::Note;

use crate::output;

/// The editor to use: an explicit override, then `$VISUAL`, `$EDITOR`, `vi`.
pub fn editor(override_with: Option<&str>) -> String {
    override_with
        .map(str::to_string)
        .or_else(|| std::env::var("VISUAL").ok())
        .or_else(|| std::env::var("EDITOR").ok())
        .unwrap_or_else(|| "vi".to_string())
}

/// Display a file, rendering Markdown when a renderer is available.
pub fn page(path: &Path) -> Result<()> {
    if is_markdown(path) && has_command("glow") {
        return launch_with_args("glow", &["-p"], path);
    }
    if has_command("bat") {
        return launch_with_args("bat", &["--style=plain"], path);
    }
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());
    launch(&pager, path)
}

/// Hand a file to the system's preferred application.
pub fn open_externally(path: &Path) -> Result<()> {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    if !has_command(opener) {
        // Without a desktop opener, showing the file beats doing nothing.
        return page(path);
    }
    launch(opener, path)
}

pub fn launch(program: &str, path: &Path) -> Result<()> {
    launch_with_args(program, &[], path)
}

/// Run an interactive program against `path`, inheriting the terminal.
pub fn launch_with_args(program: &str, args: &[&str], path: &Path) -> Result<()> {
    // $EDITOR may be a command line ("code -w"), so let the shell split it.
    let status = Command::new("sh")
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

/// Ask for confirmation on stderr, so piped stdout stays clean.
pub fn confirm(question: &str) -> Result<bool> {
    eprint!("{question} [y/N] ");
    std::io::stderr().flush()?;

    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// Offer notes to fzf and return the chosen path.
pub fn pick(notes: &[Note], query: Option<&str>) -> Result<Option<PathBuf>> {
    if !has_command("fzf") {
        bail!("`kb pick` needs fzf on PATH");
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

    let preview = if has_command("glow") { "glow -s dark {1}" } else { "cat {1}" };
    let mut command = Command::new("fzf");
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
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
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

pub fn has_command(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join(name).is_file())
    })
}

fn is_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|ext| {
        ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
    })
}
