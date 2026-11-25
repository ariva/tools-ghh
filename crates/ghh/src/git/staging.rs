use crate::git::error::{Error, Result};
use crate::git::models::{FileStatus, StageState};
use git2::Repository;
use std::path::Path;

pub fn load_status(repo_path: &Path) -> Result<Vec<FileStatus>> {
    let repo = Repository::discover(repo_path)?;
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);

    let statuses = repo.statuses(Some(&mut opts))?;
    let mut files: Vec<FileStatus> = Vec::new();

    for entry in statuses.iter() {
        let path = entry.path().unwrap_or("").to_string();
        let s = entry.status();

        if s.intersects(
            git2::Status::INDEX_NEW
                | git2::Status::INDEX_MODIFIED
                | git2::Status::INDEX_DELETED
                | git2::Status::INDEX_RENAMED,
        ) {
            let ch = if s.contains(git2::Status::INDEX_NEW) {
                'A'
            } else if s.contains(git2::Status::INDEX_DELETED) {
                'D'
            } else if s.contains(git2::Status::INDEX_RENAMED) {
                'R'
            } else {
                'M'
            };
            files.push(FileStatus {
                path: path.clone(),
                stage_state: StageState::Staged,
                status_char: ch,
            });
        }

        if s.intersects(
            git2::Status::WT_MODIFIED | git2::Status::WT_DELETED | git2::Status::WT_RENAMED,
        ) {
            let ch = if s.contains(git2::Status::WT_DELETED) {
                'D'
            } else if s.contains(git2::Status::WT_RENAMED) {
                'R'
            } else {
                'M'
            };
            files.push(FileStatus {
                path: path.clone(),
                stage_state: StageState::Unstaged,
                status_char: ch,
            });
        }

        if s.contains(git2::Status::WT_NEW) {
            files.push(FileStatus {
                path,
                stage_state: StageState::Untracked,
                status_char: '?',
            });
        }
    }

    Ok(files)
}

pub fn stage_file(repo_path: &Path, file: &FileStatus) -> Result<()> {
    let repo = Repository::discover(repo_path)?;
    let mut index = repo.index()?;
    if file.stage_state == StageState::Unstaged && file.status_char == 'D' {
        index.remove_path(std::path::Path::new(&file.path))?;
    } else {
        index.add_path(std::path::Path::new(&file.path))?;
    }
    index.write()?;
    Ok(())
}

pub fn unstage_file(repo_path: &Path, file: &FileStatus) -> Result<()> {
    let repo = Repository::discover(repo_path)?;
    match repo.head() {
        Ok(head) => {
            let commit = head.peel_to_commit()?;
            repo.reset_default(
                Some(commit.as_object()),
                std::iter::once(file.path.as_str()),
            )?;
        }
        Err(_) => {
            let mut index = repo.index()?;
            index.remove_path(std::path::Path::new(&file.path))?;
            index.write()?;
        }
    }
    Ok(())
}

pub fn stage_all(repo_path: &Path) -> Result<()> {
    let repo = Repository::discover(repo_path)?;
    let mut index = repo.index()?;
    index.add_all(["."].iter(), git2::IndexAddOption::DEFAULT, None)?;
    index.write()?;
    Ok(())
}

pub fn unstage_all(repo_path: &Path) -> Result<()> {
    let repo = Repository::discover(repo_path)?;

    let staged_paths: Vec<String> = {
        let statuses = repo.statuses(None)?;
        statuses
            .iter()
            .filter(|e| {
                e.status().intersects(
                    git2::Status::INDEX_NEW
                        | git2::Status::INDEX_MODIFIED
                        | git2::Status::INDEX_DELETED
                        | git2::Status::INDEX_RENAMED,
                )
            })
            .filter_map(|e| e.path().map(String::from))
            .collect()
    };

    if staged_paths.is_empty() {
        return Ok(());
    }

    match repo.head() {
        Ok(head) => {
            let commit = head.peel_to_commit()?;
            repo.reset_default(Some(commit.as_object()), staged_paths.iter())?;
        }
        Err(_) => {
            let mut index = repo.index()?;
            index.clear()?;
            index.write()?;
        }
    }

    Ok(())
}

