use crate::git::error::{Error, Result};
use crate::git::models::{CommitInfo, CommitStats, RefKind, RefLabel};
use chrono::{NaiveDate, NaiveDateTime};
use git2::{Oid, Repository, Signature, Sort, Time};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
pub struct ProgressiveLoadTimings {
    pub git_spawn: Option<Duration>,
    pub data_init: Option<Duration>,
    pub read_parse: Duration,
    pub snapshot_clone: Duration,
    pub process_wait: Duration,
}

/// Map of full commit hash → refs pointing at it (local/remote branches, tags).
/// Annotated tags are peeled to their target commit, like gitk does.
pub fn load_ref_labels(repo_path: &Path) -> HashMap<String, Vec<RefLabel>> {
    let mut map: HashMap<String, Vec<RefLabel>> = HashMap::new();
    let Ok(repo) = Repository::open(repo_path) else {
        return map;
    };
    let head_branch = repo
        .head()
        .ok()
        .filter(|h| h.is_branch())
        .and_then(|h| h.shorthand().map(String::from));
    let Ok(refs) = repo.references() else {
        return map;
    };
    for r in refs.flatten() {
        let kind = if r.is_branch() {
            RefKind::Branch
        } else if r.is_remote() {
            RefKind::RemoteBranch
        } else if r.is_tag() {
            RefKind::Tag
        } else {
            continue;
        };
        let Some(name) = r.shorthand().map(String::from) else {
            continue;
        };
        let Ok(commit) = r.peel_to_commit() else {
            continue;
        };
        let is_head = kind == RefKind::Branch && head_branch.as_deref() == Some(name.as_str());
        map.entry(commit.id().to_string())
            .or_default()
            .push(RefLabel {
                name,
                kind,
                is_head,
            });
    }
    for labels in map.values_mut() {
        // HEAD branch first, then branches < remotes < tags, then by name.
        labels.sort_by(|a, b| {
            b.is_head
                .cmp(&a.is_head)
                .then(a.kind.cmp(&b.kind))
                .then(a.name.cmp(&b.name))
        });
    }
    map
}

pub fn count_head_commits(repo_path: &Path) -> Result<usize> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["rev-list", "--count", "HEAD"])
        .output()?;
    if !output.status.success() {
        return Err(Error::command("git rev-list --count", &output.stderr));
    }
    Ok(String::from_utf8(output.stdout)?.trim().parse()?)
}

pub fn ensure_commit_graph(repo_path: &Path) -> Result<bool> {
    let monolithic = resolve_git_path(repo_path, "objects/info/commit-graph")?;
    let split_chain = resolve_git_path(repo_path, "objects/info/commit-graphs/commit-graph-chain")?;
    if monolithic.is_file() || split_chain.is_file() {
        return Ok(false);
    }

    let output = Command::new("git")
        .args(["-c", "commitGraph.generationVersion=1"])
        .arg("-C")
        .arg(repo_path)
        .args(["commit-graph", "write", "--reachable"])
        .output()?;
    if !output.status.success() {
        return Err(Error::command("git commit-graph write", &output.stderr));
    }
    Ok(true)
}

fn resolve_git_path(repo_path: &Path, git_path: &str) -> Result<std::path::PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            git_path,
        ])
        .output()?;
    if !output.status.success() {
        return Err(Error::command("git rev-parse", &output.stderr));
    }
    Ok(std::path::PathBuf::from(
        String::from_utf8(output.stdout)?.trim(),
    ))
}

/// Commit-list ordering. [`CommitSort::Topo`] is the default and matches what
/// gitk shows out of the box (gitk reorders `git log` output itself); [`CommitSort::Date`]
/// is gitk's "Strictly sort by date" option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommitSort {
    #[default]
    Topo,
    Date,
}

impl CommitSort {
    fn libgit2_sorting(self) -> Sort {
        match self {
            Self::Topo => Sort::TOPOLOGICAL,
            Self::Date => Sort::TOPOLOGICAL | Sort::TIME,
        }
    }

    fn rev_list_arg(self) -> &'static str {
        match self {
            Self::Topo => "--topo-order",
            Self::Date => "--date-order",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Topo => Self::Date,
            Self::Date => Self::Topo,
        }
    }

    /// Short label for the title bar and status messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::Topo => "topo",
            Self::Date => "date",
        }
    }
}

/// Full hash of the commit `HEAD` resolves to, or `None` on an unborn branch.
pub fn head_commit_hash(repo_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let hash = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!hash.is_empty()).then_some(hash)
}

pub fn load_commits(repo_path: &Path, sort: CommitSort) -> Result<Vec<CommitInfo>> {
    let mut full_history = None;
    if load_commits_progressive(repo_path, sort, &[], |commits, complete, _| {
        if complete {
            full_history = Some(commits);
        }
        true
    })
    .is_ok()
    {
        return Ok(full_history.unwrap_or_default());
    }
    load_commits_limit_via_libgit2(repo_path, sort, None).map(|(commits, _)| commits)
}

fn read_commit_info(repo: &Repository, oid: Oid) -> Result<(CommitInfo, Vec<Oid>)> {
    let commit = repo.find_commit(oid)?;
    let author = commit.author();
    let time = author.when();
    Ok((
        CommitInfo {
            hash: oid.to_string(),
            message: commit.summary().unwrap_or("").to_string(),
            full_message: None,
            author_name: author.name().unwrap_or("").to_string(),
            author_email: String::new(),
            timestamp: time.seconds(),
            offset_minutes: time.offset_minutes(),
            parent_indices: None,
            is_merge: commit.parent_count() > 1,
            graph_prefix: String::new(),
            graph_prefix_lanes: Vec::new(),
        },
        commit.parent_ids().collect(),
    ))
}

