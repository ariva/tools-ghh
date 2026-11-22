use crate::git::models::CommitInfo;
use std::collections::HashMap;

fn parent_indices(commit: &CommitInfo) -> &[usize] {
    commit.parent_indices.as_deref().unwrap_or(&[])
}

#[derive(Default)]
struct LaneScratch {
    old_occupied: Vec<bool>,
    expecting: Vec<usize>,
    merge_lanes: Vec<usize>,
}

fn render_and_advance_lanes(
    columns: &mut Vec<Option<usize>>,
    commit_idx: usize,
    is_merge: bool,
    resolved_parents: &[usize],
    scratch: &mut LaneScratch,
) -> (String, Vec<u8>) {
    scratch.old_occupied.clear();
    scratch
        .old_occupied
        .extend(columns.iter().map(Option::is_some));
    scratch.expecting.clear();
    for (idx, lane) in columns.iter().enumerate() {
        if *lane == Some(commit_idx) {
            scratch.expecting.push(idx);
        }
    }

    let commit_lane = if scratch.expecting.is_empty() {
        if let Some(pos) = columns.iter().position(|h| h.is_none()) {
            pos
        } else {
            columns.push(None);
            columns.len() - 1
        }
    } else {
        scratch.expecting[0]
    };

    for &lane in scratch.expecting.get(1..).unwrap_or(&[]) {
        columns[lane] = None;
    }

    while columns.len() <= commit_lane {
        columns.push(None);
    }
    columns[commit_lane] = resolved_parents.first().copied();

    scratch.merge_lanes.clear();
    for &parent in resolved_parents.iter().skip(1) {
        let lane = if let Some(pos) = columns.iter().position(|h| *h == Some(parent)) {
            pos
        } else if let Some(pos) = columns.iter().position(|h| h.is_none()) {
            columns[pos] = Some(parent);
            pos
        } else {
            columns.push(Some(parent));
            columns.len() - 1
        };
        if lane != commit_lane && !scratch.merge_lanes.contains(&lane) {
            scratch.merge_lanes.push(lane);
        }
    }

    let num_lanes = columns
        .len()
        .max(scratch.old_occupied.len())
        .max(commit_lane + 1);
    let left_merge = scratch
        .merge_lanes
        .iter()
        .copied()
        .filter(|&lane| lane < commit_lane)
        .min();
    let right_merge = scratch
        .merge_lanes
        .iter()
        .copied()
        .filter(|&lane| lane > commit_lane)
        .max();

    let mut prefix = String::with_capacity(num_lanes.saturating_mul(2).saturating_sub(1));
    let mut lanes: Vec<u8> = Vec::with_capacity(num_lanes.saturating_mul(2).saturating_sub(1));
    for lane in 0..num_lanes {
        let active_before = scratch.old_occupied.get(lane).copied().unwrap_or(false)
            && !scratch.expecting.contains(&lane);
        let active_after = columns.get(lane).and_then(|h| h.as_ref()).is_some();
        let merge_here = scratch.merge_lanes.contains(&lane);
        let between_left = left_merge
            .map(|left| lane > left && lane < commit_lane)
            .unwrap_or(false);
        let between_right = right_merge
            .map(|right| lane > commit_lane && lane < right)
            .unwrap_or(false);

        let (ch, color_lane) = if lane == commit_lane {
            // 'M' for every merge, even when its side lanes are hidden (e.g.
            // drill view) — the glyph marks collapsibility, like TreeEngine.
            (if is_merge { 'M' } else { '*' }, commit_lane.min(254) as u8)
        } else if merge_here {
            (
                if lane < commit_lane { '┌' } else { '┐' },
                lane.min(254) as u8,
            )
        } else if between_left || between_right {
            ('─', commit_lane.min(254) as u8)
        } else if active_before || active_after {
            ('│', lane.min(254) as u8)
        } else {
            (' ', 255u8)
        };

        prefix.push(ch);
        lanes.push(color_lane);
        if lane < num_lanes - 1 {
            prefix.push(' ');
            lanes.push(255u8);
        }
    }

    while columns.last() == Some(&None) {
        columns.pop();
    }

    (prefix, lanes)
}

/// Compute a `graph_prefix` string for each commit representing the ASCII graph
/// using lane tracking (columns). Each lane is one active branch.
#[allow(dead_code)]
pub fn compute_graph_prefixes(commits: &mut [CommitInfo]) {
    let mut columns: Vec<Option<usize>> = Vec::new();
    let len = commits.len();
    extend_graph_prefixes(commits, 0, len, &mut columns);
}

