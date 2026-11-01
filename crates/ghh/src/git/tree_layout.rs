//! gitk-style commit-tree layout (`CommitListView::Focused`).
//!
//! Differences from the classic lane engine in `graph.rs`:
//! 1. **Token-ordered lanes** — a port of gitk's `ordertoken`/`idcol`
//!    (gitk:5306): commits are grouped into branch groups (same "token"),
//!    groups are ranked by a pre-order walk of the group tree, and new lanes
//!    are inserted at the rank-sorted position instead of first-free-column.
//!    Related branches stay adjacent, so lines cross far less.
//! 2. **Arrow termination** — a lane whose parent is farther than [`MIN_GAP`]
//!    rows is cut with a `↓` after [`DOWN_LEN`] rows and resumes with a `↑`
//!    [`UP_LEN`] rows before the parent (gitk `layoutrows`, gitk:5733).
//!    Graph width stays bounded without cropping.
//! 3. **Per-line colors** — the color byte follows the branch line (inherited
//!    through first-parent continuation), not the lane column
//!    (gitk `assigncolor`, gitk:6668).
//!
//! Output shape is identical to the classic engine: a prefix `String` of
//! `2 * lanes - 1` chars plus a parallel color-byte vec (255 = separator),
//! so the renderer needs no changes.

use crate::git::models::CommitInfo;

/// Cut a lane when its parent is more than this many rows away.
const MIN_GAP: usize = 50;
/// Rows a cut lane stays visible before the `↓` glyph.
const DOWN_LEN: usize = 3;
/// Rows before the parent where a cut lane resumes with `↑`.
const UP_LEN: usize = 3;
/// Number of distinct line colors (must match the renderer palette cycle).
const NUM_COLORS: u8 = 8;

fn parent_indices(commit: &CommitInfo) -> &[usize] {
    commit.parent_indices.as_deref().unwrap_or(&[])
}

// ── order ranks (gitk ordertoken port) ───────────────────────────────────────

/// Precomputed per-commit lane-ordering ranks. Build once per commit list.
pub struct TreeLayout {
    /// Rank per commit; lanes are kept sorted by the rank of the commit they
    /// are waiting for. Equal ranks = same branch group.
    ranks: Vec<u32>,
}

impl TreeLayout {
    pub fn build(commits: &[CommitInfo]) -> Self {
        let n = commits.len();
        // Invert parent edges. Scanning newest→oldest keeps children[p][0]
        // as the newest ("first real") child, matching gitk.
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (idx, commit) in commits.iter().enumerate() {
            for &p in parent_indices(commit) {
                if p < n {
                    children[p].push(idx);
                }
            }
        }

        // Group tree: a commit continues its first child's group when that
        // child has a single parent (gitk appends an empty token digit);
        // a multi-parent hop opens a new group labelled by the parent slot.
        struct Group {
            parent: Option<usize>,
            label: usize,
            seq: usize,
        }
        let mut groups: Vec<Group> = Vec::new();
        let mut gid: Vec<usize> = vec![0; n];
        for i in 0..n {
            gid[i] = match children[i].first() {
                None => {
                    // Root of a line (no children in the loaded window).
                    groups.push(Group {
                        parent: None,
                        label: 0,
                        seq: groups.len(),
                    });
                    groups.len() - 1
                }
                Some(&fk) => {
                    let fk_parents = parent_indices(&commits[fk]);
                    if fk_parents.len() == 1 {
                        gid[fk]
                    } else {
                        let slot = fk_parents.iter().position(|&p| p == i).unwrap_or(0);
                        groups.push(Group {
                            parent: Some(gid[fk]),
                            label: slot + 1,
                            seq: groups.len(),
                        });
                        groups.len() - 1
                    }
                }
            };
        }

        // Pre-order DFS over the group tree: a group ranks before its
        // labelled child groups (equal tokens sort before longer ones),
        // children visited in (label, creation) order.
        let mut kids: Vec<Vec<usize>> = vec![Vec::new(); groups.len()];
        let mut roots: Vec<usize> = Vec::new();
        for (g, group) in groups.iter().enumerate() {
            match group.parent {
                Some(p) => kids[p].push(g),
                None => roots.push(g),
            }
        }
        for list in &mut kids {
            list.sort_by_key(|&g| (groups[g].label, groups[g].seq));
        }
        roots.sort_by_key(|&g| groups[g].seq);

        let mut group_rank: Vec<u32> = vec![0; groups.len()];
        let mut counter: u32 = 0;
        let mut stack: Vec<usize> = roots.into_iter().rev().collect();
        while let Some(g) = stack.pop() {
            group_rank[g] = counter;
            counter += 1;
            for &child in kids[g].iter().rev() {
                stack.push(child);
            }
        }

        let ranks = gid.into_iter().map(|g| group_rank[g]).collect();
        Self { ranks }
    }

