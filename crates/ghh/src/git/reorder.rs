//! Reorder commits by driving a real `git rebase -i` with a scripted todo.
//!
//! Unlike edit/squash (pure libgit2 rewrites that keep every tree), a reorder
//! re-applies patches and can conflict. Using git's own rebase means a
//! conflict leaves the standard rebase state behind: the user resolves it
//! with normal tools and we (or plain git) continue or abort.

use crate::git::error::{Error, Result};
use git2::{Repository, RepositoryState};
use std::path::Path;
use std::process::{Command, Output};

#[derive(Debug, PartialEq)]
pub enum RebaseOutcome {
    Done,
    /// Rebase stopped and is still in progress; the text names where.
    Conflict(String),
}

pub fn rebase_in_progress(repo_path: &Path) -> bool {
    Repository::discover(repo_path).is_ok_and(|repo| {
        matches!(
            repo.state(),
            RepositoryState::Rebase
                | RepositoryState::RebaseInteractive
                | RepositoryState::RebaseMerge
        )
    })
}

/// The run of commits `hash` can be reordered within, newest first: HEAD
/// down its first-parent line to (excluding) the first merge commit.
/// A plain `rebase -i` flattens merges, so nothing at or below one may move.
pub fn movable_chain(repo_path: &Path, hash: &str) -> Result<Vec<String>> {
    let repo = Repository::discover(repo_path)?;
    let mut commit = repo.head()?.peel_to_commit()?;
    let mut chain = Vec::new();
    loop {
        if commit.parent_count() > 1 {
            break;
        }
        chain.push(commit.id().to_string());
        match commit.parent(0) {
            Ok(parent) => commit = parent,
            Err(_) => break,
        }
    }
    if !chain.iter().any(|h| h == hash) {
        return Err(Error::NotMovable);
    }
    Ok(chain)
}

/// Rebase everything from `lowest_hash` (the oldest affected commit, in the
/// ORIGINAL order) up to HEAD, picking `todo` (oldest first) instead.
pub fn move_commit(repo_path: &Path, lowest_hash: &str, todo: &[String]) -> Result<RebaseOutcome> {
    if rebase_in_progress(repo_path) {
        return Err(Error::RebaseInProgress);
    }
    let status = git(
        repo_path,
        &["status", "--porcelain", "--untracked-files=no"],
    )
    .output()?;
    if !status.stdout.is_empty() {
        return Err(Error::DirtyWorkTree);
    }

    let repo = Repository::discover(repo_path)?;
    let lowest = repo.revparse_single(lowest_hash)?.peel_to_commit()?;
    let base = lowest.parent_id(0).ok().map(|id| id.to_string());
    let todo_path = repo.path().join("ghh-move-todo");
    let todo_text: String = todo.iter().map(|h| format!("pick {h}\n")).collect();
    std::fs::write(&todo_path, todo_text)?;

    // git runs `sh -c '<editor> "$@"' <editor> <todo file>`: cp our todo over git's.
    let output = git(
        repo_path,
        &["rebase", "-i", "--committer-date-is-author-date"],
    )
    .arg(base.as_deref().unwrap_or("--root"))
    .env("GIT_SEQUENCE_EDITOR", "cp \"$GHH_TODO_FILE\"")
    .env("GHH_TODO_FILE", &todo_path)
    .output();
    let _ = std::fs::remove_file(&todo_path);
    outcome(repo_path, output?)
}

pub fn rebase_continue(repo_path: &Path) -> Result<RebaseOutcome> {
    let output = git(repo_path, &["rebase", "--continue"]).output()?;
    outcome(repo_path, output)
}

pub fn rebase_abort(repo_path: &Path) -> Result<()> {
    let output = git(repo_path, &["rebase", "--abort"]).output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::cli(&output.stderr))
    }
}

fn git(repo_path: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    // GIT_EDITOR=true: never block on a commit-message editor behind the TUI.
    command
        .arg("-C")
        .arg(repo_path)
        .args(args)
        .env("GIT_EDITOR", "true");
    command
}

