//! One reading of everything that can change outside the app: working tree
//! status, HEAD, and the branch/tag labels. Taken on a background thread by
//! `app/features/repo_watch.rs` and compared with what the UI shows.

use super::{FileStatus, RefLabel, Result, load_ref_labels, load_status};
use git2::Repository;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::time::{Duration, Instant};

pub struct RepoSnapshot {
    /// Staged, unstaged and untracked files, in `load_status` order.
    pub files: Vec<FileStatus>,
    /// Changes when the content behind `files` does while the list stays
    /// the same (a modified file edited again, another hunk staged).
    pub stamp: u64,
    /// Commit HEAD points at; `None` in a repository without commits.
    pub head: Option<String>,
    /// Checked-out branch; `None` on a detached HEAD.
    pub branch: Option<String>,
    pub ref_labels: HashMap<String, Vec<RefLabel>>,
    /// How long the reading took — the caller spaces its readings by it.
    pub took: Duration,
}

/// Reads only: libgit2 status does not write the index and takes no
/// `index.lock`, so a `git` command running next to it is not disturbed.
pub fn repo_snapshot(repo_path: &Path) -> Result<RepoSnapshot> {
    let started = Instant::now();
    let files = load_status(repo_path)?;
    let repo = Repository::discover(repo_path)?;
    let head_ref = repo.head().ok();
    let head = head_ref
        .as_ref()
        .and_then(|head| head.peel_to_commit().ok())
        .map(|commit| commit.id().to_string());
    let branch = head_ref
        .as_ref()
        .filter(|head| head.is_branch())
        .and_then(|head| head.shorthand().map(String::from));
    let stamp = content_stamp(&repo, &files);
    Ok(RepoSnapshot {
        files,
        stamp,
        head,
        branch,
        ref_labels: load_ref_labels(repo_path),
        took: started.elapsed(),
    })
}

/// Size and modification time of every changed file and of the index: one
/// `stat` each, no file is read.
fn content_stamp(repo: &Repository, files: &[FileStatus]) -> u64 {
    let stat = |path: &Path| {
        std::fs::metadata(path)
            .ok()
            .map(|meta| (meta.len(), meta.modified().ok()))
    };
    let mut hasher = DefaultHasher::new();
    // Staging another hunk of a file leaves its status letters alone.
    stat(&repo.path().join("index")).hash(&mut hasher);
    if let Some(workdir) = repo.workdir() {
        for file in files {
            file.path.hash(&mut hasher);
            stat(&workdir.join(&file.path)).hash(&mut hasher);
        }
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    }

    #[test]
    fn snapshot_follows_files_content_head_and_branch() {
        let dir = std::env::temp_dir().join(format!("ghh-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.name", "t"]);
        git(&dir, &["config", "user.email", "t@t"]);
        git(&dir, &["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-qm", "initial"]);

        let clean = repo_snapshot(&dir).unwrap();
        assert!(clean.files.is_empty());
        assert_eq!(clean.branch.as_deref(), Some("main"));
        assert_eq!(clean.head.as_ref().map(String::len), Some(40));
        let again = repo_snapshot(&dir).unwrap();
        assert_eq!(clean.stamp, again.stamp, "nothing changed, same stamp");

        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        let edited = repo_snapshot(&dir).unwrap();
        assert_eq!(edited.files.len(), 1);
        assert_ne!(edited.stamp, clean.stamp);

        // Same status list (a.txt is still "M"), other content.
        std::fs::write(dir.join("a.txt"), "two and more\n").unwrap();
        let edited_more = repo_snapshot(&dir).unwrap();
        assert_eq!(edited_more.files, edited.files);
        assert_ne!(edited_more.stamp, edited.stamp, "content change is seen");

        // Branch switch on the same commit: HEAD stays, labels differ.
        git(&dir, &["checkout", "-q", "-b", "other"]);
        let switched = repo_snapshot(&dir).unwrap();
        assert_eq!(switched.branch.as_deref(), Some("other"));
        assert_eq!(switched.head, clean.head);
        assert_ne!(switched.ref_labels, clean.ref_labels);

        git(&dir, &["commit", "-qam", "second"]);
        let committed = repo_snapshot(&dir).unwrap();
        assert_ne!(committed.head, clean.head, "a commit made outside is seen");
        assert!(committed.files.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