/// Cap stored/rendered graph width. Terminals can't usefully show more than
/// ~32 lanes, but wide repos produce prefixes of 300+ lanes; storing them all
/// cost ~250 MB on a 568k-commit repo. The engines' internal lane state stays
/// full-width, so topology past the cap remains correct.
pub(crate) const MAX_GRAPH_PREFIX_CHARS: usize = 64;

pub(crate) fn cap_graph_prefix(prefix: &mut String, lanes: &mut Vec<u8>) {
    if lanes.len() > MAX_GRAPH_PREFIX_CHARS {
        lanes.truncate(MAX_GRAPH_PREFIX_CHARS);
        *prefix = prefix.chars().take(MAX_GRAPH_PREFIX_CHARS).collect();
    }
    // Push-grown buffers carry ~2x slack; these live for the whole session,
    // one pair per commit.
    lanes.shrink_to_fit();
    prefix.shrink_to_fit();
}

pub fn extend_graph_prefixes(
    commits: &mut [CommitInfo],
    start: usize,
    end: usize,
    columns: &mut Vec<Option<usize>>,
) {
    let mut scratch = LaneScratch::default();
    extend_graph_prefixes_with_scratch(commits, start, end, columns, &mut scratch);
}

fn extend_graph_prefixes_with_scratch(
    commits: &mut [CommitInfo],
    start: usize,
    end: usize,
    columns: &mut Vec<Option<usize>>,
    scratch: &mut LaneScratch,
) {
    let end = end.min(commits.len());
    for (idx, commit) in commits.iter_mut().enumerate().take(end).skip(start) {
        let (prefix, lanes) = render_and_advance_lanes(
            columns,
            idx,
            commit.is_merge,
            parent_indices(commit),
            scratch,
        );
        let (mut prefix, mut lanes) = (prefix, lanes);
        cap_graph_prefix(&mut prefix, &mut lanes);
        commit.graph_prefix = prefix;
        commit.graph_prefix_lanes = lanes;
    }
}

/// Returns indices of commits that form the side branch of a merge commit:
/// ancestors of the secondary parent(s) that are NOT ancestors of the primary
/// parent — i.e., commits unique to the merged branch.
///
/// Uses an interleaved paint-down walk (like git merge-base): both ancestries
/// advance together in ascending-index order and the walk stops once every
/// unresolved commit is already reached from the primary side. Cost is
/// proportional to the branch region, not repo history — the previous
/// full-ancestry walk froze the UI on repos with 100k+ merges when the drill
/// closure called this per sub-merge. Relies on the list being topologically
/// ordered (parents after children), which `git log` guarantees for both
/// --topo-order and --date-order.
pub fn find_branch_commit_indices(commits: &[CommitInfo], merge_idx: usize) -> Vec<usize> {
    const PRIMARY: u8 = 1;
    const SECONDARY: u8 = 2;

    let Some(merge) = commits.get(merge_idx) else {
        return vec![];
    };
    let parents = parent_indices(merge);
    if parents.len() < 2 {
        return vec![];
    }

    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let mut flags: HashMap<usize, u8> = HashMap::new();
    let mut heap: BinaryHeap<Reverse<usize>> = BinaryHeap::new();
    // Unpopped commits reached only from the secondary side; 0 → done.
    let mut pending = 0usize;

    let bump = |flags: &mut HashMap<usize, u8>,
                heap: &mut BinaryHeap<Reverse<usize>>,
                pending: &mut usize,
                idx: usize,
                add: u8| {
        let entry = flags.entry(idx).or_insert(0);
        let old = *entry;
        let new = old | add;
        if new == old {
            return;
        }
        *entry = new;
        if old == 0 {
            heap.push(Reverse(idx));
            if new & PRIMARY == 0 {
                *pending += 1;
            }
        } else if old & PRIMARY == 0 && new & PRIMARY != 0 {
            *pending -= 1;
        }
    };

    bump(&mut flags, &mut heap, &mut pending, parents[0], PRIMARY);
    for &p in &parents[1..] {
        bump(&mut flags, &mut heap, &mut pending, p, SECONDARY);
    }

    let mut branch: Vec<usize> = Vec::new();
    while pending > 0 {
        let Some(Reverse(idx)) = heap.pop() else {
            break;
        };
        let f = flags[&idx];
        if f & PRIMARY == 0 {
            pending -= 1;
            branch.push(idx);
        }
        if let Some(c) = commits.get(idx) {
            for &p in parent_indices(c) {
                bump(&mut flags, &mut heap, &mut pending, p, f);
            }
        }
    }

    branch
}

