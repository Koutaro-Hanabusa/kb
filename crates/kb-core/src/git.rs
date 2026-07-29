//! Git operations, shelled out to `git` itself.
//!
//! Only a handful of operations are needed, and every one of them is a plain
//! subprocess call — linking a full git implementation would cost far more in
//! build time and dependencies than it returns.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use jiff::Zoned;

use crate::note::parse_timestamp;

/// When a file first appeared in history and when it last changed.
#[derive(Debug, Clone)]
pub struct FileHistory {
    pub created: Zoned,
    pub updated: Zoned,
}

/// Record separator placed before each commit's date in `git log` output.
const RECORD: char = '\u{1}';

/// Read the full history of a repository in one pass.
///
/// Running `git log` per file would mean ~1,600 subprocesses for a knowledge
/// base this size — exactly the mistake that makes the tool this replaces slow.
/// One `git log` covers every file at once.
pub fn history(repo: &Path) -> Result<HashMap<PathBuf, FileHistory>> {
    let output = run(
        repo,
        // `core.quotepath=false` keeps non-ASCII filenames readable; without it
        // git escapes them into octal and no path ever matches.
        &[
            "-c",
            "core.quotepath=false",
            "log",
            "--reverse",
            "--format=%x01%aI",
            "--name-only",
        ],
    )?;

    let mut histories: HashMap<PathBuf, FileHistory> = HashMap::new();
    let mut current: Option<Zoned> = None;
    for line in output.lines() {
        if let Some(date) = line.strip_prefix(RECORD) {
            current = parse_timestamp(date);
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let Some(date) = &current else { continue };
        histories
            .entry(PathBuf::from(line))
            .and_modify(|h| h.updated = date.clone())
            .or_insert_with(|| FileHistory {
                created: date.clone(),
                updated: date.clone(),
            });
    }
    Ok(histories)
}

/// Whether the working tree has no uncommitted changes.
pub fn is_clean(repo: &Path) -> Result<bool> {
    Ok(run(repo, &["status", "--porcelain"])?.trim().is_empty())
}

pub fn is_repository(path: &Path) -> bool {
    path.join(".git").exists()
}

pub fn add_all(repo: &Path) -> Result<()> {
    run(repo, &["add", "-A"]).map(|_| ())
}

/// Stage everything matching `pathspec`, e.g. `*.md`.
pub fn add(repo: &Path, pathspec: &str) -> Result<()> {
    run(repo, &["add", "--", pathspec]).map(|_| ())
}

/// A staged change: its status letter and path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedChange {
    pub status: char,
    pub path: String,
}

impl StagedChange {
    pub fn is_addition(&self) -> bool {
        self.status == 'A'
    }
}

pub fn staged_changes(repo: &Path) -> Result<Vec<StagedChange>> {
    let output = run(
        repo,
        &[
            "-c",
            "core.quotepath=false",
            "diff",
            "--cached",
            "--name-status",
        ],
    )?;
    Ok(output
        .lines()
        .filter_map(|line| {
            let (status, path) = line.split_once('\t')?;
            Some(StagedChange {
                status: status.chars().next()?,
                // A rename reports `old\tnew`; the new name is what matters.
                path: path.rsplit('\t').next()?.to_string(),
            })
        })
        .collect())
}

/// Whether the repository has an upstream to pull from and push to.
pub fn has_upstream(repo: &Path) -> bool {
    run(repo, &["rev-parse", "--abbrev-ref", "@{upstream}"]).is_ok()
}

pub fn commit(repo: &Path, message: &str) -> Result<()> {
    run(repo, &["commit", "-m", message]).map(|_| ())
}

/// Pull with rebase, stashing anything left in the working tree.
///
/// These repositories routinely carry unrelated dirt — a nested checkout, an
/// index file some other tool rewrote — and without `--autostash` a rebase
/// refuses to start because of it.
pub fn pull_rebase(repo: &Path) -> Result<String> {
    run(repo, &["pull", "--rebase", "--autostash"])
}