pub fn commit_staged(repo_path: &Path, message: &str) -> Result<()> {
    let repo = Repository::discover(repo_path)?;
    let mut index = repo.index()?;
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;
    let sig = repo.signature()?;

    let parent_commits: Vec<git2::Commit<'_>> = match repo.head() {
        Ok(head) => vec![head.peel_to_commit()?],
        Err(_) => vec![],
    };
    let parent_refs: Vec<&git2::Commit<'_>> = parent_commits.iter().collect();

    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)?;
    Ok(())
}

pub fn get_file_diff(repo_path: &Path, file: &FileStatus) -> Result<String> {
    let repo = Repository::discover(repo_path)?;

    if file.stage_state == StageState::Untracked {
        let workdir = repo.workdir().ok_or(Error::NoWorkdir)?;
        let full_path = workdir.join(&file.path);
        return match std::fs::read_to_string(&full_path) {
            Ok(content) => {
                let mut out = format!("+++ new file: {}\n", file.path);
                for line in content.lines() {
                    out.push('+');
                    out.push_str(line);
                    out.push('\n');
                }
                Ok(out)
            }
            Err(_) => Ok(format!("(binary or unreadable: {})", file.path)),
        };
    }

    let mut diff_opts = git2::DiffOptions::new();
    diff_opts.pathspec(&file.path);

    let diff = if file.stage_state == StageState::Staged {
        let head_tree = repo
            .head()
            .and_then(|h| h.peel_to_commit())
            .and_then(|c| c.tree())
            .ok();
        repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut diff_opts))?
    } else {
        repo.diff_index_to_workdir(None, Some(&mut diff_opts))?
    };

    let mut output = patch_text(&diff)?;

    if output.is_empty() {
        output = format!("(no diff for {})", file.path);
    }

    Ok(output)
}

/// A libgit2 diff as patch text, one line per line (`diff --git` headers
/// included, so `parse_diff_files` finds the files).
fn patch_text(diff: &git2::Diff) -> Result<String> {
    let mut output = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        let origin = line.origin();
        if let Ok(content) = std::str::from_utf8(line.content()) {
            let content = content.trim_end_matches('\n').trim_end_matches('\r');
            if matches!(origin, '+' | '-' | ' ') {
                output.push(origin);
            }
            output.push_str(content);
            output.push('\n');
        }
        true
    })?;
    Ok(output)
}

/// Everything not committed yet as one patch: staged changes, then unstaged,
/// then untracked files as additions.
pub struct WorkingTreePatch {
    pub lines: Vec<String>,
    /// Line index where the unstaged part starts.
    pub unstaged_from: usize,
    /// Line index where the untracked part starts.
    pub untracked_from: usize,
}