    fn rank(&self, commit_idx: usize) -> u32 {
        self.ranks.get(commit_idx).copied().unwrap_or(u32::MAX)
    }
}

// ── lane engine ──────────────────────────────────────────────────────────────

struct Lane {
    /// Row (in layout space) where this line meets its commit.
    target_row: usize,
    /// Commit index of that target — rank source for sorted insertion.
    target_commit: usize,
    color: u8,
    /// Emit `↑` on the row the lane (re)appears.
    resumed: bool,
    /// `Some(row)`: cut lane — emit `↓` at `row`, then drop.
    stub_until: Option<usize>,
    // Per-row transient render flags.
    is_commit: bool,
    merge_here: bool,
}

/// Incremental gitk-style lane state; feed rows forward only.
pub struct TreeLaneEngine {
    lanes: Vec<Lane>,
    /// Cut lanes waiting to resume: (resume_row, target_row, target_commit, color).
    pending_resume: Vec<(usize, usize, usize, u8)>,
    next_color: u8,
    computed_until: usize,
}

impl Default for TreeLaneEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TreeLaneEngine {
    pub fn new() -> Self {
        Self {
            lanes: Vec::new(),
            pending_resume: Vec::new(),
            next_color: 0,
            computed_until: 0,
        }
    }

    pub fn computed_until(&self) -> usize {
        self.computed_until
    }

    /// Extend prefixes for `computed_until..end` (uncollapsed: row == commit
    /// index). Writes `graph_prefix`/`graph_prefix_lanes` on the commits.
    pub fn extend(&mut self, commits: &mut [CommitInfo], layout: &TreeLayout, end: usize) {
        let end = end.min(commits.len());
        // Index loop: each iteration reads commits[row], mutates self, then
        // writes commits[row] — iterator form conflicts with the self borrow.
        #[allow(clippy::needless_range_loop)]
        for row in self.computed_until..end {
            let parents: Vec<(usize, usize)> = parent_indices(&commits[row])
                .iter()
                .map(|&p| (p, p))
                .collect();
            let is_merge = commits[row].is_merge;
            let (prefix, lanes) = self.advance_row(row, row, is_merge, &parents, layout);
            let (mut prefix, mut lanes) = (prefix, lanes);
            crate::git::graph::cap_graph_prefix(&mut prefix, &mut lanes);
            commits[row].graph_prefix = prefix;
            commits[row].graph_prefix_lanes = lanes;
        }
        self.computed_until = end;
    }

    /// Pick a color for a new line: cycle the palette, avoid the colors of
    /// the would-be neighbours (cheap version of gitk `findcrossings`).
    fn pick_color(&mut self, insert_pos: usize) -> u8 {
        let left = insert_pos
            .checked_sub(1)
            .and_then(|i| self.lanes.get(i))
            .map(|l| l.color);
        let right = self.lanes.get(insert_pos).map(|l| l.color);
        for _ in 0..NUM_COLORS {
            let c = self.next_color;
            self.next_color = (self.next_color + 1) % NUM_COLORS;
            if Some(c) != left && Some(c) != right {
                return c;
            }
        }
        0
    }

