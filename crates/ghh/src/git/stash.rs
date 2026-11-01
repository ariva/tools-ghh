//! Stash access through the git CLI: list, show (patch + stat), apply/pop/drop.

use anyhow::{Result, anyhow};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct StashEntry {
    /// Reflog selector, e.g. `stash@{0}` — shifts whenever a stash is added or dropped.
    pub name: String,
    /// Stash commit hash — the stable identity.
    pub hash: String,
    pub date: String,
    pub author: String,
    /// Short hash of the commit the stash was made on.
    pub base: String,
    /// e.g. `WIP on main: a1b2c3d subject` or `On main: my message`.
    pub message: String,
}

pub fn list_stashes(repo_path: &Path) -> Result<Vec<StashEntry>> {
    // No --date here: it would turn %gd into a date instead of the index.
    let out = run(repo_path, &["stash", "list", "--format=%gd%x1f%H%x1f%ci%x1f%an%x1f%p%x1f%gs"])?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\x1f');
            Some(StashEntry {
                name: parts.next()?.to_string(),
                hash: parts.next()?.to_string(),
                date: parts.next()?.chars().take(16).collect(),
                author: parts.next()?.to_string(),
                base: parts.next()?.split(' ').next().unwrap_or("").to_string(),
                message: parts.next()?.to_string(),
            })
        })
        .collect())
}

/// Full patch of a stash, untracked files (`stash -u`) included.
pub fn stash_patch(repo_path: &Path, hash: &str) -> Result<String> {
    run(repo_path, &["stash", "show", "-p", "--include-untracked", hash])
}

/// `--stat` lines: one per file, then the "N files changed" summary.
pub fn stash_stat(repo_path: &Path, hash: &str) -> Result<String> {
    run(repo_path, &["stash", "show", "--stat", "--include-untracked", hash])
}

/// Apply the stash to the working tree; `pop` also drops it on success.
pub fn stash_apply(repo_path: &Path, entry: &StashEntry, pop: bool) -> Result<()> {
    ensure_unshifted(repo_path, entry)?;
    run(repo_path, &["stash", if pop { "pop" } else { "apply" }, &entry.name]).map(drop)
}

pub fn stash_drop(repo_path: &Path, entry: &StashEntry) -> Result<()> {
    ensure_unshifted(repo_path, entry)?;
    run(repo_path, &["stash", "drop", &entry.name]).map(drop)
}

/// `stash@{n}` is positional: refuse to act if another process shifted the
/// stack since we listed it, or we would pop/drop the wrong stash.
fn ensure_unshifted(repo_path: &Path, entry: &StashEntry) -> Result<()> {
    let current = run(repo_path, &["rev-parse", "--verify", "--quiet", &entry.name]).unwrap_or_default();
    if current.trim() == entry.hash {
        Ok(())
    } else {
        Err(anyhow!("stash list changed outside the app — press r to reload"))
    }
}

fn run(repo_path: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").arg("-C").arg(repo_path).args(args).output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        return Ok(stdout);
    }
    // A conflicting apply/pop reports CONFLICT on stdout; other failures on stderr.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = stdout
        .lines()
        .find(|l| l.starts_with("CONFLICT"))
        .or_else(|| stderr.lines().find(|l| l.starts_with("error:")))
        .or_else(|| stderr.lines().find(|l| !l.trim().is_empty()))
        .unwrap_or("git command failed");
    Err(anyhow!("{}", reason.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap().status.success();
        assert!(ok, "git {args:?} failed");
    }

    #[test]
    fn list_show_apply_drop_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ghh-stash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        git(&dir, &["config", "user.name", "t"]);
        git(&dir, &["config", "user.email", "t@t"]);
        git(&dir, &["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-qm", "initial"]);
        assert!(list_stashes(&dir).unwrap().is_empty());

        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        std::fs::write(dir.join("new.txt"), "fresh\n").unwrap();
        git(&dir, &["stash", "push", "-u", "-m", "my stash"]);

        let stashes = list_stashes(&dir).unwrap();
        assert_eq!(stashes.len(), 1);
        let entry = &stashes[0];
        assert_eq!(entry.name, "stash@{0}");
        assert!(entry.message.ends_with("my stash"), "{entry:?}");
        assert_eq!((entry.hash.len(), entry.author.as_str()), (40, "t"));
        assert!(!entry.base.is_empty() && entry.date.len() == 16, "{entry:?}");

        let patch = stash_patch(&dir, &entry.hash).unwrap();
        assert!(patch.contains("+two") && patch.contains("new.txt"), "{patch}");
        assert!(stash_stat(&dir, &entry.hash).unwrap().contains("2 files changed"));

        // Apply keeps the stash; a stale entry (shifted stack) is refused.
        stash_apply(&dir, entry, false).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "two\n");
        let stale = StashEntry { hash: "0".repeat(40), ..entry.clone() };
        assert!(stash_drop(&dir, &stale).unwrap_err().to_string().contains("reload"));
        stash_drop(&dir, entry).unwrap();
        assert!(list_stashes(&dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