/// `files` is the status list from [`load_status`]; only its untracked
/// entries are read here, the tracked changes come from the two diffs.
pub fn working_tree_patch(repo_path: &Path, files: &[FileStatus]) -> Result<WorkingTreePatch> {
    // ponytail: the patch is built in one go on the UI thread and kept in
    // memory; untracked content stops at these limits. Move to a loader
    // thread if a huge uncommitted diff ever stalls the list.
    const MAX_UNTRACKED_FILE_BYTES: u64 = 1024 * 1024;
    const MAX_PATCH_LINES: usize = 50_000;

    let repo = Repository::discover(repo_path)?;
    let head_tree = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .and_then(|c| c.tree())
        .ok();
    let mut lines: Vec<String> = Vec::new();
    let section = |lines: &mut Vec<String>, title: &str, body: &str| {
        if !body.is_empty() {
            lines.push(format!(
                "\u{2500}\u{2500}\u{2500}\u{2500} {title} \u{2500}\u{2500}\u{2500}\u{2500}"
            ));
            lines.extend(body.lines().map(String::from));
        }
    };

    let staged = repo.diff_tree_to_index(head_tree.as_ref(), None, None)?;
    section(&mut lines, "Staged", &patch_text(&staged)?);
    let unstaged_from = lines.len();
    let unstaged = repo.diff_index_to_workdir(None, None)?;
    section(&mut lines, "Unstaged", &patch_text(&unstaged)?);
    let untracked_from = lines.len();

    let workdir = repo.workdir().ok_or(Error::NoWorkdir)?;
    let mut untracked = String::new();
    for file in files
        .iter()
        .filter(|f| f.stage_state == StageState::Untracked)
    {
        untracked.push_str(&format!(
            "diff --git a/{0} b/{0}\nnew file mode 100644\n--- /dev/null\n+++ b/{0}\n",
            file.path
        ));
        let full_path = workdir.join(&file.path);
        let size = std::fs::metadata(&full_path).map(|m| m.len()).unwrap_or(0);
        if size > MAX_UNTRACKED_FILE_BYTES {
            untracked.push_str(&format!("(not shown: {size} bytes)\n"));
        } else if lines.len() + untracked.lines().count() > MAX_PATCH_LINES {
            untracked.push_str("(not shown: patch is already very long)\n");
        } else {
            match std::fs::read_to_string(&full_path) {
                Ok(content) => {
                    for line in content.lines() {
                        untracked.push('+');
                        untracked.push_str(line);
                        untracked.push('\n');
                    }
                }
                Err(_) => untracked.push_str("(binary or unreadable)\n"),
            }
        }
    }
    section(&mut lines, "Untracked", &untracked);

    Ok(WorkingTreePatch {
        lines,
        unstaged_from,
        untracked_from,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Staged, unstaged and untracked changes land in one patch, in that
    /// order, each under a `diff --git` header the file parser reads.
    #[test]
    fn working_tree_patch_has_staged_unstaged_and_untracked_parts() {
        let dir = std::env::temp_dir().join(format!("ghh-worktree-patch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = Repository::init(&dir).unwrap();
        let sig = git2::Signature::new("t", "t@t", &git2::Time::new(1_000_000, 0)).unwrap();
        let add = |name: &str| {
            let mut index = repo.index().unwrap();
            index.add_path(Path::new(name)).unwrap();
            index.write().unwrap();
            index.write_tree().unwrap()
        };
        std::fs::write(dir.join("staged.txt"), "one\n").unwrap();
        std::fs::write(dir.join("unstaged.txt"), "one\n").unwrap();
        add("staged.txt");
        let tree = repo.find_tree(add("unstaged.txt")).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "c0", &tree, &[])
            .unwrap();

        std::fs::write(dir.join("staged.txt"), "one\ntwo\n").unwrap();
        add("staged.txt");
        std::fs::write(dir.join("unstaged.txt"), "changed\n").unwrap();
        std::fs::write(dir.join("new.txt"), "fresh\n").unwrap();

        let files = load_status(&dir).unwrap();
        assert_eq!(files.len(), 3);
        let patch = working_tree_patch(&dir, &files).unwrap();
        let header = |name: &str| {
            patch
                .lines
                .iter()
                .position(|l| l == &format!("diff --git a/{name} b/{name}"))
                .unwrap_or_else(|| panic!("no header for {name}: {:#?}", patch.lines))
        };
        assert!(header("staged.txt") < patch.unstaged_from);
        assert!((patch.unstaged_from..patch.untracked_from).contains(&header("unstaged.txt")));
        assert!(header("new.txt") > patch.untracked_from);
        assert!(patch.lines.contains(&"+two".to_string()));
        assert!(patch.lines.contains(&"-one".to_string()));
        assert!(patch.lines.contains(&"+fresh".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
