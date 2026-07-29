//! Creating, deleting, moving, and copying items, keeping `.index` in step.
//!
//! Every mutation here has to touch two things: the file on disk and the index
//! entry that gives it an id. Doing one without the other is how ids start
//! pointing at the wrong note.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::index::Index;
use crate::note::filename_stem;
use crate::workspace::Notebook;

/// Remove an item and retire its id.
pub fn delete(path: &Path) -> Result<()> {
    let dir = parent_of(path)?;
    let name = file_name(path)?;

    if path.is_dir() {
        std::fs::remove_dir_all(path)
            .with_context(|| format!("removing {}", path.display()))?;
    } else {
        std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    }

    let mut index = Index::load(dir)?;
    if index.remove(&name).is_some() {
        index.save(dir)?;
    }
    Ok(())
}

/// Move or rename an item.
///
/// Within one directory the id follows the file, because it is the same item
/// under a new name. Across directories the old id is retired and a new one
/// issued, since ids are only unique within their own index.
pub fn rename(from: &Path, to: &Path) -> Result<PathBuf> {
    let from_dir = parent_of(from)?;
    let from_name = file_name(from)?;
    let to = resolve_destination(from, to)?;
    let to_dir = parent_of(&to)?;
    let to_name = file_name(&to)?;

    if to.exists() {
        bail!("already exists: {}", to.display());
    }
    std::fs::create_dir_all(to_dir)
        .with_context(|| format!("creating {}", to_dir.display()))?;
    std::fs::rename(from, &to)
        .with_context(|| format!("moving {} to {}", from.display(), to.display()))?;

    if from_dir == to_dir {
        let mut index = Index::load(from_dir)?;
        if index.rename(&from_name, &to_name).is_some() {
            index.save(from_dir)?;
        }
    } else {
        let mut source = Index::load(from_dir)?;
        if source.remove(&from_name).is_some() {
            source.save(from_dir)?;
        }
        let mut target = Index::load(to_dir)?;
        target.add(&to_name);
        target.save(to_dir)?;
    }
    Ok(to)
}

/// Copy an item, giving the copy its own id.
pub fn copy(from: &Path, to: &Path) -> Result<PathBuf> {
    let destination = resolve_destination(from, to)?;
    let to = if destination.exists() { available_beside(&destination) } else { destination };
    let to_dir = parent_of(&to)?;

    std::fs::create_dir_all(to_dir)
        .with_context(|| format!("creating {}", to_dir.display()))?;
    std::fs::copy(from, &to)
        .with_context(|| format!("copying {} to {}", from.display(), to.display()))?;

    let mut index = Index::load(to_dir)?;
    index.add(&file_name(&to)?);
    index.save(to_dir)?;
    Ok(to)
}

/// Rename an item after its own title, as `nb move --to-title` does.
pub fn rename_to_title(path: &Path, title: &str) -> Result<PathBuf> {
    let extension = path.extension().map(|e| e.to_string_lossy().into_owned());
    let stem = filename_stem(title);
    let name = match extension {
        Some(ext) => format!("{stem}.{ext}"),
        None => stem,
    };
    rename(path, &parent_of(path)?.join(name))
}

/// Create a folder and index it.
pub fn create_folder(parent: &Path, name: &str) -> Result<PathBuf> {
    let path = parent.join(name);
    if path.exists() {
        bail!("already exists: {}", path.display());
    }
    std::fs::create_dir_all(&path)
        .with_context(|| format!("creating {}", path.display()))?;

    let mut index = Index::load(parent)?;
    index.add(name);
    index.save(parent)?;
    Ok(path)
}

/// Items directly inside `dir`, excluding dotfiles.
pub fn list_dir(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| !name.to_string_lossy().starts_with('.'))
        })
        .collect();
    paths.sort();
    Ok(paths)
}

/// How many items sit directly inside `dir`.
pub fn count(dir: &Path) -> Result<usize> {
    Ok(list_dir(dir)?.len())
}

/// Interpret a destination that may be a directory or a full path.
fn resolve_destination(from: &Path, to: &Path) -> Result<PathBuf> {
    if to.is_dir() {
        return Ok(to.join(file_name(from)?));
    }
    Ok(to.to_path_buf())
}

/// `name.md` → `name-2.md`, and so on.
fn available_beside(path: &Path) -> PathBuf {
    let dir = path.parent().unwrap_or(Path::new("."));
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2u32..)
        .map(|n| dir.join(format!("{stem}-{n}{ext}")))
        .find(|candidate| !candidate.exists())
        .expect("an unused filename exists")
}

fn parent_of(path: &Path) -> Result<&Path> {
    path.parent().with_context(|| format!("{} has no parent directory", path.display()))
}