fn outcome(repo_path: &Path, output: Output) -> Result<RebaseOutcome> {
    if output.status.success() {
        return Ok(RebaseOutcome::Done);
    }
    if !rebase_in_progress(repo_path) {
        return Err(Error::cli(&output.stderr));
    }
    let stopped_at = Repository::discover(repo_path)
        .ok()
        .and_then(|repo| {
            let commit = repo
                .revparse_single("REBASE_HEAD")
                .ok()?
                .peel_to_commit()
                .ok()?;
            let hash = commit.id().to_string();
            Some(format!("{} {}", &hash[..7], commit.summary().unwrap_or("")))
        })
        .unwrap_or_else(|| "unknown commit".into());
    Ok(RebaseOutcome::Conflict(stopped_at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Temp repo with one commit per (file, content) pair; returns hashes oldest first.
    fn repo_with(name: &str, commits: &[(&str, &str)]) -> (PathBuf, Vec<String>) {
        let dir = std::env::temp_dir().join(format!("ghh-reorder-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = Repository::init(&dir).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "t").unwrap();
        config.set_str("user.email", "t@t").unwrap();
        config.set_bool("commit.gpgsign", false).unwrap();
        let sig = git2::Signature::new("t", "t@t", &git2::Time::new(1_000_000, 0)).unwrap();
        let mut hashes = Vec::new();
        for (i, (file, content)) in commits.iter().enumerate() {
            std::fs::write(dir.join(file), content).unwrap();
            let mut index = repo.index().unwrap();
            index.add_path(Path::new(file)).unwrap();
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let parents: Vec<git2::Commit> = repo
                .head()
                .ok()
                .map(|h| h.peel_to_commit().unwrap())
                .into_iter()
                .collect();
            let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
            let oid = repo
                .commit(
                    Some("HEAD"),
                    &sig,
                    &sig,
                    &format!("c{i}"),
                    &tree,
                    &parent_refs,
                )
                .unwrap();
            hashes.push(oid.to_string());
        }
        (dir, hashes)
    }

    fn subjects(dir: &Path) -> String {
        let out = git(dir, &["log", "--format=%s"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn move_reorders_independent_commits() {
        let (dir, h) = repo_with("ok", &[("a", "1"), ("b", "1"), ("c", "1"), ("d", "1")]);
        assert_eq!(movable_chain(&dir, &h[1]).unwrap().len(), 4);
        // Move c1 above c2: rebase from c1 with todo c2, c1, c3.
        let todo = vec![h[2].clone(), h[1].clone(), h[3].clone()];
        assert_eq!(
            move_commit(&dir, &h[1], &todo).unwrap(),
            RebaseOutcome::Done
        );
        assert_eq!(subjects(&dir), "c3 c1 c2 c0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conflict_stays_in_progress_and_abort_restores() {
        let (dir, h) = repo_with("conflict", &[("f", "0"), ("f", "1"), ("f", "2")]);
        let todo = vec![h[2].clone(), h[1].clone()];
        let result = move_commit(&dir, &h[1], &todo).unwrap();
        assert!(
            matches!(result, RebaseOutcome::Conflict(ref at) if at.ends_with("c2")),
            "{result:?}"
        );
        assert!(rebase_in_progress(&dir));
        // Unresolved: continue reports the conflict again instead of failing hard.
        assert!(matches!(
            rebase_continue(&dir).unwrap(),
            RebaseOutcome::Conflict(_)
        ));
        rebase_abort(&dir).unwrap();
        assert!(!rebase_in_progress(&dir));
        assert_eq!(subjects(&dir), "c2 c1 c0");
        let head = Repository::open(&dir)
            .unwrap()
            .head()
            .unwrap()
            .target()
            .unwrap();
        assert_eq!(head.to_string(), h[2]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dirty_worktree_is_refused() {
        let (dir, h) = repo_with("dirty", &[("a", "1"), ("b", "1")]);
        std::fs::write(dir.join("a"), "changed").unwrap();
        let err = move_commit(&dir, &h[0], &[h[1].clone(), h[0].clone()]).unwrap_err();
        assert!(err.to_string().contains("uncommitted"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
