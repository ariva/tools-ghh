use crate::git::models::{FileStatus, StageState};
use anyhow::{Result, anyhow};
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
        let workdir = repo
            .workdir()
            .ok_or_else(|| anyhow!("No working directory"))?;
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

    let mut output = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        let origin = line.origin();
        if let Ok(content) = std::str::from_utf8(line.content()) {
            let content = content.trim_end_matches('\n').trim_end_matches('\r');
            match origin {
                '+' | '-' | ' ' => {
                    output.push(origin);
                    output.push_str(content);
                    output.push('\n');
                }
                _ => {
                    output.push_str(content);
                    output.push('\n');
                }
            }
        }
        true
    })?;

    if output.is_empty() {
        output = format!("(no diff for {})", file.path);
    }

    Ok(output)
}