    /// gitk `idcol`: position keeping lane ranks non-decreasing left→right.
    fn insert_pos(&self, layout: &TreeLayout, target_commit: usize) -> usize {
        let rank = layout.rank(target_commit);
        self.lanes
            .partition_point(|lane| layout.rank(lane.target_commit) <= rank)
    }

    /// Lay out one row. `parents` = (target_row, target_commit) pairs, first
    /// parent first. Returns the rendered prefix + color bytes.
    fn advance_row(
        &mut self,
        row: usize,
        commit_idx: usize,
        is_merge: bool,
        parents: &[(usize, usize)],
        layout: &TreeLayout,
    ) -> (String, Vec<u8>) {
        // 1. Resume cut lanes that are due.
        let mut due: Vec<(usize, usize, usize, u8)> = Vec::new();
        self.pending_resume.retain(|&entry| {
            if entry.0 <= row {
                due.push(entry);
                false
            } else {
                true
            }
        });
        for (_, target_row, target_commit, color) in due {
            if target_row <= row {
                continue; // resumed too late (window edge); parent row handles it
            }
            let pos = self.insert_pos(layout, target_commit);
            self.lanes.insert(
                pos,
                Lane {
                    target_row,
                    target_commit,
                    color,
                    resumed: true,
                    stub_until: None,
                    is_commit: false,
                    merge_here: false,
                },
            );
        }

        // 2. Commit lane: first lane expecting this row, extras converge away.
        let expecting: Vec<usize> = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| l.target_row == row)
            .map(|(i, _)| i)
            .collect();
        let commit_pos = match expecting.first() {
            Some(&pos) => {
                for &extra in expecting.iter().skip(1).rev() {
                    self.lanes.remove(extra);
                }
                pos
            }
            None => {
                let pos = self.insert_pos(layout, commit_idx);
                let color = self.pick_color(pos);
                self.lanes.insert(
                    pos,
                    Lane {
                        target_row: row,
                        target_commit: commit_idx,
                        color,
                        resumed: false,
                        stub_until: None,
                        is_commit: false,
                        merge_here: false,
                    },
                );
                pos
            }
        };
        self.lanes[commit_pos].is_commit = true;

        // 3. Extra parents (merge sources): join an existing lane or open a
        //    new one at the rank-sorted position; cut immediately if far.
        for &(target_row, target_commit) in parents.iter().skip(1) {
            if let Some(lane) = self
                .lanes
                .iter_mut()
                .find(|l| l.target_row == target_row && !l.is_commit)
            {
                lane.merge_here = true;
                continue;
            }
            let pos = self.insert_pos(layout, target_commit);
            let color = self.pick_color(pos);
            let stub_until = (target_row.saturating_sub(row) > MIN_GAP).then(|| {
                self.pending_resume.push((
                    target_row.saturating_sub(UP_LEN),
                    target_row,
                    target_commit,
                    color,
                ));
                row + DOWN_LEN
            });
            self.lanes.insert(
                pos,
                Lane {
                    target_row,
                    target_commit,
                    color,
                    resumed: false,
                    stub_until,
                    is_commit: false,
                    merge_here: true,
                },
            );
        }

        // 4. Render.
        let commit_pos = self.lanes.iter().position(|l| l.is_commit).unwrap_or(0);
        let left_merge = self.lanes[..commit_pos].iter().position(|l| l.merge_here);
        let right_merge = self
            .lanes
            .iter()
            .rposition(|l| l.merge_here)
            .filter(|&p| p > commit_pos);
        let num_lanes = self.lanes.len();
        let mut prefix = String::with_capacity(num_lanes * 2);
        let mut bytes: Vec<u8> = Vec::with_capacity(num_lanes * 2);
        for (pos, lane) in self.lanes.iter().enumerate() {
            let (ch, color) = if lane.is_commit {
                (if is_merge { 'M' } else { '*' }, lane.color)
            } else if lane.merge_here {
                (if pos < commit_pos { '┌' } else { '┐' }, lane.color)
            } else if lane.resumed {
                ('↑', lane.color)
            } else if lane.stub_until == Some(row) {
                ('↓', lane.color)
            } else if left_merge.is_some_and(|l| pos > l && pos < commit_pos)
                || right_merge.is_some_and(|r| pos > commit_pos && pos < r)
            {
                ('─', self.lanes[commit_pos].color)
            } else {
                ('│', lane.color)
            };
            prefix.push(ch);
            bytes.push(color);
            if pos + 1 < num_lanes {
                prefix.push(' ');
                bytes.push(255);
            }
        }