fn file_name(path: &Path) -> Result<String> {
    Ok(path
        .file_name()
        .with_context(|| format!("{} has no filename", path.display()))?
        .to_string_lossy()
        .into_owned())
}

/// Reconcile every indexed directory in a notebook with what is on disk.
pub fn reconcile(notebook: &Notebook) -> Result<usize> {
    let mut updated = 0;
    let mut dirs = vec![notebook.root.clone()];
    while let Some(dir) = dirs.pop() {
        let mut index = Index::load(&dir)?;
        if index.reconcile(&dir)? {
            index.save(&dir)?;
            updated += 1;
        }
        for path in list_dir(&dir)? {
            if path.is_dir() {
                dirs.push(path);
            }
        }
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kb-items-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn deleting_retires_the_id_but_leaves_later_ones_alone() {
        let dir = fixture("delete");
        for name in ["a.md", "b.md", "c.md"] {
            write(&dir, name, "x");
        }
        Index::parse("a.md\nb.md\nc.md\n").save(&dir).unwrap();

        delete(&dir.join("b.md")).unwrap();

        let index = Index::load(&dir).unwrap();
        assert_eq!(index.name_of(2), None);
        assert_eq!(index.name_of(3), Some("c.md"));
        assert!(!dir.join("b.md").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn renaming_within_a_directory_keeps_the_id() {
        let dir = fixture("rename");
        write(&dir, "a.md", "x");
        Index::parse("a.md\n").save(&dir).unwrap();

        let moved = rename(&dir.join("a.md"), &dir.join("renamed.md")).unwrap();

        assert_eq!(moved, dir.join("renamed.md"));
        let index = Index::load(&dir).unwrap();
        assert_eq!(index.name_of(1), Some("renamed.md"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn moving_across_directories_reissues_the_id() {
        let dir = fixture("move");
        let sub = dir.join("folder");
        std::fs::create_dir_all(&sub).unwrap();
        write(&dir, "a.md", "x");
        Index::parse("a.md\nkeep.md\n").save(&dir).unwrap();
        Index::parse("existing.md\n").save(&sub).unwrap();

        rename(&dir.join("a.md"), &sub.join("a.md")).unwrap();

        assert_eq!(Index::load(&dir).unwrap().name_of(1), None);
        // Ids are per-directory, so the note lands after what is already there.
        assert_eq!(Index::load(&sub).unwrap().name_of(2), Some("a.md"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn moving_into_a_directory_keeps_the_filename() {
        let dir = fixture("moveinto");
        let sub = dir.join("folder");
        std::fs::create_dir_all(&sub).unwrap();
        write(&dir, "a.md", "x");

        let moved = rename(&dir.join("a.md"), &sub).unwrap();

        assert_eq!(moved, sub.join("a.md"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn moving_onto_an_existing_file_is_refused() {
        let dir = fixture("clobber");
        write(&dir, "a.md", "one");
        write(&dir, "b.md", "two");

        assert!(rename(&dir.join("a.md"), &dir.join("b.md")).is_err());
        assert_eq!(std::fs::read_to_string(dir.join("b.md")).unwrap(), "two");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copying_gives_the_copy_its_own_id() {
        let dir = fixture("copy");
        write(&dir, "a.md", "body");
        Index::parse("a.md\n").save(&dir).unwrap();

        let copied = copy(&dir.join("a.md"), &dir.join("a.md")).unwrap();

        assert_eq!(copied, dir.join("a-2.md"));
        let index = Index::load(&dir).unwrap();
        assert_eq!(index.name_of(1), Some("a.md"));
        assert_eq!(index.name_of(2), Some("a-2.md"));
        assert_eq!(std::fs::read_to_string(&copied).unwrap(), "body");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn renaming_to_the_title_uses_nb_filename_rules() {
        let dir = fixture("totitle");
        write(&dir, "20260108203523.md", "x");

        let moved = rename_to_title(&dir.join("20260108203523.md"), "日本語UIライティング - 句点のルール")
            .unwrap();

        assert_eq!(moved.file_name().unwrap(), "日本語uiライティング_-_句点のルール.md");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn creating_a_folder_indexes_it() {
        let dir = fixture("folder");
        Index::parse("a.md\n").save(&dir).unwrap();

        create_folder(&dir, "notes").unwrap();

        assert!(dir.join("notes").is_dir());
        assert_eq!(Index::load(&dir).unwrap().name_of(2), Some("notes"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn counting_skips_dotfiles() {
        let dir = fixture("count");
        write(&dir, "a.md", "x");
        write(&dir, "b.md", "x");
        Index::parse("a.md\nb.md\n").save(&dir).unwrap();

        assert_eq!(count(&dir).unwrap(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