/// Hybrid history walk: `git rev-list` (subprocess) streams the commit ORDER
/// — its topological sort needs the whole history in memory, and in a child
/// process that memory is returned to the OS when it exits, and the first
/// lines arrive in milliseconds. Metadata is then read in-process via libgit2
/// (serially for the first small batches, parallel workers for the bulk), so
/// no commit text ever crosses a pipe. Parents are resolved to indices during
/// the walk — the UI never has to ask git for topology again.
pub fn load_commits_progressive<F>(
    repo_path: &Path,
    sort: CommitSort,
    checkpoints: &[usize],
    mut emit: F,
) -> Result<()>
where
    F: FnMut(Vec<CommitInfo>, bool, ProgressiveLoadTimings) -> bool,
{
    use std::io::BufRead;
    let open_started = Instant::now();
    let repo = Repository::discover(repo_path)?;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("rev-list")
        .arg(sort.rev_list_arg())
        .arg("HEAD")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().ok_or(Error::NoRevListOutput)?;
    let mut lines = std::io::BufReader::new(stdout).lines();
    let git_spawn = open_started.elapsed();

    let init_started = Instant::now();
    let mut commits: Vec<CommitInfo> =
        Vec::with_capacity(checkpoints.last().copied().unwrap_or(1024));
    let mut parent_oids: Vec<Vec<Oid>> = Vec::with_capacity(commits.capacity());
    let mut oid_to_index: HashMap<Oid, usize> = HashMap::new();
    let mut next_checkpoint = 0;
    let data_init = init_started.elapsed();
    let mut startup_timings = Some((git_spawn, data_init));
    let mut read_parse_started = Instant::now();

    // Re-run per emit: commits near the bottom of a batch reference parents
    // that haven't been walked yet; the next emit picks them up. Cost is a
    // hash lookup per parent, three emits total.
    let resolve_parents = |commits: &mut [CommitInfo],
                           parent_oids: &[Vec<Oid>],
                           oid_to_index: &HashMap<Oid, usize>| {
        for (commit, parents) in commits.iter_mut().zip(parent_oids) {
            commit.parent_indices = Some(
                parents
                    .iter()
                    .filter_map(|oid| oid_to_index.get(oid).copied())
                    .collect(),
            );
        }
    };

    // Serial phase: stream the small checkpoint batches for instant first paint.
    while next_checkpoint < checkpoints.len() {
        let Some(line) = lines.next() else {
            break;
        };
        let Ok(oid) = Oid::from_str(line?.trim()) else {
            continue;
        };
        let (info, parents) = read_commit_info(&repo, oid)?;
        commits.push(info);
        parent_oids.push(parents);
        oid_to_index.insert(oid, commits.len() - 1);

        if commits.len() == checkpoints[next_checkpoint] {
            let read_parse = read_parse_started.elapsed();
            let clone_started = Instant::now();
            resolve_parents(&mut commits, &parent_oids, &oid_to_index);
            let snapshot = commits.clone();
            let snapshot_clone = clone_started.elapsed();
            let (git_spawn, data_init) = startup_timings
                .take()
                .map(|(spawn, init)| (Some(spawn), Some(init)))
                .unwrap_or_default();
            if !emit(
                snapshot,
                false,
                ProgressiveLoadTimings {
                    git_spawn,
                    data_init,
                    read_parse,
                    snapshot_clone,
                    process_wait: Duration::ZERO,
                },
            ) {
                return Ok(());
            }
            next_checkpoint += 1;
            read_parse_started = Instant::now();
        }
    }

    // Bulk phase: drain the remaining OIDs (23 MB of hex lines on a 568k
    // repo — order only), then read commit metadata in parallel. Each worker
    // opens its own Repository; chunks keep walk order, so batch prefixes
    // stay stable.
    let remaining: Vec<Oid> = lines
        .map_while(std::result::Result::ok)
        .filter_map(|line| Oid::from_str(line.trim()).ok())
        .collect();
    let _ = child.wait();
    let serial_len = commits.len();
    if !remaining.is_empty() {
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(1, 8);
        let chunk_size = remaining.len().div_ceil(workers).max(1);
        // Workers fill preallocated slices in place — no per-chunk result
        // vectors, so bulk loading never holds two copies of the history.
        let total = serial_len + remaining.len();
        commits.resize_with(total, CommitInfo::placeholder);
        parent_oids.resize_with(total, Vec::new);
        let statuses: Vec<Result<()>> = std::thread::scope(|s| {
            let mut commit_tail = &mut commits[serial_len..];
            let mut parent_tail = &mut parent_oids[serial_len..];
            let mut handles = Vec::new();
            for slice in remaining.chunks(chunk_size) {
                let (commit_chunk, commit_rest) = commit_tail.split_at_mut(slice.len());
                let (parent_chunk, parent_rest) = parent_tail.split_at_mut(slice.len());
                commit_tail = commit_rest;
                parent_tail = parent_rest;
                let path = repo_path.to_path_buf();
                handles.push(s.spawn(move || -> Result<()> {
                    let repo = Repository::discover(&path)?;
                    for (i, &oid) in slice.iter().enumerate() {
                        let (info, parents) = read_commit_info(&repo, oid)?;
                        commit_chunk[i] = info;
                        parent_chunk[i] = parents;
                    }
                    Ok(())
                }));
            }
            handles
                .into_iter()
                .map(|handle| handle.join().expect("history reader thread panicked"))
                .collect()
        });
        for status in statuses {
            status?;
        }
        for (offset, oid) in remaining.iter().enumerate() {
            oid_to_index.insert(*oid, serial_len + offset);
        }
    }

    resolve_parents(&mut commits, &parent_oids, &oid_to_index);
    let read_parse = read_parse_started.elapsed();
    let (git_spawn, data_init) = startup_timings
        .take()
        .map(|(spawn, init)| (Some(spawn), Some(init)))
        .unwrap_or_default();
    emit(
        commits,
        true,
        ProgressiveLoadTimings {
            git_spawn,
            data_init,
            read_parse,
            snapshot_clone: Duration::ZERO,
            process_wait: Duration::ZERO,
        },
    );
    Ok(())
}

pub(crate) fn has_stable_commit_prefix(current: &[CommitInfo], next: &[CommitInfo]) -> bool {
    current.len() <= next.len()
        && current
            .iter()
            .zip(next)
            .all(|(current, next)| current.hash == next.hash)
}