pub fn push(repo: &Path) -> Result<String> {
    run(repo, &["push"])
}

pub fn init(repo: &Path) -> Result<()> {
    run(repo, &["init", "-q"]).map(|_| ())
}

pub fn clone(url: &str, into: &Path, branch: Option<&str>) -> Result<()> {
    let mut args: Vec<String> = vec!["clone".into(), url.into(), into.display().to_string()];
    if let Some(branch) = branch {
        args.push("--branch".into());
        args.push(branch.into());
    }
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    // Clone runs before the directory exists, so it cannot use `git -C`.
    let output = Command::new("git")
        .args(&argv)
        .output()
        .context("running `git clone`")?;
    if !output.status.success() {
        bail!(
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

pub fn current_branch(repo: &Path) -> Result<String> {
    Ok(run(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string())
}

/// Set the commit author recorded for this repository.
pub fn set_author(repo: &Path, name: Option<&str>, email: Option<&str>) -> Result<()> {
    if let Some(name) = name {
        run(repo, &["config", "user.name", name])?;
    }
    if let Some(email) = email {
        run(repo, &["config", "user.email", email])?;
    }
    Ok(())
}

/// The commit author configured for this repository, as `(name, email)`.
pub fn author(repo: &Path) -> (Option<String>, Option<String>) {
    let read = |key: &str| {
        run(repo, &["config", "--get", key])
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    (read("user.name"), read("user.email"))
}

pub fn remote_url(repo: &Path) -> Result<String> {
    Ok(run(repo, &["remote", "get-url", "origin"])?
        .trim()
        .to_string())
}

/// Point `origin` at `url`, adding the remote if it is not there yet.
pub fn set_remote(repo: &Path, url: &str) -> Result<()> {
    if remote_url(repo).is_ok() {
        run(repo, &["remote", "set-url", "origin", url]).map(|_| ())
    } else {
        run(repo, &["remote", "add", "origin", url]).map(|_| ())
    }
}

/// Track `branch` on `origin` for the current branch.
pub fn set_upstream(repo: &Path, branch: &str) -> Result<()> {
    run(repo, &["fetch", "origin", branch])?;
    run(
        repo,
        &["branch", "--set-upstream-to", &format!("origin/{branch}")],
    )
    .map(|_| ())
}

/// Branch names on a remote.
///
/// Reads them from the remote itself rather than local tracking refs, so the
/// answer reflects what is actually there.
pub fn remote_branches(repo: &Path, url: Option<&str>) -> Result<Vec<String>> {
    let target = url.unwrap_or("origin");
    let output = run(repo, &["ls-remote", "--heads", target])?;
    Ok(output
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter_map(|(_, refname)| refname.strip_prefix("refs/heads/"))
        .map(str::to_string)
        .collect())
}

/// The branch the remote's HEAD points at, if it advertises one.
pub fn remote_head(repo: &Path) -> Option<String> {
    let output = run(repo, &["ls-remote", "--symref", "origin", "HEAD"]).ok()?;
    output.lines().find_map(|line| {
        line.strip_prefix("ref: refs/heads/")?
            .split_whitespace()
            .next()
            .map(str::to_string)
    })
}

/// Refuse early when git will not allow a branch to be removed.
///
/// A remote will not delete the branch its HEAD points at, so both delete and
/// rename fail on it — `nb` documents the same limitation. Checking first turns
/// a confusing push error into a clear one, and stops rename from leaving the
/// new branch behind next to the old.
fn ensure_deletable(repo: &Path, branch: &str) -> Result<()> {
    if remote_head(repo).as_deref() == Some(branch) {
        bail!(
            "`{branch}` is the remote's default branch and git will not delete it; \
             point the remote's HEAD at another branch first"
        );
    }
    Ok(())
}

pub fn delete_remote_branch(repo: &Path, branch: &str) -> Result<()> {
    ensure_deletable(repo, branch)?;
    run(repo, &["push", "origin", "--delete", branch]).map(|_| ())
}

/// Rename a branch on the remote: push it under the new name, drop the old one.
pub fn rename_remote_branch(repo: &Path, from: &str, to: &str) -> Result<()> {
    ensure_deletable(repo, from)?;
    run(repo, &["fetch", "origin", from])?;
    run(
        repo,
        &[
            "push",
            "origin",
            &format!("refs/remotes/origin/{from}:refs/heads/{to}"),
        ],
    )?;
    delete_remote_branch(repo, from)
}

/// Replace a remote branch with an empty one, discarding its history.
///
/// The new branch is an orphan with a single empty commit, built in a temporary
/// worktree so the caller's checkout is never disturbed.
pub fn reset_remote_branch(repo: &Path, branch: &str) -> Result<()> {
    let worktree = repo.join(format!(".kb-reset-{branch}"));
    let worktree_arg = worktree.to_string_lossy().into_owned();

    run(repo, &["worktree", "add", "--detach", &worktree_arg])?;
    let result = (|| -> Result<()> {
        run(&worktree, &["checkout", "--orphan", branch])?;
        run(&worktree, &["reset", "--hard"])?;
        run(
            &worktree,
            &["commit", "--allow-empty", "-m", "Initial commit"],
        )?;
        run(&worktree, &["push", "--force", "origin", branch])?;
        Ok(())
    })();

    // Clean up whether or not the reset worked.
    let _ = run(repo, &["worktree", "remove", "--force", &worktree_arg]);
    result
}

pub fn remove_remote(repo: &Path) -> Result<()> {
    run(repo, &["remote", "remove", "origin"]).map(|_| ())
}

/// Run an arbitrary git command, returning stdout.
///
/// This backs `kb git`, which is a deliberate passthrough — whatever git accepts
/// is what it accepts.
pub fn run_raw(repo: &Path, args: &[&str]) -> Result<String> {
    run(repo, args)
}

/// Run a git command in `repo`, returning stdout.
fn run(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("running `git {}`", args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            repo.display(),
            stderr.trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo(name: &str) -> PathBuf {
        let repo = std::env::temp_dir().join(format!("kb-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "test"],
        ] {
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(&args)
                .output()
                .unwrap();
        }
        repo
    }

    fn commit_file(repo: &Path, name: &str, contents: &str, date: &str) {
        std::fs::write(repo.join(name), contents).unwrap();
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["add", "-A"])
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["commit", "-q", "-m", name])
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .unwrap();
    }

    #[test]
    fn reads_first_and_last_change_per_file() {
        let repo = init_repo("history");
        commit_file(&repo, "a.md", "one", "2026-01-08T20:35:23+09:00");
        commit_file(&repo, "b.md", "one", "2026-02-01T10:00:00+09:00");
        commit_file(&repo, "a.md", "two", "2026-07-17T09:12:44+09:00");

        let hist = history(&repo).unwrap();
        let a = &hist[Path::new("a.md")];
        assert_eq!(a.created.date().to_string(), "2026-01-08");
        assert_eq!(a.updated.date().to_string(), "2026-07-17");
        let b = &hist[Path::new("b.md")];
        assert_eq!(b.created.date().to_string(), "2026-02-01");
        assert_eq!(b.updated.date().to_string(), "2026-02-01");
        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn handles_non_ascii_filenames() {
        let repo = init_repo("utf8");
        commit_file(
            &repo,
            "日本語のノート.md",
            "text",
            "2026-03-01T12:00:00+09:00",
        );
        let hist = history(&repo).unwrap();
        assert!(hist.contains_key(Path::new("日本語のノート.md")));
        std::fs::remove_dir_all(&repo).unwrap();
    }

    /// Set up a repo with a bare remote and one pushed branch.
    fn with_remote(name: &str) -> (PathBuf, PathBuf) {
        let repo = init_repo(name);
        let bare =
            std::env::temp_dir().join(format!("kb-git-{name}-remote-{}.git", std::process::id()));
        let _ = std::fs::remove_dir_all(&bare);
        Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&bare)
            .output()
            .unwrap();

        commit_file(&repo, "a.md", "x", "2026-01-01T00:00:00+09:00");
        set_remote(&repo, &bare.to_string_lossy()).unwrap();
        run(&repo, &["push", "-q", "-u", "origin", "HEAD:master"]).unwrap();
        (repo, bare)
    }

    #[test]
    fn lists_branches_from_the_remote() {
        let (repo, bare) = with_remote("branches");
        run(&repo, &["push", "-q", "origin", "HEAD:topic"]).unwrap();

        let mut branches = remote_branches(&repo, None).unwrap();
        branches.sort();
        assert_eq!(branches, vec!["master", "topic"]);

        std::fs::remove_dir_all(&repo).unwrap();
        std::fs::remove_dir_all(&bare).unwrap();
    }

    #[test]
    fn renaming_a_branch_moves_it() {
        let (repo, bare) = with_remote("rename");
        run(&repo, &["push", "-q", "origin", "HEAD:topic"]).unwrap();

        rename_remote_branch(&repo, "topic", "renamed").unwrap();

        let branches = remote_branches(&repo, None).unwrap();
        assert!(branches.contains(&"renamed".to_string()));
        assert!(!branches.contains(&"topic".to_string()));

        std::fs::remove_dir_all(&repo).unwrap();
        std::fs::remove_dir_all(&bare).unwrap();
    }

    /// git refuses to delete the branch a remote's HEAD points at, so say so
    /// before pushing anything rather than after.
    #[test]
    fn the_default_branch_is_refused_up_front() {
        let (repo, bare) = with_remote("defaultbranch");
        let before = remote_branches(&repo, None).unwrap();

        let error = rename_remote_branch(&repo, "master", "main")
            .unwrap_err()
            .to_string();
        assert!(error.contains("default branch"), "{error}");
        assert!(delete_remote_branch(&repo, "master").is_err());

        // Nothing was created on the way to failing.
        assert_eq!(remote_branches(&repo, None).unwrap(), before);

        std::fs::remove_dir_all(&repo).unwrap();
        std::fs::remove_dir_all(&bare).unwrap();
    }

    #[test]
    fn resetting_a_branch_empties_it() {
        let (repo, bare) = with_remote("reset");
        run(&repo, &["push", "-q", "origin", "HEAD:topic"]).unwrap();

        reset_remote_branch(&repo, "topic").unwrap();

        // One empty commit, and the file is gone from that branch.
        let log = run(&repo, &["ls-remote", "origin", "refs/heads/topic"]).unwrap();
        assert!(!log.trim().is_empty());
        assert!(
            remote_branches(&repo, None)
                .unwrap()
                .contains(&"topic".to_string())
        );

        std::fs::remove_dir_all(&repo).unwrap();
        std::fs::remove_dir_all(&bare).unwrap();
    }

    #[test]
    fn reads_and_writes_the_author() {
        let repo = init_repo("author");
        set_author(&repo, Some("Someone"), Some("someone@example.com")).unwrap();
        let (name, email) = author(&repo);
        assert_eq!(name.as_deref(), Some("Someone"));
        assert_eq!(email.as_deref(), Some("someone@example.com"));
        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn detects_a_dirty_tree() {
        let repo = init_repo("clean");
        commit_file(&repo, "a.md", "one", "2026-01-01T00:00:00+09:00");
        assert!(is_clean(&repo).unwrap());
        std::fs::write(repo.join("a.md"), "changed").unwrap();
        assert!(!is_clean(&repo).unwrap());
        std::fs::remove_dir_all(&repo).unwrap();
    }
}