pub fn for_each_visible_prefix_in_window<F>(
    commits: &[CommitInfo],
    hidden_flags: &[bool],
    visible_start: usize,
    visible_end: usize,
    mut on_visible: F,
) where
    F: FnMut(usize, String, Vec<u8>),
{
    let mut nearest_visible_cache: Vec<isize> = vec![-2; commits.len()];

    fn nearest_visible_idx(
        idx: usize,
        commits: &[CommitInfo],
        hidden_flags: &[bool],
        cache: &mut [isize],
        depth: u32,
    ) -> Option<usize> {
        if depth > 500 {
            return None;
        }
        match cache[idx] {
            -2 => {}
            -1 => return None,
            cached => return Some(cached as usize),
        }
        if !hidden_flags[idx] {
            cache[idx] = idx as isize;
            return Some(idx);
        }
        let c = commits.get(idx)?;
        let resolved = nearest_visible_idx(
            *parent_indices(c).first()?,
            commits,
            hidden_flags,
            cache,
            depth + 1,
        );
        cache[idx] = resolved.map(|i| i as isize).unwrap_or(-1);
        resolved
    }

    let mut columns: Vec<Option<usize>> = Vec::new();
    let mut scratch = LaneScratch::default();
    let mut visible_pos = 0usize;

    for (idx, commit) in commits.iter().enumerate() {
        // Hidden commits must not touch the lane state: parent edges of
        // visible commits already resolve past them (nearest_visible_idx),
        // and feeding them in leaves stale lanes that widen visible rows
        // (e.g. a drill scope with a large hidden gap between its rows).
        if hidden_flags.get(idx).copied().unwrap_or(false) {
            continue;
        }
        let resolved_parents: Vec<usize> = parent_indices(commit)
            .iter()
            .filter_map(|&parent_idx| {
                nearest_visible_idx(
                    parent_idx,
                    commits,
                    hidden_flags,
                    &mut nearest_visible_cache,
                    0,
                )
            })
            .collect();
        let (prefix, lanes) = render_and_advance_lanes(
            &mut columns,
            idx,
            commit.is_merge,
            &resolved_parents,
            &mut scratch,
        );

        if visible_pos >= visible_start && visible_pos < visible_end {
            let (mut prefix, mut lanes) = (prefix, lanes);
            cap_graph_prefix(&mut prefix, &mut lanes);
            on_visible(idx, prefix, lanes);
        }
        visible_pos += 1;
        if visible_pos >= visible_end {
            break;
        }
    }
}

/// For every commit, the mainline commit that first "contains" it: walk the
/// first-parent chain from `head_idx` (that is the mainline as merges see
/// it), then flood-fill ancestors from each chain commit OLDEST-first through
/// all parents, stamping unstamped commits. The stamp is the merge commit
/// that brought the commit onto the mainline — for GitHub-style flows, the
/// "Merge pull request #N" commit. `u32::MAX` = on the mainline itself
/// (committed directly). O(commits + edges), one pass, cache the result.
pub fn compute_merge_points(commits: &[crate::git::CommitInfo], head_idx: usize) -> Vec<u32> {
    const DIRECT: u32 = u32::MAX;
    let n = commits.len();
    let mut merged_via = vec![0u32; n];
    let mut stamped = vec![false; n];
    if n == 0 || head_idx >= n {
        return merged_via;
    }

    // First-parent chain from HEAD = the mainline.
    let mut chain = Vec::new();
    let mut on_chain = vec![false; n];
    let mut cursor = Some(head_idx);
    while let Some(idx) = cursor {
        if on_chain[idx] {
            break; // corrupt topology guard
        }
        on_chain[idx] = true;
        merged_via[idx] = DIRECT;
        stamped[idx] = true;
        chain.push(idx);
        cursor = commits[idx]
            .parent_indices
            .as_ref()
            .and_then(|parents| parents.first().copied());
    }

    // Oldest mainline point first: ancestors reached from an older point were
    // merged there; later chain commits must not re-stamp them.
    let mut stack: Vec<usize> = Vec::new();
    for &chain_idx in chain.iter().rev() {
        let Some(parents) = commits[chain_idx].parent_indices.as_ref() else {
            continue;
        };
        stack.extend(parents.iter().copied().filter(|&p| !stamped[p]));
        while let Some(idx) = stack.pop() {
            if stamped[idx] {
                continue;
            }
            stamped[idx] = true;
            merged_via[idx] = chain_idx as u32;
            if let Some(parents) = commits[idx].parent_indices.as_ref() {
                stack.extend(parents.iter().copied().filter(|&p| !stamped[p]));
            }
        }
    }
    merged_via
}