fn load_commits_limit_via_libgit2(
    repo_path: &Path,
    sort: CommitSort,
    limit: Option<usize>,
) -> Result<(Vec<CommitInfo>, bool)> {
    let repo = Repository::discover(repo_path)?;
    let mut revwalk = repo.revwalk()?;
    revwalk.push_head()?;
    revwalk.set_sorting(sort.libgit2_sorting())?;

    let mut commits = Vec::new();
    let mut commit_oids: Vec<Oid> = Vec::new();
    let mut parent_oids: Vec<Vec<Oid>> = Vec::new();
    let mut has_more = false;
    for oid_result in revwalk {
        if limit.is_some_and(|limit| commits.len() >= limit) {
            has_more = true;
            break;
        }
        let oid = oid_result?;
        let commit = repo.find_commit(oid)?;
        let author = commit.author();
        let time = author.when();

        let hash = oid.to_string();
        let message = commit.summary().unwrap_or("").to_string();
        let is_merge = commit.parent_count() > 1;
        let commit_parent_oids: Vec<Oid> = commit.parent_ids().collect();

        commits.push(CommitInfo {
            hash,
            message,
            full_message: None,
            author_name: author.name().unwrap_or("").to_string(),
            author_email: String::new(),
            timestamp: time.seconds(),
            offset_minutes: time.offset_minutes(),
            parent_indices: Some(Vec::new()),
            is_merge,
            graph_prefix: String::new(),
            graph_prefix_lanes: Vec::new(),
        });
        commit_oids.push(oid);
        parent_oids.push(commit_parent_oids);
    }

    let oid_to_index: HashMap<Oid, usize> = commit_oids
        .iter()
        .enumerate()
        .map(|(idx, oid)| (*oid, idx))
        .collect();
    for (idx, commit) in commits.iter_mut().enumerate() {
        commit.parent_indices = Some(
            parent_oids[idx]
                .iter()
                .filter_map(|oid| oid_to_index.get(oid).copied())
                .collect(),
        );
    }
    Ok((commits, has_more))
}

/// What happened to the file(s) matching `patterns` in `hash`, vs its first
/// parent: A(dded) M(odified) D(eleted) R(enamed). None when the commit
/// touched no matching file. Rename detection on; first matching delta wins.
pub fn file_status_in_commit(
    repo_path: &Path,
    hash: &str,
    patterns: &[String],
) -> Result<Option<char>> {
    let repo = Repository::discover(repo_path)?;
    let commit = repo
        .revparse_single(hash)
        .map_err(|_| Error::CommitNotFound(hash.to_string()))?
        .peel_to_commit()?;
    let tree = commit.tree()?;
    let parent_tree = match commit.parent(0) {
        Ok(parent) => Some(parent.tree()?),
        Err(_) => None, // root commit: diff against the empty tree
    };
    let mut options = git2::DiffOptions::new();
    for pattern in patterns {
        options.pathspec(pattern);
    }
    let mut diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut options))?;
    let mut find = git2::DiffFindOptions::new();
    find.renames(true);
    diff.find_similar(Some(&mut find))?;
    let status = diff.deltas().next().map(|delta| match delta.status() {
        git2::Delta::Added | git2::Delta::Copied => 'A',
        git2::Delta::Deleted => 'D',
        git2::Delta::Renamed => 'R',
        _ => 'M',
    });
    Ok(status)
}

pub fn get_commit_author_email(repo_path: &Path, hash: &str) -> Result<String> {
    let repo = Repository::discover(repo_path)?;
    let oid = repo
        .revparse_single(hash)
        .map_err(|_| Error::CommitNotFound(hash.to_string()))?;
    let commit = oid.peel_to_commit()?;
    Ok(commit.author().email().unwrap_or("").to_string())
}

pub fn get_commit_full_message(repo_path: &Path, hash: &str) -> Result<String> {
    let repo = Repository::discover(repo_path)?;
    let oid = repo
        .revparse_single(hash)
        .map_err(|_| Error::CommitNotFound(hash.to_string()))?;
    let commit = oid.peel_to_commit()?;
    Ok(commit.message().unwrap_or("").to_string())
}

/// Multiple pathspecs are a union: a commit matches if it touched any of them.
pub fn matching_commit_hashes_for_file(
    repo_path: &Path,
    patterns: &[&str],
    full_history: bool,
    diff_filter: Option<char>,
) -> Result<HashSet<String>> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo_path).args(["log", "--all"]);
    // --full-history: without it git's history simplification drops merge
    // commits from pathspec logs, so file search misses merges that touch
    // the file (found via 366029b in big-dummy-repo). Slower; the popup
    // checkbox can turn it off.
    if full_history {
        command.arg("--full-history");
    }
    // A/M/D/R action filter; rename detection must be on for R.
    if let Some(action) = diff_filter {
        command.arg(format!("--diff-filter={action}"));
        if action == 'R' {
            command.arg("-M");
        }
    }
    let output = command
        .args(["--format=%H", "--"])
        .args(patterns)
        .output()?;
    if !output.status.success() {
        return Err(Error::command("git file search", &output.stderr));
    }
    Ok(parse_hash_lines(&output.stdout))
}

fn parse_hash_lines(stdout: &[u8]) -> HashSet<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter(|line| line.len() == 40 && line.chars().all(|ch| ch.is_ascii_hexdigit()))
        .map(ToOwned::to_owned)
        .collect()
}