        // 5. Post-row bookkeeping: continue the commit line into its first
        //    parent (color inherited), drop finished/cut lanes, clear flags.
        let mut idx = 0;
        self.lanes.retain_mut(|lane| {
            let keep = if lane.is_commit {
                match parents.first() {
                    Some(&(target_row, target_commit)) => {
                        lane.target_row = target_row;
                        lane.target_commit = target_commit;
                        if target_row.saturating_sub(row) > MIN_GAP {
                            self.pending_resume.push((
                                target_row.saturating_sub(UP_LEN),
                                target_row,
                                target_commit,
                                lane.color,
                            ));
                            lane.stub_until = Some(row + DOWN_LEN);
                        }
                        true
                    }
                    None => false, // root: line ends here
                }
            } else {
                lane.stub_until != Some(row)
            };
            lane.is_commit = false;
            lane.merge_here = false;
            lane.resumed = false;
            idx += 1;
            keep
        });

        (prefix, bytes)
    }
}

// ── collapse-aware windowed layout ───────────────────────────────────────────

/// Resolve a commit to itself or its nearest visible first-parent ancestor.
/// Mirrors `graph.rs::nearest_visible_idx`. -1 = none.
fn nearest_visible(
    commits: &[CommitInfo],
    hidden: &[bool],
    idx: usize,
    memo: &mut [isize],
) -> isize {
    let mut chain: Vec<usize> = Vec::new();
    let mut current = idx as isize;
    let result = loop {
        if current < 0 || current as usize >= commits.len() {
            break -1;
        }
        let cur = current as usize;
        if memo[cur] != -2 {
            break memo[cur];
        }
        if !hidden.get(cur).copied().unwrap_or(false) {
            break cur as isize;
        }
        chain.push(cur);
        if chain.len() > 500 {
            break -1;
        }
        current = parent_indices(&commits[cur])
            .first()
            .map(|&p| p as isize)
            .unwrap_or(-1);
    };
    for c in chain {
        memo[c] = result;
    }
    result
}

