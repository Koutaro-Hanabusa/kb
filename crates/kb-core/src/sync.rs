//! Committing and exchanging notes with the remote.

use anyhow::{Result, bail};

use crate::git::{self, StagedChange};
use crate::workspace::Notebook;

fn is_markdown(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

/// What a sync did to one notebook.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub notebook: String,
    /// Paths committed, empty when there was nothing to commit.
    pub committed: Vec<String>,
    pub message: Option<String>,
    pub pulled: bool,
    pub pushed: bool,
}

/// Commit any note changes, then pull and push.
///
/// Only Markdown is staged by default. These repositories hold more than notes —
/// a Worker, CI config, a nested checkout — and sweeping all of it into an
/// automatic commit is how unrelated work gets published by accident.
pub fn sync(
    notebook: &Notebook,
    message: Option<&str>,
    include_everything: bool,
) -> Result<Outcome> {
    let repo = &notebook.root;
    let mut outcome = Outcome {
        notebook: notebook.name.clone(),
        committed: Vec::new(),
        message: None,
        pulled: false,
        pushed: false,
    };

    if include_everything {
        git::add_all(repo)?;
    } else {
        git::add(repo, "*.md")?;
    }

    let staged = git::staged_changes(repo)?;

    // Staging Markdown does not unstage what someone else already staged, and
    // committing now would sweep it in. Stop rather than publish a change the
    // caller never asked for.
    if !include_everything {
        let foreign: Vec<&str> = staged
            .iter()
            .filter(|c| !is_markdown(&c.path))
            .map(|c| c.path.as_str())
            .collect();
        if !foreign.is_empty() {
            bail!(
                "{} has non-Markdown changes already staged ({}); commit or unstage them, or pass --all",
                notebook.name,
                foreign.join(", ")
            );
        }
    }

    if !staged.is_empty() {
        let text = message
            .map(str::to_string)
            .unwrap_or_else(|| describe(&staged));
        git::commit(repo, &text)?;
        outcome.committed = staged.iter().map(|c| c.path.clone()).collect();
        outcome.message = Some(text);
    }

    if git::has_upstream(repo) {
        git::pull_rebase(repo)?;
        outcome.pulled = true;
        git::push(repo)?;
        outcome.pushed = true;
    }
    Ok(outcome)
}

/// Summarise staged changes for a commit subject.
fn describe(staged: &[StagedChange]) -> String {
    let verb = if staged.iter().all(StagedChange::is_addition) {
        "Add"
    } else {
        "Update"
    };
    match staged {
        [only] => format!("{verb} {}", only.path),
        many => format!("{verb} {} notes", many.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(status: char, path: &str) -> StagedChange {
        StagedChange {
            status,
            path: path.to_string(),
        }
    }

    #[test]
    fn names_a_single_file() {
        assert_eq!(
            describe(&[change('A', "knowledge/a.md")]),
            "Add knowledge/a.md"
        );
        assert_eq!(
            describe(&[change('M', "knowledge/a.md")]),
            "Update knowledge/a.md"
        );
    }

    #[test]
    fn counts_a_batch() {
        let staged = [change('A', "a.md"), change('A', "b.md")];
        assert_eq!(describe(&staged), "Add 2 notes");
    }

    #[test]
    fn a_mixed_batch_reads_as_an_update() {
        let staged = [change('A', "a.md"), change('M', "b.md")];
        assert_eq!(describe(&staged), "Update 2 notes");
    }

    #[test]
    fn recognises_markdown_by_extension() {
        assert!(is_markdown("knowledge/a.md"));
        assert!(is_markdown("knowledge/a.MARKDOWN"));
        assert!(!is_markdown("knowledge/.index"));
        assert!(!is_markdown("mcp"));
        assert!(!is_markdown("a.md.bak"));
    }
}