#[cfg(test)]
mod tests {
    use super::{
        compute_merge_points, find_branch_commit_indices, for_each_visible_prefix_in_window,
    };
    use crate::git::models::CommitInfo;

    fn commit(parents: Vec<usize>) -> CommitInfo {
        let is_merge = parents.len() > 1;
        CommitInfo {
            hash: String::new(),
            message: String::new(),
            full_message: None,
            author_name: String::new(),
            author_email: String::new(),
            timestamp: 0,
            offset_minutes: 0,
            parent_indices: Some(parents),
            is_merge,
            graph_prefix: String::new(),
            graph_prefix_lanes: Vec::new(),
        }
    }

    #[test]
    fn branch_indices_unique_to_merged_branch() {
        // 0 = merge(1, 2); branch: 2 -> 3, both off mainline 1 -> 4 -> 5.
        let commits = vec![
            commit(vec![1, 2]),
            commit(vec![4]),
            commit(vec![3]),
            commit(vec![4]),
            commit(vec![5]),
            commit(vec![]),
        ];
        assert_eq!(find_branch_commit_indices(&commits, 0), vec![2, 3]);
        // Non-merge and out-of-range: empty.
        assert_eq!(find_branch_commit_indices(&commits, 1), Vec::<usize>::new());
        assert_eq!(
            find_branch_commit_indices(&commits, 99),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn branch_indices_exclude_commits_reachable_from_primary() {
        // 0 = merge(1, 2); secondary 2's parent 3 is also on the mainline
        // (1 -> 3), so only 2 is unique to the branch.
        let commits = vec![
            commit(vec![1, 2]),
            commit(vec![3]),
            commit(vec![3]),
            commit(vec![]),
        ];
        assert_eq!(find_branch_commit_indices(&commits, 0), vec![2]);
    }

    #[test]
    fn branch_indices_empty_when_branch_already_merged() {
        // 0 = merge(1, 2) where 2 is an ancestor of 1 — nothing unique.
        let commits = vec![commit(vec![1, 2]), commit(vec![2]), commit(vec![])];
        assert_eq!(find_branch_commit_indices(&commits, 0), Vec::<usize>::new());
    }

    #[test]
    fn hidden_gap_does_not_widen_visible_rows() {
        // Drill-like mask: 0 and 1 visible, a hidden linear run 2..=5, then 6
        // visible. 1's parent resolves through the gap to 6.
        let commits = vec![
            commit(vec![1]),
            commit(vec![2]),
            commit(vec![3]),
            commit(vec![4]),
            commit(vec![5]),
            commit(vec![6]),
            commit(vec![]),
        ];
        let hidden = vec![false, false, true, true, true, true, false];
        let mut out: Vec<(usize, String)> = Vec::new();
        for_each_visible_prefix_in_window(&commits, &hidden, 0, 3, |idx, prefix, _| {
            out.push((idx, prefix));
        });
        assert_eq!(out.len(), 3);
        // A single line, no stale lanes from the hidden run.
        for (idx, prefix) in &out {
            assert_eq!(prefix, "*", "row {idx} must stay single-lane");
        }
    }

    /// Topology (index = row, HEAD = 0, first parent listed first):
    ///   0 M3 -> [1]        mainline
    ///   1 M2 -> [4, 2]     merge of PR branch (2,3)
    ///   2 pr  -> [3]
    ///   3 pr  -> [4]
    ///   4 M1 -> [6, 5]     merge of older PR (5)
    ///   5 old -> [6]
    ///   6 root
    #[test]
    fn merge_points_find_the_pr_merge() {
        let commits = vec![
            commit(vec![1]),
            commit(vec![4, 2]),
            commit(vec![3]),
            commit(vec![4]),
            commit(vec![6, 5]),
            commit(vec![6]),
            commit(vec![]),
        ];
        let mp = compute_merge_points(&commits, 0);
        const DIRECT: u32 = u32::MAX;
        assert_eq!(mp[0], DIRECT); // mainline
        assert_eq!(mp[1], DIRECT);
        assert_eq!(mp[4], DIRECT);
        assert_eq!(mp[6], DIRECT);
        assert_eq!(mp[2], 1); // PR commits -> merged via M2
        assert_eq!(mp[3], 1);
        assert_eq!(mp[5], 4); // older PR -> merged via M1, NOT re-stamped by M2
    }
}