/// Stateless windowed layout for collapsed mode: walks the visible commit
/// sequence from the top, calls `on_visible(commit_idx, prefix, lanes)` for
/// rows inside `visible_start..visible_end`. Gap distances are measured in
/// visible rows. Same contract as `graph.rs::for_each_visible_prefix_in_window`.
pub fn tree_prefixes_in_window<F>(
    commits: &[CommitInfo],
    layout: &TreeLayout,
    hidden_flags: &[bool],
    visible_start: usize,
    visible_end: usize,
    mut on_visible: F,
) where
    F: FnMut(usize, String, Vec<u8>),
{
    // Visible sequence + commit → row map.
    let mut rows: Vec<usize> = Vec::new();
    let mut row_of: Vec<usize> = vec![usize::MAX; commits.len()];
    for (idx, _) in commits.iter().enumerate() {
        if !hidden_flags.get(idx).copied().unwrap_or(false) {
            row_of[idx] = rows.len();
            rows.push(idx);
        }
    }

    let mut memo: Vec<isize> = vec![-2; commits.len()];
    let mut engine = TreeLaneEngine::new();
    let walk_end = visible_end.min(rows.len());
    for (row, &commit_idx) in rows.iter().enumerate().take(walk_end) {
        // Parent edges through hidden commits resolve to the nearest visible
        // ancestor; edges landing on already-drawn rows or nowhere drop out.
        let mut parents: Vec<(usize, usize)> = Vec::new();
        for &p in parent_indices(&commits[commit_idx]) {
            let visible = nearest_visible(commits, hidden_flags, p, &mut memo);
            if visible >= 0 {
                let target = visible as usize;
                let target_row = row_of[target];
                if target_row != usize::MAX
                    && target_row > row
                    && !parents.iter().any(|&(r, _)| r == target_row)
                {
                    parents.push((target_row, target));
                }
            }
        }
        let is_merge = commits[commit_idx].is_merge;
        let (prefix, lanes) = engine.advance_row(row, commit_idx, is_merge, &parents, layout);
        if row >= visible_start {
            let (mut prefix, mut lanes) = (prefix, lanes);
            crate::git::graph::cap_graph_prefix(&mut prefix, &mut lanes);
            on_visible(commit_idx, prefix, lanes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal commit for layout tests; `parents` are indices.
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

    fn layout_all(commits: &mut [CommitInfo]) -> Vec<String> {
        let layout = TreeLayout::build(commits);
        let mut engine = TreeLaneEngine::new();
        let end = commits.len();
        engine.extend(commits, &layout, end);
        commits.iter().map(|c| c.graph_prefix.clone()).collect()
    }

    #[test]
    fn drill_mask_still_draws_subgraph_lanes() {
        // Drill-down view: everything hidden except the side branch of merge 1,
        // whose head 3 is itself a sub-merge of 4 and 5.
        let commits = vec![
            commit(vec![1]),    // 0 tip
            commit(vec![2, 3]), // 1 drilled merge
            commit(vec![7]),    // 2 main
            commit(vec![4, 5]), // 3 side head, sub-merge
            commit(vec![6]),    // 4 side A
            commit(vec![6]),    // 5 side B
            commit(vec![7]),    // 6 side base
            commit(vec![]),     // 7 root
        ];
        let layout = TreeLayout::build(&commits);
        let mut hidden = vec![true; commits.len()];
        for i in [3, 4, 5, 6] {
            hidden[i] = false;
        }
        let mut prefixes = Vec::new();
        tree_prefixes_in_window(&commits, &layout, &hidden, 0, 4, |idx, prefix, _| {
            prefixes.push((idx, prefix));
        });
        let merge_prefix = &prefixes.iter().find(|(i, _)| *i == 3).unwrap().1;
        assert!(
            merge_prefix.contains('\u{2510}'),
            "sub-merge must open a second lane, got {prefixes:?}"
        );
    }

    #[test]
    fn linear_history_single_lane_single_color() {
        let mut commits = vec![commit(vec![1]), commit(vec![2]), commit(vec![])];
        let prefixes = layout_all(&mut commits);
        assert_eq!(prefixes, vec!["*", "*", "*"]);
        let colors: Vec<u8> = commits.iter().map(|c| c.graph_prefix_lanes[0]).collect();
        assert_eq!(colors, vec![colors[0]; 3], "line keeps one color");
    }

    #[test]
    fn simple_merge_draws_connector_and_distinct_color() {
        // 0 = merge of 1 (main) and 2 (side); both parented on 3.
        let mut commits = vec![
            commit(vec![1, 2]),
            commit(vec![3]),
            commit(vec![3]),
            commit(vec![]),
        ];
        let prefixes = layout_all(&mut commits);
        assert_eq!(prefixes[0], "M ┐");
        assert_eq!(prefixes[1], "* │");
        assert_eq!(prefixes[2], "│ *");
        assert_eq!(prefixes[3], "*");
        let main_color = commits[0].graph_prefix_lanes[0];
        let side_color = commits[0].graph_prefix_lanes[2];
        assert_ne!(main_color, side_color, "side branch gets its own color");
        assert_eq!(commits[2].graph_prefix_lanes[2], side_color);
    }

    #[test]
    fn rank_order_keeps_branches_adjacent() {
        // Two merges on the main line; each side branch is 2 commits long.
        // main: 0-1-3-5-7(root); side A (from 0): 2,4? — build explicit:
        // 0 = M(1, 2), 1 = M(3, 4), 2→6, 3→6? Use clear shape:
        // 0 M(1,4)  1 M(2,5)  2→3  3→6  4→6  5→6  6 root
        let mut commits = vec![
            commit(vec![1, 4]),
            commit(vec![2, 5]),
            commit(vec![3]),
            commit(vec![6]),
            commit(vec![6]),
            commit(vec![6]),
            commit(vec![]),
        ];
        let layout = TreeLayout::build(&commits);
        // gitk semantics: every merge hop extends the token for BOTH parent
        // slots (first parent gets digit 0), so the main line's rank grows at
        // each merge, and 2/3/6 (single-parent chain) share one rank.
        assert_eq!(layout.rank(2), layout.rank(3));
        assert_eq!(layout.rank(3), layout.rank(6));
        assert!(layout.rank(0) < layout.rank(1));
        assert!(layout.rank(1) < layout.rank(2));
        // Side branches nest: the deeper fork (5, token d0+d1) sorts before
        // the outer one (4, token d1) — lanes [main, 5, 4].
        assert!(layout.rank(5) < layout.rank(4));
        assert!(layout.rank(2) < layout.rank(5));
        let prefixes = layout_all(&mut commits);
        // Every row at most 3 lanes; side lanes sit right of main lane.
        for p in &prefixes {
            assert!(p.chars().filter(|c| *c != ' ').count() <= 3, "{p:?}");
            assert!(
                p.starts_with(['*', 'M', '│']),
                "main line stays lane 0: {p:?}"
            );
        }
    }

    #[test]
    fn long_gap_cuts_lane_with_arrows() {
        // 0 = merge of 1 and LAST (far away root). Chain 1..=n-1, n-1 root... build:
        // commits 0..=59: 0 = M(1, 59); i in 1..59 → i+1; 59 root.
        let mut commits: Vec<CommitInfo> = Vec::new();
        commits.push(commit(vec![1, 59]));
        for i in 1..59 {
            commits.push(commit(vec![i + 1]));
        }
        commits.push(commit(vec![]));
        let prefixes = layout_all(&mut commits);
        assert_eq!(prefixes[0], "M ┐", "merge row opens side lane");
        // Stub lives DOWN_LEN rows then ↓.
        assert_eq!(prefixes[DOWN_LEN], "* ↓", "cut after DOWN_LEN rows");
        // Middle rows: side lane gone.
        assert_eq!(prefixes[10], "*", "lane absent mid-gap");
        // Resume UP_LEN before parent row 59.
        assert_eq!(prefixes[59 - UP_LEN], "* ↑", "resume with up arrow");
        assert_eq!(prefixes[58], "* │");
        assert_eq!(prefixes[59], "*", "lines converge on parent");
        // Cut and resumed lane keep the same color.
        let cut_color = commits[DOWN_LEN].graph_prefix_lanes[2];
        let resume_color = commits[59 - UP_LEN].graph_prefix_lanes[2];
        assert_eq!(cut_color, resume_color);
    }

    #[test]
    fn windowed_collapse_resolves_hidden_parents() {
        // 0 = M(1, 2); 1→3; 2→3 (side, hidden); 3 root.
        let commits = vec![
            commit(vec![1, 2]),
            commit(vec![3]),
            commit(vec![3]),
            commit(vec![]),
        ];
        let layout = TreeLayout::build(&commits);
        let hidden = vec![false, false, true, false];
        let mut out: Vec<(usize, String)> = Vec::new();
        tree_prefixes_in_window(&commits, &layout, &hidden, 0, 4, |idx, prefix, _| {
            out.push((idx, prefix));
        });
        // Hidden side parent resolves through first-parent chain to 3.
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].0, 0);
        assert_eq!(out[2], (3, "*".to_string()), "lines converge on 3");
    }

    #[test]
    fn window_range_limits_callback_not_walk() {
        let commits = vec![commit(vec![1]), commit(vec![2]), commit(vec![])];
        let layout = TreeLayout::build(&commits);
        let hidden = vec![false; 3];
        let mut seen: Vec<usize> = Vec::new();
        tree_prefixes_in_window(&commits, &layout, &hidden, 1, 3, |idx, _, _| {
            seen.push(idx);
        });
        assert_eq!(seen, vec![1, 2]);
    }
}
