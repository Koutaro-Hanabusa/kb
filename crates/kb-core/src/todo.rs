//! Todos, pinning, and notebook archiving.
//!
//! A todo is a note whose heading carries a checkbox — `# [ ] task` open,
//! `# [x] task` done — saved as `*.todo.md`. Pinned items are listed in a
//! `.pindex` file, and an archived notebook is marked by an empty `.archived`.
//! All three shapes are `nb`'s, verified by running it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Extension marking a note as a todo.
pub const TODO_EXT: &str = "todo.md";
/// File listing pinned entry names, one per line.
pub const PINDEX_FILE: &str = ".pindex";
/// Marker file making a notebook archived.
pub const ARCHIVED_FILE: &str = ".archived";

pub fn is_todo(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().to_ascii_lowercase().ends_with(TODO_EXT))
}

/// Render a new todo.
pub fn render(task: &str, tags: &[String]) -> String {
    let mut out = format!("# [ ] {task}\n");
    if !tags.is_empty() {
        let tags: Vec<String> =
            tags.iter().map(|tag| format!("#{}", tag.trim_start_matches('#'))).collect();
        out.push_str(&format!("\n## Tags\n\n{}\n", tags.join(" ")));
    }
    out
}

/// Whether the todo's box is checked.
pub fn is_done(raw: &str) -> bool {
    checkbox_line(raw).is_some_and(|(_, done, _)| done)
}

/// The task text, without the checkbox.
pub fn task_of(raw: &str) -> Option<String> {
    checkbox_line(raw).map(|(_, _, task)| task)
}

/// Check or uncheck the todo, returning the new contents.
///
/// Only the checkbox itself is rewritten, so nothing else in the note moves.
pub fn set_done(raw: &str, done: bool) -> Option<String> {
    let (index, was_done, task) = checkbox_line(raw)?;
    if was_done == done {
        return Some(raw.to_string());
    }

    let mark = if done { 'x' } else { ' ' };
    let mut lines: Vec<String> = raw.lines().map(str::to_string).collect();
    lines[index] = format!("# [{mark}] {task}");

    let mut out = lines.join("\n");
    if raw.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

/// Locate the checkbox heading: its line index, state, and text.
fn checkbox_line(raw: &str) -> Option<(usize, bool, String)> {
    raw.lines().enumerate().find_map(|(index, line)| {
        let rest = line.trim_start().strip_prefix("# [")?;
        let (mark, rest) = rest.split_at_checked(1)?;
        let task = rest.strip_prefix(']')?.trim().to_string();
        match mark {
            " " => Some((index, false, task)),
            "x" | "X" => Some((index, true, task)),
            _ => None,
        }
    })
}

// ─────────────────────────── pinning ───────────────────────────

/// Entry names pinned in `dir`, in pin order.
pub fn pinned(dir: &Path) -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(dir.join(PINDEX_FILE)) else {
        return Vec::new();
    };
    raw.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_string).collect()
}

pub fn is_pinned(dir: &Path, name: &str) -> bool {
    pinned(dir).iter().any(|entry| entry == name)
}

pub fn pin(dir: &Path, name: &str) -> Result<()> {
    let mut entries = pinned(dir);
    if entries.iter().any(|entry| entry == name) {
        return Ok(());
    }
    entries.push(name.to_string());
    write_pindex(dir, &entries)
}

pub fn unpin(dir: &Path, name: &str) -> Result<()> {
    let mut entries = pinned(dir);
    entries.retain(|entry| entry != name);
    write_pindex(dir, &entries)
}

fn write_pindex(dir: &Path, entries: &[String]) -> Result<()> {
    let path = dir.join(PINDEX_FILE);
    if entries.is_empty() {
        // An empty pindex is the same as none; leaving one behind would be litter.
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("removing {}", path.display()))?;
        }
        return Ok(());
    }
    let mut body = entries.join("\n");
    body.push('\n');
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))
}

// ─────────────────────────── archiving ───────────────────────────

pub fn is_archived(notebook_root: &Path) -> bool {
    notebook_root.join(ARCHIVED_FILE).exists()
}