pub fn list_repo_files(repo_path: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .output()?;
    if !output.status.success() {
        return Err(Error::command("git file list", &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub fn populate_commit_parents(
    repo_path: &Path,
    commits: &mut [CommitInfo],
    start: usize,
    end: usize,
) -> Result<()> {
    let end = end.min(commits.len());
    if commits[start.min(end)..end]
        .iter()
        .all(|commit| commit.parent_indices.is_some())
    {
        return Ok(());
    }

    let repo = Repository::discover(repo_path)?;
    let hash_to_index: HashMap<String, usize> = commits
        .iter()
        .enumerate()
        .map(|(idx, commit)| (commit.hash.clone(), idx))
        .collect();
    for commit in commits.iter_mut().take(end).skip(start) {
        if commit.parent_indices.is_some() {
            continue;
        }
        let oid =
            Oid::from_str(&commit.hash).map_err(|_| Error::CommitNotFound(commit.hash.clone()))?;
        let git_commit = repo.find_commit(oid)?;
        let parent_hashes: Vec<String> = git_commit.parent_ids().map(|id| id.to_string()).collect();
        commit.parent_indices = Some(
            parent_hashes
                .iter()
                .filter_map(|hash| hash_to_index.get(hash).copied())
                .collect(),
        );
        commit.is_merge = parent_hashes.len() > 1;
    }
    Ok(())
}

pub fn modify_commit(
    repo_path: &Path,
    target_hash: &str,
    new_message: Option<&str>,
    new_author_name: Option<&str>,
    new_author_email: Option<&str>,
    new_date: Option<&str>,
) -> Result<()> {
    let repo = Repository::discover(repo_path)?;

    let target_oid = repo
        .revparse_single(target_hash)
        .map_err(|_| Error::CommitNotFound(target_hash.to_string()))?
        .peel_to_commit()?
        .id();

    let head = repo.head()?;
    let head_oid = head.target().ok_or(Error::HeadNotDirect)?;

    let mut revwalk = repo.revwalk()?;
    revwalk.push(head_oid)?;
    revwalk.set_sorting(Sort::TOPOLOGICAL)?;

    let mut commits_to_rewrite: Vec<Oid> = Vec::new();
    let mut found = false;
    for oid_result in &mut revwalk {
        let oid = oid_result?;
        commits_to_rewrite.push(oid);
        if oid == target_oid {
            found = true;
            break;
        }
    }

    if !found {
        return Err(Error::NotInBranchHistory(target_hash.to_string()));
    }

    commits_to_rewrite.reverse();

    let mut oid_map: HashMap<Oid, Oid> = HashMap::new();

    for (i, &old_oid) in commits_to_rewrite.iter().enumerate() {
        let old_commit = repo.find_commit(old_oid)?;
        let old_tree = old_commit.tree()?;

        let new_parent_oids: Vec<Oid> = old_commit
            .parent_ids()
            .map(|p| oid_map.get(&p).copied().unwrap_or(p))
            .collect();
        let new_parents: Vec<git2::Commit<'_>> = new_parent_oids
            .iter()
            .map(|&oid| repo.find_commit(oid))
            .collect::<std::result::Result<_, git2::Error>>()?;
        let new_parents_refs: Vec<&git2::Commit<'_>> = new_parents.iter().collect();

        let old_author = old_commit.author();
        let old_committer = old_commit.committer();

        let (author_name, author_email, author_time) = if i == 0 {
            let name = new_author_name
                .unwrap_or_else(|| old_author.name().unwrap_or(""))
                .to_string();
            let email = new_author_email
                .unwrap_or_else(|| old_author.email().unwrap_or(""))
                .to_string();
            let time = if let Some(date_str) = new_date {
                parse_date_to_git_time(
                    date_str,
                    old_author.when().offset_minutes(),
                    chrono::Utc::now().timestamp().rem_euclid(60) as u32,
                )?
            } else {
                old_author.when()
            };
            (name, email, time)
        } else {
            (
                old_author.name().unwrap_or("").to_string(),
                old_author.email().unwrap_or("").to_string(),
                old_author.when(),
            )
        };

        let message = if i == 0 {
            new_message
                .unwrap_or_else(|| old_commit.message().unwrap_or(""))
                .to_string()
        } else {
            old_commit.message().unwrap_or("").to_string()
        };

        let new_author = Signature::new(&author_name, &author_email, &author_time)?;
        // An edited date applies to BOTH dates: tools sorting by committer
        // date (git log --date-order, GitHub) would otherwise still show
        // the old one.
        let committer_time = if i == 0 && new_date.is_some() {
            author_time
        } else {
            old_committer.when()
        };
        let committer_sig = Signature::new(
            old_committer.name().unwrap_or(""),
            old_committer.email().unwrap_or(""),
            &committer_time,
        )?;

        let new_oid = repo.commit(
            None,
            &new_author,
            &committer_sig,
            &message,
            &old_tree,
            &new_parents_refs,
        )?;

        oid_map.insert(old_oid, new_oid);
    }

    let new_head_oid = oid_map[&head_oid];
    if head.is_branch() {
        let head_ref_name = head.name().ok_or(Error::HeadRefName)?.to_string();
        repo.find_reference(&head_ref_name)?
            .set_target(new_head_oid, "ghh: rewrite commit")?;
    } else {
        repo.set_head_detached(new_head_oid)?;
    }

    Ok(())
}

/// Parse an edited date. Inputs without an explicit seconds part (HH:MM or
/// date-only) get `fill_seconds` (the wall clock's current seconds at edit
/// time), so edited commits never all sit at :00.
fn parse_date_to_git_time(date_str: &str, offset_minutes: i32, fill_seconds: u32) -> Result<Time> {
    let s = date_str.trim();
    let fill_seconds = fill_seconds.min(59);

    if let Ok(ts) = s.parse::<i64>() {
        return Ok(Time::new(ts, offset_minutes));
    }

    // Wall-clock input is in the commit's own timezone (the same one
    // `date_display_text` renders in), not UTC.
    let in_offset = |dt: NaiveDateTime| {
        Time::new(
            dt.and_utc().timestamp() - i64::from(offset_minutes) * 60,
            offset_minutes,
        )
    };

    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Ok(in_offset(dt));
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M") {
        return Ok(in_offset(
            dt + chrono::Duration::seconds(i64::from(fill_seconds)),
        ));
    }

    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(in_offset(d.and_hms_opt(0, 0, fill_seconds).unwrap()));
    }

    Err(Error::InvalidDate(s.to_string()))
}

/// Squash a set of commits (base + squash_hashes) into one commit.
pub fn squash_commits(
    repo_path: &Path,
    base_hash: &str,
    squash_hashes: &[String],
    message: &str,
) -> Result<()> {
    if squash_hashes.is_empty() {
        return Err(Error::NothingToSquash);
    }

    let repo = Repository::discover(repo_path)?;

    let base_oid = resolve_commit_oid(&repo, base_hash)
        .map_err(|_| Error::SquashBaseNotFound(base_hash.trim().to_string()))?;

    let squash_oids: Vec<Oid> = squash_hashes
        .iter()
        .map(|hash| {
            resolve_commit_oid(&repo, hash).map_err(|e| Error::SquashCommitNotFound {
                hash: hash.trim().to_string(),
                source: Box::new(e),
            })
        })
        .collect::<Result<_>>()?;
    let squash_set: HashSet<Oid> = squash_oids.iter().copied().collect();

    if squash_set.contains(&base_oid) {
        return Err(Error::SquashBaseIntoItself);
    }

    let all_selected: HashSet<Oid> = squash_set
        .iter()
        .copied()
        .chain(std::iter::once(base_oid))
        .collect();

    let head = repo.head()?;
    let head_oid = head.target().ok_or(Error::HeadNotDirect)?;

    let mut revwalk = repo.revwalk()?;
    revwalk.push(head_oid)?;
    revwalk.set_sorting(Sort::TOPOLOGICAL)?;

    let mut chain: Vec<Oid> = Vec::new();
    let mut seen = 0usize;
    for oid_res in &mut revwalk {
        let oid = oid_res?;
        chain.push(oid);
        if all_selected.contains(&oid) {
            seen += 1;
            if seen == all_selected.len() {
                break;
            }
        }
    }
    if seen < all_selected.len() {
        return Err(Error::SquashNotInHistory {
            found: seen,
            selected: all_selected.len(),
        });
    }

    for &oid in &all_selected {
        if repo.find_commit(oid)?.parent_count() > 1 {
            return Err(Error::SquashMerge(oid.to_string()[..7].to_string()));
        }
    }

    let sel_positions: Vec<usize> = chain
        .iter()
        .enumerate()
        .filter(|(_, id)| all_selected.contains(*id))
        .map(|(i, _)| i)
        .collect();

    let newest_sel_pos = *sel_positions.iter().min().unwrap();
    let oldest_sel_pos = *sel_positions.iter().max().unwrap();

    for id in &chain[(newest_sel_pos + 1)..oldest_sel_pos] {
        if !all_selected.contains(id) {
            let c = repo.find_commit(*id)?;
            let msg = c.message().unwrap_or("").lines().next().unwrap_or("");
            return Err(Error::SquashGap {
                hash: id.to_string()[..7].to_string(),
                subject: msg.to_string(),
            });
        }
    }

    let tip_oid = chain[newest_sel_pos];
    let oldest_sel_oid = chain[oldest_sel_pos];
    let tip_tree = repo.find_commit(tip_oid)?.tree()?;

    chain.reverse();

    let mut oid_map: HashMap<Oid, Oid> = HashMap::new();
    let mut new_combined: Option<Oid> = None;

    for &old_oid in &chain {
        let old_commit = repo.find_commit(old_oid)?;

        let new_parent_oids: Vec<Oid> = old_commit
            .parent_ids()
            .map(|p| oid_map.get(&p).copied().unwrap_or(p))
            .collect();
        let new_parents: Vec<git2::Commit<'_>> = new_parent_oids
            .iter()
            .map(|&oid| repo.find_commit(oid))
            .collect::<std::result::Result<_, git2::Error>>()?;
        let new_parents_refs: Vec<&git2::Commit<'_>> = new_parents.iter().collect();

        if old_oid == oldest_sel_oid {
            let base_commit = repo.find_commit(base_oid)?;
            let ba = base_commit.author();
            let bc = base_commit.committer();
            let author = Signature::new(
                ba.name().unwrap_or(""),
                ba.email().unwrap_or(""),
                &ba.when(),
            )?;
            let committer = Signature::new(
                bc.name().unwrap_or(""),
                bc.email().unwrap_or(""),
                &bc.when(),
            )?;
            let new_oid = repo.commit(
                None,
                &author,
                &committer,
                message,
                &tip_tree,
                &new_parents_refs,
            )?;
            oid_map.insert(old_oid, new_oid);
            new_combined = Some(new_oid);
        } else if all_selected.contains(&old_oid) {
            oid_map.insert(old_oid, new_combined.unwrap());
        } else {
            let old_author = old_commit.author();
            let old_committer = old_commit.committer();
            let msg = old_commit.message().unwrap_or("").to_string();
            let author = Signature::new(
                old_author.name().unwrap_or(""),
                old_author.email().unwrap_or(""),
                &old_author.when(),
            )?;
            let committer = Signature::new(
                old_committer.name().unwrap_or(""),
                old_committer.email().unwrap_or(""),
                &old_committer.when(),
            )?;
            let old_tree = old_commit.tree()?;
            let new_oid = repo.commit(
                None,
                &author,
                &committer,
                &msg,
                &old_tree,
                &new_parents_refs,
            )?;
            oid_map.insert(old_oid, new_oid);
        }
    }

    let new_head_oid = oid_map[&head_oid];
    if head.is_branch() {
        let head_ref_name = head.name().ok_or(Error::HeadRefName)?.to_string();
        repo.find_reference(&head_ref_name)?
            .set_target(new_head_oid, "ghh: squash commits")?;
    } else {
        repo.set_head_detached(new_head_oid)?;
    }

    Ok(())
}

fn resolve_commit_oid(repo: &Repository, hash: &str) -> Result<Oid> {
    let normalized = hash.trim();
    let oid = Oid::from_str(normalized).map_err(|source| Error::InvalidHash {
        hash: normalized.to_string(),
        source,
    })?;
    repo.find_commit(oid)
        .map(|_| oid)
        .map_err(Error::CommitUnavailable)
}

pub fn get_commit_stats(repo_path: &Path, hash: &str) -> Result<CommitStats> {
    let repo = Repository::discover(repo_path)?;
    let oid = repo
        .revparse_single(hash)
        .map_err(|_| Error::CommitNotFound(hash.to_string()))?;
    let commit = oid.peel_to_commit()?;
    let tree = commit.tree()?;

    let parent_tree = commit.parent(0).and_then(|p| p.tree()).ok();
    let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;

    let diff_stats = diff.stats()?;
    let mut files_added = 0usize;
    let mut files_modified = 0usize;
    let mut files_deleted = 0usize;

    diff.foreach(
        &mut |delta, _progress| {
            match delta.status() {
                git2::Delta::Added | git2::Delta::Copied => files_added += 1,
                git2::Delta::Deleted => files_deleted += 1,
                _ => files_modified += 1,
            }
            true
        },
        None,
        None,
        None,
    )?;

    Ok(CommitStats {
        files_added,
        files_modified,
        files_deleted,
        lines_inserted: diff_stats.insertions(),
        lines_deleted: diff_stats.deletions(),
    })
}

pub fn get_commit_diff(repo_path: &Path, hash: &str) -> Result<String> {
    let repo = Repository::discover(repo_path)?;
    let oid = repo
        .revparse_single(hash)
        .map_err(|_| Error::CommitNotFound(hash.to_string()))?;
    let commit = oid.peel_to_commit()?;
    let tree = commit.tree()?;

    let parent_tree = commit.parent(0).and_then(|p| p.tree()).ok();
    let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;

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
        output = format!("(no diff for commit {hash})");
    }

    Ok(output)
}

pub fn get_commit_patch(repo_path: &Path, hash: &str) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args([
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--binary",
            "--full-index",
            "--no-ext-diff",
            "-p",
            hash,
        ])
        .output()?;
    if !output.status.success() {
        return Err(Error::command("git diff-tree", &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::{CommitSort, Sort, has_stable_commit_prefix, parse_date_to_git_time};

    #[test]
    fn ref_labels_cover_branches_and_peeled_tags() {
        use crate::git::models::RefKind;

        let dir = std::env::temp_dir().join(format!("ghh-ref-labels-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = git2::Repository::init(&dir).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
        let commit_obj = repo.find_object(oid, None).unwrap();
        repo.tag("v1", &commit_obj, &sig, "annotated", false)
            .unwrap();

        let labels = super::load_ref_labels(&dir);
        let on_commit = &labels[&oid.to_string()];
        // HEAD branch sorts first, annotated tag peels to the commit.
        assert!(on_commit[0].kind == RefKind::Branch && on_commit[0].is_head);
        assert!(
            on_commit
                .iter()
                .any(|l| l.kind == RefKind::Tag && l.name == "v1")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression: an edited date must land on BOTH signatures — the old
    /// code rewrote only the author date, so committer-date-ordered tools
    /// kept showing the old date.
    #[test]
    fn modify_commit_date_updates_committer_date_too() {
        let dir = std::env::temp_dir().join(format!("ghh-mod-date-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = git2::Repository::init(&dir).unwrap();
        let sig = git2::Signature::new("t", "t@t", &git2::Time::new(1_000_000, 0)).unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();

        super::modify_commit(&dir, &oid.to_string(), None, None, None, Some("2000000")).unwrap();

        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.author().when().seconds(), 2_000_000);
        assert_eq!(
            head.committer().when().seconds(),
            2_000_000,
            "committer date must follow the edited date"
        );
        // Without a date edit both signatures keep their own dates.
        super::modify_commit(
            &dir,
            &head.id().to_string(),
            Some("renamed message"),
            None,
            None,
            None,
        )
        .unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.committer().when().seconds(), 2_000_000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression: `git log -- <path>` history simplification drops merge
    /// commits, so a merge touching the file was invisible to file search
    /// (366029b in big-dummy-repo). --full-history must keep it.
    #[test]
    fn file_search_includes_merge_commits_touching_the_file() {
        let dir = std::env::temp_dir().join(format!("ghh-file-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = git2::Repository::init(&dir).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let make_commit = |parents: &[git2::Oid], entries: &[(&str, &str)], msg: &str| {
            let mut builder = repo.treebuilder(None).unwrap();
            for (name, content) in entries {
                let blob = repo.blob(content.as_bytes()).unwrap();
                builder.insert(name, blob, 0o100644).unwrap();
            }
            let tree = repo.find_tree(builder.write().unwrap()).unwrap();
            let parent_commits: Vec<git2::Commit> = parents
                .iter()
                .map(|oid| repo.find_commit(*oid).unwrap())
                .collect();
            let parent_refs: Vec<&git2::Commit> = parent_commits.iter().collect();
            repo.commit(None, &sig, &sig, msg, &tree, &parent_refs)
                .unwrap()
        };
        let base = make_commit(&[], &[("CHANGELOG.md", "v1"), ("other.txt", "x")], "base");
        // Side branch edits CHANGELOG.md; mainline edits other.txt.
        let side = make_commit(
            &[base],
            &[("CHANGELOG.md", "v2"), ("other.txt", "x")],
            "side",
        );
        let main = make_commit(
            &[base],
            &[("CHANGELOG.md", "v1"), ("other.txt", "y")],
            "main",
        );
        // Merge takes the side's CHANGELOG.md: TREESAME to side -> default
        // git log -- CHANGELOG.md drops it.
        let merge = make_commit(
            &[main, side],
            &[("CHANGELOG.md", "v2"), ("other.txt", "y")],
            "merge side",
        );
        repo.reference("refs/heads/main", merge, true, "test")
            .unwrap();
        let hashes =
            super::matching_commit_hashes_for_file(&dir, &["CHANGELOG.md"], true, None).unwrap();
        assert!(hashes.contains(&base.to_string()));
        assert!(hashes.contains(&side.to_string()));
        assert!(
            hashes.contains(&merge.to_string()),
            "merge touching the file must be found"
        );
        assert!(!hashes.contains(&main.to_string()), "main didn't touch it");
        // Fast mode documents the tradeoff: the merge is simplified away.
        let fast =
            super::matching_commit_hashes_for_file(&dir, &["CHANGELOG.md"], false, None).unwrap();
        assert!(!fast.contains(&merge.to_string()), "fast mode skips merges");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// --diff-filter narrows file search to one action: add/modify/delete.
    #[test]
    fn file_search_diff_filter_narrows_by_action() {
        let dir = std::env::temp_dir().join(format!("ghh-file-action-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = git2::Repository::init(&dir).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let make_commit = |parents: &[git2::Oid], entries: &[(&str, &str)], msg: &str| {
            let mut builder = repo.treebuilder(None).unwrap();
            for (name, content) in entries {
                let blob = repo.blob(content.as_bytes()).unwrap();
                builder.insert(name, blob, 0o100644).unwrap();
            }
            let tree = repo.find_tree(builder.write().unwrap()).unwrap();
            let parent_commits: Vec<git2::Commit> = parents
                .iter()
                .map(|oid| repo.find_commit(*oid).unwrap())
                .collect();
            let parent_refs: Vec<&git2::Commit> = parent_commits.iter().collect();
            repo.commit(None, &sig, &sig, msg, &tree, &parent_refs)
                .unwrap()
        };
        let added = make_commit(&[], &[("file.txt", "v1"), ("keep.txt", "k")], "add file");
        let modified = make_commit(
            &[added],
            &[("file.txt", "v2"), ("keep.txt", "k")],
            "modify file",
        );
        let deleted = make_commit(&[modified], &[("keep.txt", "k")], "delete file");
        repo.reference("refs/heads/main", deleted, true, "test")
            .unwrap();
        let by = |filter: Option<char>| {
            super::matching_commit_hashes_for_file(&dir, &["file.txt"], true, filter).unwrap()
        };
        let all = by(None);
        assert!(all.contains(&added.to_string()));
        assert!(all.contains(&modified.to_string()));
        assert!(all.contains(&deleted.to_string()));
        assert_eq!(by(Some('A')), [added.to_string()].into_iter().collect());
        assert_eq!(by(Some('M')), [modified.to_string()].into_iter().collect());
        assert_eq!(by(Some('D')), [deleted.to_string()].into_iter().collect());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_sort_is_topological_like_gitk() {
        // gitk passes no ordering flag and reorders git log itself; the result
        // matches --topo-order, so that is the default here.
        assert_eq!(CommitSort::default(), CommitSort::Topo);
        assert_eq!(CommitSort::Topo.libgit2_sorting(), Sort::TOPOLOGICAL);
        assert_eq!(
            CommitSort::Date.libgit2_sorting(),
            Sort::TOPOLOGICAL | Sort::TIME
        );
    }

    /// Entered wall-clock time is interpreted in the commit's timezone, so
    /// display → edit → parse round-trips to the same timestamp.
    #[test]
    fn parse_date_uses_commit_offset_not_utc() {
        // 2020-12-25 18:31 at UTC+03:00 = 15:31 UTC = 1608910260
        let t = parse_date_to_git_time("2020-12-25 18:31", 180, 0).unwrap();
        assert_eq!(t.seconds(), 1608910260);
        assert_eq!(t.offset_minutes(), 180);

        // Round-trip through the display formatter.
        let info = commit("a", "one");
        let info = crate::git::CommitInfo {
            timestamp: t.seconds(),
            offset_minutes: t.offset_minutes(),
            ..info
        };
        assert_eq!(info.date_display_text(), "2020-12-25 18:31");
        assert_eq!(info.date_edit_text(), "2020-12-25 18:31:00");

        // Negative offset and date-only form: midnight at UTC-05:30.
        let t = parse_date_to_git_time("2020-12-25", -330, 0).unwrap();
        assert_eq!(t.seconds(), 1608854400 + 330 * 60);

        // Raw unix timestamp passes through untouched.
        assert_eq!(
            parse_date_to_git_time("1608910260", 180, 0)
                .unwrap()
                .seconds(),
            1608910260
        );
    }

    /// Inputs without an explicit seconds part get the wall clock's current
    /// seconds — edited commits must not all land on :00.
    #[test]
    fn parse_date_fills_missing_seconds() {
        let base = parse_date_to_git_time("2020-12-25 18:31", 180, 0)
            .unwrap()
            .seconds();
        // HH:MM input: fill applied.
        assert_eq!(
            parse_date_to_git_time("2020-12-25 18:31", 180, 42)
                .unwrap()
                .seconds(),
            base + 42
        );
        // Explicit seconds win — no fill.
        assert_eq!(
            parse_date_to_git_time("2020-12-25 18:31:07", 180, 42)
                .unwrap()
                .seconds(),
            base + 7
        );
        // Date-only input: midnight + fill.
        let midnight = parse_date_to_git_time("2020-12-25", 0, 0)
            .unwrap()
            .seconds();
        assert_eq!(
            parse_date_to_git_time("2020-12-25", 0, 59)
                .unwrap()
                .seconds(),
            midnight + 59
        );
        // Out-of-range fill is clamped, never rolls the minute over.
        assert_eq!(
            parse_date_to_git_time("2020-12-25 18:31", 180, 99)
                .unwrap()
                .seconds(),
            base + 59
        );
    }

    #[test]
    fn sort_toggles_both_ways() {
        assert_eq!(CommitSort::Topo.toggled(), CommitSort::Date);
        assert_eq!(CommitSort::Date.toggled(), CommitSort::Topo);
        assert_eq!(CommitSort::Topo.toggled().toggled(), CommitSort::Topo);
    }

    fn commit(hash: &str, message: &str) -> crate::git::CommitInfo {
        crate::git::CommitInfo {
            hash: hash.to_string(),
            message: message.to_string(),
            full_message: None,
            author_name: "Author".to_string(),
            author_email: String::new(),
            timestamp: 1,
            offset_minutes: 0,
            parent_indices: None,
            is_merge: false,
            graph_prefix: String::new(),
            graph_prefix_lanes: Vec::new(),
        }
    }

    #[test]
    fn progressive_batches_require_stable_prefix() {
        let first = vec![commit("a", "one"), commit("b", "two")];
        let extended = vec![commit("a", "one"), commit("b", "two"), commit("c", "three")];
        let reordered = vec![commit("b", "two"), commit("a", "one"), commit("c", "three")];

        assert!(has_stable_commit_prefix(&first, &extended));
        assert!(!has_stable_commit_prefix(&first, &reordered));
    }

    /// A/M/D/R detection for the file-search letters, against a real repo:
    /// add → modify → rename → delete of the same file.
    #[test]
    fn file_status_detects_a_m_r_d() {
        let dir = std::env::temp_dir().join(format!("ghh-fstatus-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = git2::Repository::init(&dir).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let commit_all = |message: &str| {
            let mut index = repo.index().unwrap();
            index
                .add_all(["."], git2::IndexAddOption::DEFAULT, None)
                .unwrap();
            index.update_all(["."], None).unwrap();
            index.write().unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
            let parents: Vec<&git2::Commit> = parent.iter().collect();
            repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
                .unwrap()
                .to_string()
        };

        std::fs::write(dir.join("data.txt"), "one\ntwo\nthree\n").unwrap();
        let added = commit_all("add");
        std::fs::write(dir.join("data.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        let modified = commit_all("modify");
        std::fs::rename(dir.join("data.txt"), dir.join("moved.txt")).unwrap();
        let renamed = commit_all("rename");
        std::fs::remove_file(dir.join("moved.txt")).unwrap();
        let deleted = commit_all("delete");

        let patterns = vec!["data.txt".to_string(), "moved.txt".to_string()];
        let status = |hash: &str| super::file_status_in_commit(&dir, hash, &patterns).unwrap();
        assert_eq!(status(&added), Some('A'));
        assert_eq!(status(&modified), Some('M'));
        assert_eq!(status(&renamed), Some('R'));
        assert_eq!(status(&deleted), Some('D'));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Unborn HEAD (fresh `git init`): loader completes with zero commits
    /// instead of erroring or hanging.
    #[test]
    fn loader_handles_unborn_head() {
        let dir = std::env::temp_dir().join(format!("ghh-unborn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        git2::Repository::init(&dir).unwrap();

        let mut emitted = Vec::new();
        super::load_commits_progressive(&dir, CommitSort::Topo, &[2], |commits, complete, _| {
            emitted.push((commits.len(), complete));
            true
        })
        .unwrap();
        assert_eq!(emitted, vec![(0, true)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Not a repository at all: loader errors instead of hanging or panicking.
    #[test]
    fn loader_errors_on_non_repo() {
        let dir = std::env::temp_dir().join(format!("ghh-non-repo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let result = super::load_commits_progressive(&dir, CommitSort::Topo, &[2], |_, _, _| true);
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// End-to-end: libgit2 progressive loader emits stable-prefix batches with
    /// parent indices resolved and metadata populated.
    #[test]
    fn libgit2_progressive_loader_resolves_parents_and_batches() {
        let dir = std::env::temp_dir().join(format!("ghh-libgit2-loader-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = git2::Repository::init(&dir).unwrap();
        let sig = git2::Signature::now("Loader Test", "l@t").unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let mut parent: Option<git2::Oid> = None;
        for i in 0..5 {
            let parents: Vec<git2::Commit> = parent
                .map(|p| repo.find_commit(p).unwrap())
                .into_iter()
                .collect();
            let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
            parent = Some(
                repo.commit(
                    Some("HEAD"),
                    &sig,
                    &sig,
                    &format!("c{i}"),
                    &tree,
                    &parent_refs,
                )
                .unwrap(),
            );
        }

        let mut batches: Vec<(usize, bool)> = Vec::new();
        let mut prev: Vec<crate::git::CommitInfo> = Vec::new();
        super::load_commits_progressive(&dir, CommitSort::Topo, &[2], |commits, complete, _| {
            assert!(has_stable_commit_prefix(&prev, &commits));
            batches.push((commits.len(), complete));
            prev = commits;
            true
        })
        .unwrap();

        assert_eq!(batches, vec![(2, false), (5, true)]);
        let full = prev;
        assert_eq!(full[0].message, "c4"); // newest first
        assert_eq!(full[0].author_name, "Loader Test");
        assert_eq!(full[0].parent_indices, Some(vec![1])); // parents resolved in-walk
        assert_eq!(full[4].parent_indices, Some(vec![])); // root
        assert!(!full[0].is_merge);
        assert_eq!(full[0].hash.len(), 40);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod perf_probe {
    //! Stage-cost benches, run explicitly (never in normal `cargo test`):
    //!   just bench          — synthetic fixtures, machine-independent
    //!   just bench-repo     — stages against the big test repo (if present)
    use std::time::Instant;

    const BIG_REPO: &str = "/home/arunas/dev/ariva/repos/big-dummy-repo";

    fn synthetic_commits(len: usize) -> Vec<crate::git::CommitInfo> {
        // Linear chain with a merge every 10th commit back 5 — enough lane
        // churn to exercise both engines' hot paths.
        (0..len)
            .map(|i| {
                let mut parents = if i + 1 < len { vec![i + 1] } else { vec![] };
                if i % 10 == 0 && i + 5 < len {
                    parents.push(i + 5);
                }
                let mut c = crate::git::CommitInfo::placeholder();
                c.hash = format!("{i:040x}");
                c.message = format!("commit {i} touching things");
                c.author_name = "Bench Author".into();
                c.is_merge = parents.len() > 1;
                c.parent_indices = Some(parents);
                c
            })
            .collect()
    }

    /// Engine + search throughput on 100k synthetic commits. Prints timings;
    /// fails only if wildly off the rails (10x the expected ceilings), so CI
    /// noise doesn't cause flakes but a real regression still screams.
    #[test]
    #[ignore]
    fn bench_engines_and_search_synthetic() {
        let mut commits = synthetic_commits(100_000);
        let end = commits.len();

        let t = Instant::now();
        let layout = crate::git::tree_layout::TreeLayout::build(&commits);
        let layout_ms = t.elapsed().as_millis();

        let t = Instant::now();
        let mut lanes = crate::git::tree_layout::TreeLaneEngine::new();
        lanes.extend(&mut commits, &layout, end);
        let tree_ms = t.elapsed().as_millis();

        let t = Instant::now();
        let mut columns = Vec::new();
        crate::git::graph::extend_graph_prefixes(&mut commits, 0, end, &mut columns);
        let classic_ms = t.elapsed().as_millis();

        let t = Instant::now();
        let hits = commits
            .iter()
            .filter(|c| crate::app::features::search::wildcard_or_contains(&c.message, "touching"))
            .count();
        let search_ms = t.elapsed().as_millis();

        println!(
            "bench 100k: layout={layout_ms}ms tree={tree_ms}ms classic={classic_ms}ms search={search_ms}ms ({hits} hits)"
        );
        assert!(
            layout_ms < 2_000,
            "TreeLayout::build regressed: {layout_ms}ms"
        );
        assert!(tree_ms < 2_000, "tree lanes regressed: {tree_ms}ms");
        assert!(
            classic_ms < 3_000,
            "classic lanes regressed: {classic_ms}ms"
        );
        assert!(search_ms < 1_000, "search scan regressed: {search_ms}ms");
        assert_eq!(hits, 100_000);
    }

    /// Real-repo stage costs; skipped when the big test repo is absent.
    #[test]
    #[ignore]
    fn perf_big_repo_stages() {
        let repo = std::path::Path::new(BIG_REPO);
        if !repo.is_dir() {
            eprintln!("skip: {BIG_REPO} not present");
            return;
        }
        let t = Instant::now();
        let mut commits = super::load_commits(repo, super::CommitSort::Topo).unwrap();
        println!(
            "load_commits (hybrid walk): {:?} for {} commits",
            t.elapsed(),
            commits.len()
        );

        let t = Instant::now();
        let layout = crate::git::tree_layout::TreeLayout::build(&commits);
        println!("TreeLayout::build: {:?}", t.elapsed());

        let t = Instant::now();
        let mut lanes = crate::git::tree_layout::TreeLaneEngine::new();
        let end = commits.len();
        lanes.extend(&mut commits, &layout, end);
        println!("TreeLaneEngine::extend full: {:?}", t.elapsed());

        let t = Instant::now();
        let mut columns = Vec::new();
        crate::git::graph::extend_graph_prefixes(&mut commits, 0, end, &mut columns);
        println!("classic extend full: {:?}", t.elapsed());
    }
}