pub fn archive(notebook_root: &Path) -> Result<()> {
    let path = notebook_root.join(ARCHIVED_FILE);
    std::fs::write(&path, "").with_context(|| format!("writing {}", path.display()))
}

pub fn unarchive(notebook_root: &Path) -> Result<()> {
    let path = notebook_root.join(ARCHIVED_FILE);
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

/// Order paths with pinned entries first, each group keeping its own order.
pub fn sort_pinned_first(dir: &Path, paths: &mut [PathBuf]) {
    let pinned = pinned(dir);
    let rank = |path: &PathBuf| {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        pinned.iter().position(|entry| *entry == name).unwrap_or(usize::MAX)
    };
    paths.sort_by_key(rank);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_an_open_todo() {
        assert_eq!(render("買い物に行く", &[]), "# [ ] 買い物に行く\n");
    }

    #[test]
    fn renders_tags_below_the_task() {
        assert_eq!(
            render("レビューする", &["work".into()]),
            "# [ ] レビューする\n\n## Tags\n\n#work\n"
        );
    }

    #[test]
    fn reads_and_flips_the_checkbox() {
        let open = "# [ ] タスク\n";
        assert!(!is_done(open));
        assert_eq!(task_of(open).as_deref(), Some("タスク"));

        let done = set_done(open, true).unwrap();
        assert_eq!(done, "# [x] タスク\n");
        assert!(is_done(&done));

        assert_eq!(set_done(&done, false).unwrap(), open);
    }

    #[test]
    fn flipping_leaves_the_rest_of_the_note_alone() {
        let raw = "# [ ] タスク\n\n## Tags\n\n#work\n";
        assert_eq!(set_done(raw, true).unwrap(), "# [x] タスク\n\n## Tags\n\n#work\n");
    }

    #[test]
    fn an_uppercase_mark_counts_as_done() {
        assert!(is_done("# [X] done\n"));
    }

    #[test]
    fn a_plain_note_has_no_checkbox() {
        assert_eq!(set_done("# just a heading\n", true), None);
        assert!(!is_done("# just a heading\n"));
    }

    #[test]
    fn setting_the_state_it_already_has_changes_nothing() {
        let raw = "# [x] done\n";
        assert_eq!(set_done(raw, true).unwrap(), raw);
    }

    #[test]
    fn recognises_todo_paths() {
        assert!(is_todo(Path::new("a/20260729131756.todo.md")));
        assert!(!is_todo(Path::new("a/note.md")));
    }

    #[test]
    fn pinning_round_trips() {
        let dir = std::env::temp_dir().join(format!("kb-pin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        assert!(pinned(&dir).is_empty());
        pin(&dir, "a.md").unwrap();
        pin(&dir, "b.md").unwrap();
        pin(&dir, "a.md").unwrap(); // idempotent
        assert_eq!(pinned(&dir), vec!["a.md", "b.md"]);
        assert!(is_pinned(&dir, "a.md"));

        unpin(&dir, "a.md").unwrap();
        assert_eq!(pinned(&dir), vec!["b.md"]);

        unpin(&dir, "b.md").unwrap();
        assert!(!dir.join(PINDEX_FILE).exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pinned_items_sort_first() {
        let dir = std::env::temp_dir().join(format!("kb-pinsort-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        pin(&dir, "c.md").unwrap();

        let mut paths = vec![dir.join("a.md"), dir.join("b.md"), dir.join("c.md")];
        sort_pinned_first(&dir, &mut paths);
        assert_eq!(paths[0], dir.join("c.md"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn archiving_is_a_marker_file() {
        let dir = std::env::temp_dir().join(format!("kb-archive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        assert!(!is_archived(&dir));
        archive(&dir).unwrap();
        assert!(is_archived(&dir));
        assert_eq!(std::fs::read_to_string(dir.join(ARCHIVED_FILE)).unwrap(), "");

        unarchive(&dir).unwrap();
        assert!(!is_archived(&dir));
        unarchive(&dir).unwrap(); // idempotent

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
