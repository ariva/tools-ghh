//! Commit-list behaviour: selection movement, lazy per-commit stats/diff
//! loading, merge-branch collapsing, and graph-prefix computation.

use crate::app::App;
use crate::app::models::{CommitListFocus, CommitListView, VisibleRow, parse_diff_files};
use crate::events::{ActionKind, AppEvent, EventTime};
use crate::git;
use crate::git::GraphEngine;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

const SELECTED_STATS_DEBOUNCE: Duration = Duration::from_millis(400);

/// One drill-down level into a collapsed branch ("cd" into its hidden
/// commits). `saved_selected` + `saved_offset` restore the outer view on
/// Backspace exactly as it was — same cursor, same scroll position.
pub struct DrillFrame {
    pub merge_hash: String,
    pub count: usize,
    pub saved_selected: Option<usize>,
    pub saved_offset: usize,
}

/// Row index of the collapsed-branch placeholder for `merge_hash`, if shown.
fn collapsed_row_position(visible: &[VisibleRow], merge_hash: &str) -> Option<usize> {
    visible.iter().position(
        |row| matches!(row, VisibleRow::CollapsedBranch { merge_hash: h, .. } if h == merge_hash),
    )
}

impl App {
    pub fn selected_commit(&self) -> Option<&git::CommitInfo> {
        self.list_state
            .selected()
            .and_then(|i| match self.visible.get(i)? {
                VisibleRow::Commit { idx, .. } | VisibleRow::DrillHeader { idx, .. } => {
                    self.commits.get(self.display_idx(*idx))
                }
                VisibleRow::CollapsedBranch { .. } | VisibleRow::WorkingTree => None,
            })
    }

    pub fn load_selected_stats(&mut self) {
        if self.working_tree_selected() && self.show_commit_diff {
            self.load_working_tree_diff();
        }
        if let Some(commit) = self.selected_commit() {
            let hash = commit.hash.clone();
            let needs_email = commit.author_email.is_empty();
            let needs_full_message = self.show_commit_diff && commit.full_message.is_none();
            if !self.stats_cache.contains_key(&hash)
                && let Ok(stats) = git::get_commit_stats(&self.repo_path, &hash)
            {
                self.stats_cache.insert(hash.clone(), stats);
            }
            if needs_email
                && let Some(commit_idx) = self.commit_position(&hash)
                && let Ok(author_email) = git::get_commit_author_email(&self.repo_path, &hash)
            {
                self.commits[commit_idx].author_email = author_email;
            }
            if needs_full_message
                && let Some(commit_idx) = self.commit_position(&hash)
                && let Ok(message) = git::get_commit_full_message(&self.repo_path, &hash)
            {
                self.commits[commit_idx].full_message = Some(message);
            }
            if self.show_commit_diff && !self.commit_diff_cache.contains_key(&hash) {
                let lines = match git::get_commit_diff(&self.repo_path, &hash) {
                    Ok(diff) => diff.lines().map(String::from).collect(),
                    Err(e) => vec![format!("Error loading diff: {e}")],
                };
                self.commit_diff_file_cache
                    .insert(hash.clone(), parse_diff_files(&lines));
                self.commit_diff_cache.insert(hash, lines);
            }
        }
        self.selected_stats_due_at = None;
    }

    pub fn schedule_selected_stats_load(&mut self) {
        self.selected_stats_due_at = Some(Instant::now() + SELECTED_STATS_DEBOUNCE);
    }

    pub fn maybe_load_selected_stats(&mut self) {
        if self
            .selected_stats_due_at
            .map(|due_at| Instant::now() >= due_at)
            .unwrap_or(false)
        {
            let started = Instant::now();
            self.load_selected_stats();
            self.push_perf_metric("commit_list.deferred_stats_load", started.elapsed());
        }
    }

    pub fn reset_graph_cache(&mut self) {
        // Commit list changed: both engines' caches (including the tree
        // engine's topology ranks) are invalid.
        self.classic_engine.reset_full();
        self.tree_engine.reset_full();
        self.prefix_writer = None;
        self.branch_commit_cache.clear();
        self.merge_points = None;
    }

    /// Engine used by the active view, if the view draws a graph at all.
    fn active_engine_kind(&self) -> Option<git::GraphEngineKind> {
        match self.commit_list_view {
            CommitListView::Simple => None,
            CommitListView::Graph => Some(git::GraphEngineKind::Classic),
            CommitListView::Focused => Some(git::GraphEngineKind::Tree),
        }
    }

    pub(crate) fn cycle_commit_list_view(&mut self) {
        self.commit_list_view = self.commit_list_view.next();
    }

    pub(crate) fn cycle_commit_list_view_back(&mut self) {
        self.commit_list_view = self.commit_list_view.prev();
    }

    pub fn ensure_graph_prefixes_for_visible_range(&mut self, _start: usize, end: usize) {
        const GRAPH_PREFIX_BATCH: usize = 20_000; // pure in-memory since parents ship with the walk
        let started = Instant::now();
        if self.visible.is_empty() {
            return;
        }
        let Some(kind) = self.active_engine_kind() else {
            return; // Simple view draws no graph
        };
        // The tree engine needs the full parent topology for its rank table.
        if kind == git::GraphEngineKind::Tree {
            let len = self.commits.len();
            if let Err(e) = git::populate_commit_parents(&self.repo_path, &mut self.commits, 0, len)
            {
                self.status_message = Some(format!("Error loading commit parents: {e}"));
                return;
            }
        }
        // Both engines write the same per-commit prefix fields; when the
        // active view switches engines, the incoming engine must recompute.
        if self.prefix_writer != Some(kind) {
            self.classic_engine.reset();
            self.tree_engine.reset();
            self.prefix_writer = Some(kind);
            self.masked_prefixes_ready = false;
        }
        // Masked view (collapse, drill or search filter): rows are a subset of
        // the commits, so prefixes come from the hidden-aware windowed walk.
        // `rebuild_visible` clears the mask on the plain fast path.
        if !self.collapsed_hidden_flags.is_empty() {
            // The hidden-aware walk starts at commit 0 regardless of the
            // requested window, so it costs O(total commits). Run it ONCE per
            // mask change over every visible row (rows keep their overrides);
            // subsequent draws skip it entirely.
            if self.masked_prefixes_ready {
                return;
            }
            let mut commit_ord = 0usize;
            for row in self.visible.iter() {
                match row {
                    // DrillHeader is part of the engine's visible walk (its
                    // commit is unhidden), so it counts like a commit row.
                    VisibleRow::Commit { .. } | VisibleRow::DrillHeader { .. } => {
                        commit_ord += 1;
                    }
                    VisibleRow::CollapsedBranch { .. } | VisibleRow::WorkingTree => {}
                }
            }
            if commit_ord == 0 {
                return;
            }
            let (visible_start, visible_end) = (0, commit_ord);
            let mut prefixes: HashMap<usize, (String, Vec<u8>)> = HashMap::new();
            {
                let engine: &mut dyn git::GraphEngine = match kind {
                    git::GraphEngineKind::Classic => &mut self.classic_engine,
                    git::GraphEngineKind::Tree => &mut self.tree_engine,
                };
                engine.prefixes_in_window(
                    &self.commits,
                    &self.collapsed_hidden_flags,
                    visible_start,
                    visible_end,
                    &mut |idx, prefix, prefix_lanes| {
                        prefixes.insert(idx, (prefix, prefix_lanes));
                    },
                );
            }
            for row in self.visible.iter_mut() {
                if let VisibleRow::Commit {
                    idx,
                    prefix_override,
                    ..
                }
                | VisibleRow::DrillHeader {
                    idx,
                    prefix_override,
                } = row
                    && let Some(computed) = prefixes.remove(idx)
                {
                    *prefix_override = Some(computed);
                }
            }
            self.masked_prefixes_ready = true;
            self.push_perf_metric("commit_list.ensure_graph_prefixes", started.elapsed());
            return;
        }
        let commit_end = end.min(self.commits.len());
        let engine: &mut dyn git::GraphEngine = match kind {
            git::GraphEngineKind::Classic => &mut self.classic_engine,
            git::GraphEngineKind::Tree => &mut self.tree_engine,
        };
        let old_end = engine.computed_until();
        if commit_end <= old_end {
            return;
        }
        let compute_end = (old_end + GRAPH_PREFIX_BATCH).min(commit_end);
        // The classic engine only needs parents for the batch it processes.
        if kind == git::GraphEngineKind::Classic
            && let Err(e) = git::populate_commit_parents(
                &self.repo_path,
                &mut self.commits,
                old_end,
                compute_end,
            )
        {
            self.status_message = Some(format!("Error loading commit parents: {e}"));
            return;
        }
        let engine: &mut dyn git::GraphEngine = match kind {
            git::GraphEngineKind::Classic => &mut self.classic_engine,
            git::GraphEngineKind::Tree => &mut self.tree_engine,
        };
        engine.extend(&mut self.commits, compute_end);
        self.push_perf_metric("commit_list.ensure_graph_prefixes", started.elapsed());
    }

    pub(crate) fn toggle_commit_diff(&mut self) {
        self.show_commit_diff = !self.show_commit_diff;
        self.commit_diff_scroll = 0;
        self.commit_diff_info_scroll = 0;
        self.commit_list_focus = CommitListFocus::List;
        if self.show_commit_diff {
            self.load_selected_stats();
        } else {
            self.clear_text_selection();
        }
    }

    pub(crate) fn cycle_commit_diff_focus(&mut self, forward: bool) {
        // Panes that exist right now, in Tab order.
        let mut panes = vec![CommitListFocus::List];
        if self.show_commit_diff {
            panes.extend([CommitListFocus::DiffInfo, CommitListFocus::Diff]);
        }
        if self.working_tree_selected() {
            panes.push(CommitListFocus::Details);
        }
        let at = panes
            .iter()
            .position(|pane| *pane == self.commit_list_focus)
            .unwrap_or(0);
        let step = if forward { 1 } else { panes.len() - 1 };
        self.commit_list_focus = panes[(at + step) % panes.len()];
    }

    /// Select the visible row that corresponds to `commit_idx` in `self.commits`.
    pub(crate) fn select_commit_idx(&mut self, commit_idx: usize) {
        if let Some(vis_idx) = self
            .visible
            .iter()
            .position(|row| matches!(row, VisibleRow::Commit { idx, .. } if *idx == commit_idx))
        {
            self.list_state.select(Some(vis_idx));
        }
    }

    pub(crate) fn move_down(&mut self) {
        let len = self.visible.len();
        if len == 0 {
            return;
        }
        let next = match self.list_state.selected() {
            Some(i) if i + 1 < len => i + 1,
            Some(i) => i,
            None => 0,
        };
        self.list_state.select(Some(next));
    }

    pub(crate) fn move_up(&mut self) {
        let prev = match self.list_state.selected() {
            Some(0) | None => 0,
            Some(i) => i - 1,
        };
        self.list_state.select(Some(prev));
    }

    pub(crate) fn move_top(&mut self) {
        if !self.visible.is_empty() {
            self.list_state.select(Some(0));
        }
    }

    /// Jump to the previous/next visible commit that carries a branch or tag
    /// label ({ and } keys, gitk has nothing like it).
    pub(crate) fn jump_labeled(&mut self, forward: bool) {
        let Some(current) = self.list_state.selected() else {
            return;
        };
        let rows = self.visible.len();
        let step = |i: usize| if forward { i + 1 } else { i.wrapping_sub(1) };
        let mut i = step(current);
        while i < rows {
            if let Some(VisibleRow::Commit { idx, .. }) = self.visible.get(i)
                && self.ref_labels.contains_key(&self.commits[*idx].hash)
            {
                self.list_state.select(Some(i));
                self.schedule_selected_stats_load();
                return;
            }
            i = step(i);
        }
        self.status_message = Some(format!(
            "No labeled commit {}.",
            if forward { "below" } else { "above" }
        ));
    }

    /// Jump to the selected commit's first parent ([) or nearest child (]).
    pub(crate) fn jump_parent_or_child(&mut self, to_parent: bool) {
        let Some(commit_idx) = self.selected_commit_index() else {
            return;
        };
        let target = if to_parent {
            self.commits
                .get(commit_idx)
                .and_then(|c| c.parent_indices.as_ref())
                .and_then(|p| p.first().copied())
        } else {
            // Topological order puts children above their parent; the nearest
            // one scanning upward is the "first" child.
            (0..commit_idx).rev().find(|&i| {
                self.commits[i]
                    .parent_indices
                    .as_ref()
                    .is_some_and(|p| p.contains(&commit_idx))
            })
        };
        match target {
            Some(idx) => {
                self.select_commit_idx(idx);
                self.schedule_selected_stats_load();
            }
            None => {
                self.status_message = Some(
                    if to_parent {
                        "No parent (root commit)."
                    } else {
                        "No child in view."
                    }
                    .into(),
                );
            }
        }
    }

    /// Mainline commit that merged `commit_idx` in, or None when the commit
    /// sits on the mainline itself. Lazy one-pass compute, cached until the
    /// commit list changes; gated on full history (partial topology would
    /// mis-stamp commits near the load boundary).
    pub(crate) fn merge_point_of(&mut self, commit_idx: usize) -> Option<usize> {
        if self.history_loading {
            return None;
        }
        if self.merge_points.is_none() {
            let head_idx = self
                .head_hash
                .as_deref()
                .and_then(|hash| self.commit_position(hash))
                .unwrap_or(0);
            self.merge_points = Some(git::compute_merge_points(&self.commits, head_idx));
        }
        let stamp = self.merge_points.as_ref()?.get(commit_idx).copied()?;
        (stamp != u32::MAX).then_some(stamp as usize)
    }

    /// Row currently showing `commit_idx`, DrillHeader included.
    fn visible_row_of(&self, commit_idx: usize) -> Option<usize> {
        self.visible.iter().position(|row| {
            matches!(row,
                VisibleRow::Commit { idx, .. } | VisibleRow::DrillHeader { idx, .. }
                    if *idx == commit_idx)
        })
    }

    /// Select the row showing `commit_idx`, leaving whatever scope hides it:
    /// drill views are exited, an active filter is dropped, collapsed
    /// branches containing it are expanded. Returns a note describing the
    /// scope that was left, or None when the commit was already in view;
    /// Err(()) when it cannot be shown at all.
    pub(crate) fn reveal_and_select_commit(
        &mut self,
        commit_idx: usize,
    ) -> Result<Option<&'static str>, ()> {
        if let Some(row) = self.visible_row_of(commit_idx) {
            self.list_state.select(Some(row));
            return Ok(None);
        }
        if !self.drill_stack.is_empty() {
            self.drill_stack.clear();
            self.rebuild_visible();
            if let Some(row) = self.visible_row_of(commit_idx) {
                self.list_state.select(Some(row));
                return Ok(Some("left drill view"));
            }
        }
        if self.search.filter_active {
            self.search.filter_active = false;
            self.rebuild_visible();
            if let Some(row) = self.visible_row_of(commit_idx) {
                self.list_state.select(Some(row));
                return Ok(Some("filter removed"));
            }
        }
        if !self.collapsed_merges.is_empty() {
            let hiding: Vec<String> = self
                .collapsed_merges
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .filter(|hash| self.branch_indices_for(hash).contains(&commit_idx))
                .collect();
            if !hiding.is_empty() {
                for hash in hiding {
                    self.collapsed_merges.remove(&hash);
                }
                self.rebuild_visible();
                if let Some(row) = self.visible_row_of(commit_idx) {
                    self.list_state.select(Some(row));
                    return Ok(Some("branch expanded"));
                }
            }
        }
        Err(())
    }

    /// Jump to the mainline merge that brought the selected commit in (m).
    /// Works from any view: leaves drill/filter/collapse scopes when the
    /// merge lives outside them. Remembers where it came from for
    /// `jump_back_from_merge_point`.
    pub(crate) fn jump_to_merge_point(&mut self) {
        if self.full_history_required("finding the merge point") {
            return;
        }
        let Some(commit_idx) = self.selected_commit_index() else {
            return;
        };
        match self.merge_point_of(commit_idx) {
            Some(merge_idx) => match self.reveal_and_select_commit(merge_idx) {
                Ok(note) => {
                    self.merge_jump_origin = Some(self.commits[commit_idx].hash.clone());
                    self.schedule_selected_stats_load();
                    let c = &self.commits[merge_idx];
                    let suffix = note.map(|n| format!(" ({n})")).unwrap_or_default();
                    self.status_message = Some(format!(
                        "Merged via {} {}{suffix}. b: back",
                        c.short_hash(),
                        c.message
                    ));
                }
                Err(()) => {
                    let c = &self.commits[merge_idx];
                    self.status_message = Some(format!(
                        "Merged via {} {} — not in current view.",
                        c.short_hash(),
                        c.message
                    ));
                }
            },
            None => {
                self.status_message =
                    Some("On the mainline — committed directly (no PR merge).".into());
            }
        }
    }

    /// Return to the commit the last `m` jump started from (b). One step
    /// back, not a history: the origin is forgotten once used. Scopes that
    /// `m` left (drill, filter) stay left; the commit is selected in the
    /// view as it is now.
    pub(crate) fn jump_back_from_merge_point(&mut self) {
        let Some(hash) = self.merge_jump_origin.take() else {
            self.status_message = Some("Nothing to return to: m jumps to a merge first.".into());
            return;
        };
        let Some(commit_idx) = self.commit_position(&hash) else {
            self.status_message = Some("The commit m jumped from is no longer in the list.".into());
            return;
        };
        match self.reveal_and_select_commit(commit_idx) {
            Ok(note) => {
                self.schedule_selected_stats_load();
                let c = &self.commits[commit_idx];
                let suffix = note.map(|n| format!(" ({n})")).unwrap_or_default();
                self.status_message =
                    Some(format!("Back to {} {}{suffix}", c.short_hash(), c.message));
            }
            Err(()) => {
                self.status_message =
                    Some("The commit m jumped from is not in the current view.".into());
            }
        }
    }

    /// A/M/D/R for the active file search on this commit, lazily computed
    /// against the first parent and cached by hash.
    pub(crate) fn search_file_status(&mut self, commit_idx: usize) -> Option<char> {
        if self.search.active_file_patterns.is_empty() {
            return None;
        }
        let hash = self.commits.get(commit_idx)?.hash.clone();
        if let Some(cached) = self.file_status_cache.get(&hash) {
            return *cached;
        }
        let status =
            git::file_status_in_commit(&self.repo_path, &hash, &self.search.active_file_patterns)
                .ok()
                .flatten();
        self.file_status_cache.insert(hash, status);
        status
    }

    /// Commit index (into `commits`) of the selected row, if it is a commit.
    pub(crate) fn selected_commit_index(&self) -> Option<usize> {
        self.list_state
            .selected()
            .and_then(|i| match self.visible.get(i)? {
                VisibleRow::Commit { idx, .. } | VisibleRow::DrillHeader { idx, .. } => Some(*idx),
                VisibleRow::CollapsedBranch { .. } | VisibleRow::WorkingTree => None,
            })
    }

    /// Jump halfway toward the end (or start) of the list — repeated presses
    /// binary-search through the history.
    pub(crate) fn move_halfway(&mut self, toward_end: bool) {
        let len = self.visible.len();
        if len == 0 {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let target = if toward_end {
            current + (len - 1 - current) / 2
        } else {
            current / 2
        };
        self.list_state.select(Some(target));
    }

    pub(crate) fn move_bottom(&mut self) {
        let len = self.visible.len();
        if len > 0 {
            self.list_state.select(Some(len - 1));
        }
    }

    pub(crate) fn move_page_down(&mut self, page_size: usize) {
        let len = self.visible.len();
        if len == 0 {
            return;
        }
        let next = match self.list_state.selected() {
            Some(i) => (i + page_size.max(1)).min(len - 1),
            None => 0,
        };
        self.list_state.select(Some(next));
    }

    pub(crate) fn move_page_up(&mut self, page_size: usize) {
        if self.visible.is_empty() {
            return;
        }
        let prev = match self.list_state.selected() {
            Some(i) => i.saturating_sub(page_size.max(1)),
            None => 0,
        };
        self.list_state.select(Some(prev));
    }

    pub(crate) fn rebuild_visible(&mut self) {
        let rebuild_started = Instant::now();
        self.masked_prefixes_ready = false;
        let commit_len = self.commits.len();
        let parents_started = Instant::now();
        if let Err(e) =
            git::populate_commit_parents(&self.repo_path, &mut self.commits, 0, commit_len)
        {
            self.status_message = Some(format!("Error loading commit parents: {e}"));
            return;
        }
        self.push_perf_metric(
            "commit_list.rebuild_visible.populate_parents",
            parents_started.elapsed(),
        );

        // Gather the inputs the pure row builder needs (cached lookups need
        // &mut self, so this stays here).
        let collapsed: Vec<String> = self.collapsed_merges.iter().cloned().collect();
        // Hidden commits of the drilled-into merge become the whole view; the
        // drilled merges themselves must not hide their branches here.
        let scope_hash = self.drill_stack.last().map(|f| f.merge_hash.clone());
        let scope_depths: Option<HashMap<usize, usize>> = scope_hash
            .clone()
            .map(|hash| self.drill_scope_depths(&hash));
        let hidden_started = Instant::now();
        let mut hidden_flags = vec![false; self.commits.len()];
        let mut merge_hidden_counts: HashMap<usize, usize> = HashMap::new();
        for merge_hash in collapsed {
            if self.drill_stack.iter().any(|f| f.merge_hash == merge_hash) {
                continue;
            }
            let branch = self.branch_indices_for(&merge_hash);
            if let Some(merge_idx) = self.commit_position(&merge_hash) {
                merge_hidden_counts.insert(merge_idx, branch.len());
            }
            for idx in branch {
                if let Some(flag) = hidden_flags.get_mut(idx) {
                    *flag = true;
                }
            }
        }
        self.push_perf_metric(
            "commit_list.rebuild_visible.build_hidden_set",
            hidden_started.elapsed(),
        );

        let prefix_started = Instant::now();
        let spec = VisibleSpec {
            hidden_flags,
            merge_hidden_counts,
            header_idx: scope_hash.as_ref().and_then(|h| self.commit_position(h)),
            scope_depths,
            filter_matches: (self.search.filter_active && !self.search.matches.is_empty())
                .then(|| self.search.matches.clone()),
            no_overrides: self.collapsed_merges.is_empty(),
        };
        let (visible, final_hidden) = build_visible_rows(&self.commits, spec);
        self.collapsed_hidden_flags = final_hidden;
        self.visible = visible;
        self.ensure_working_tree_row();
        self.push_perf_metric(
            "commit_list.rebuild_visible.compute_visible_prefixes",
            prefix_started.elapsed(),
        );

        let new_len = self.visible.len();
        if new_len == 0 {
            self.list_state.select(None);
        } else if let Some(sel) = self.list_state.selected() {
            self.list_state.select(Some(sel.min(new_len - 1)));
        }
        self.push_perf_metric(
            "commit_list.rebuild_visible.total",
            rebuild_started.elapsed(),
        );
    }

    pub(crate) fn branch_indices_for(&mut self, merge_hash: &str) -> Vec<usize> {
        if let Some(cached) = self.branch_commit_cache.get(merge_hash) {
            return cached.clone();
        }
        let Some(merge_idx) = self.commit_position(merge_hash) else {
            return vec![];
        };
        let branch = git::find_branch_commit_indices(&self.commits, merge_idx);
        self.branch_commit_cache
            .insert(merge_hash.to_string(), branch.clone());
        branch
    }

    /// Drill scope: the merge's branch commits plus, transitively, the branch
    /// commits of every merge inside it. A sub-merge's branch can be excluded
    /// from the plain "unique to branch" set (e.g. a topic also merged to
    /// mainline separately), yet its commits still belong under this header.
    /// Returns commit index → nesting depth (direct branch = 1, sub-merge
    /// branches deeper). BFS keeps the shallowest depth when a commit is
    /// reachable through several merges.
    pub(crate) fn drill_scope_depths(&mut self, merge_hash: &str) -> HashMap<usize, usize> {
        // ponytail: caps keep "merge master into branch" sub-merges from
        // nesting half the repo inside one drill (found on a 455k-commit
        // repo, where the uncapped closure spun for minutes). A skipped
        // sub-merge still renders as ▼ and can be drilled directly.
        const SUB_MERGE_EXPANSION_CAP: usize = 100;
        const DRILL_SCOPE_CAP: usize = 5_000;
        let mut depths: HashMap<usize, usize> = HashMap::new();
        let mut queue: std::collections::VecDeque<(String, usize)> =
            std::collections::VecDeque::from([(merge_hash.to_string(), 0)]);
        let mut seen_merges: HashSet<String> = HashSet::new();
        while let Some((hash, depth)) = queue.pop_front() {
            if !seen_merges.insert(hash.clone()) {
                continue;
            }
            if depths.len() >= DRILL_SCOPE_CAP {
                break;
            }
            let branch = self.branch_indices_for(&hash);
            if depth > 0 && branch.len() > SUB_MERGE_EXPANSION_CAP {
                continue;
            }
            for idx in branch {
                if let std::collections::hash_map::Entry::Vacant(entry) = depths.entry(idx) {
                    entry.insert(depth + 1);
                    if self.commits[idx].is_merge {
                        queue.push_back((self.commits[idx].hash.clone(), depth + 1));
                    }
                }
            }
        }
        depths
    }

    pub(crate) fn toggle_collapse(&mut self) {
        let started = Instant::now();
        let selected_row = self.list_state.selected().and_then(|i| self.visible.get(i));
        let from_placeholder = matches!(selected_row, Some(VisibleRow::CollapsedBranch { .. }));
        let hash = match selected_row {
            Some(VisibleRow::Commit { idx, .. }) => {
                let c = &self.commits[*idx];
                if !c.is_merge {
                    self.status_message = Some("Not a merge commit.".into());
                    return;
                }
                c.hash.clone()
            }
            Some(VisibleRow::CollapsedBranch { merge_hash, .. }) => merge_hash.clone(),
            Some(VisibleRow::DrillHeader { idx, .. }) => self.commits[*idx].hash.clone(),
            Some(VisibleRow::WorkingTree) | None => return,
        };
        if self.collapsed_merges.remove(&hash) {
            self.status_message = Some("Branch expanded.".into());
            self.event_bus.emit(AppEvent::Action {
                kind: ActionKind::BranchExpanded { hash: hash.clone() },
                screen: self.current_screen(),
                at: EventTime::now(),
            });
            self.rebuild_visible();
            // When expanding from the placeholder row, jump to the merge commit.
            if from_placeholder && let Some(commit_idx) = self.commit_position(&hash) {
                self.select_commit_idx(commit_idx);
            }
        } else {
            let count = self.branch_indices_for(&hash).len();
            if count == 0 {
                self.status_message = Some("No collapsible branch found.".into());
                return;
            }
            self.collapsed_merges.insert(hash.clone());
            self.status_message = Some(format!(
                "Collapsed {count} commit{}. Space/←→ to expand.",
                if count == 1 { "" } else { "s" }
            ));
            self.event_bus.emit(AppEvent::Action {
                kind: ActionKind::BranchCollapsed { hash },
                screen: self.current_screen(),
                at: EventTime::now(),
            });
            self.rebuild_visible();
        }
        self.push_perf_metric("commit_list.toggle_collapse", started.elapsed());
    }

    /// Enter the branch commits of the selected row, like `cd`-ing into a
    /// folder. Works on a collapsed-branch placeholder or any merge commit.
    pub(crate) fn drill_into_selected(&mut self) {
        let merge_hash = match self.list_state.selected().and_then(|i| self.visible.get(i)) {
            Some(VisibleRow::CollapsedBranch { merge_hash, .. }) => merge_hash.clone(),
            Some(VisibleRow::Commit { idx, .. }) => {
                let c = &self.commits[*idx];
                if !c.is_merge {
                    return;
                }
                c.hash.clone()
            }
            _ => return,
        };
        let count = self.drill_scope_depths(&merge_hash).len();
        if count == 0 {
            self.status_message = Some("No collapsible branch found.".into());
            return;
        }
        self.breadcrumb_start = None;
        self.drill_stack.push(DrillFrame {
            merge_hash: merge_hash.clone(),
            count,
            saved_selected: self.list_state.selected(),
            saved_offset: self.list_state.offset(),
        });
        self.rebuild_visible();
        if !self
            .visible
            .iter()
            .any(|row| matches!(row, VisibleRow::Commit { .. }))
        {
            self.drill_stack.pop();
            self.rebuild_visible();
            self.status_message = Some("No commits in this branch.".into());
            return;
        }
        self.list_state.select(Some(0));
        self.schedule_selected_stats_load();
        let short = &merge_hash[..merge_hash.len().min(8)];
        self.status_message = Some(format!(
            "Showing {count} commit{} of {short}. Backspace: back.",
            if count == 1 { "" } else { "s" }
        ));
    }

    /// Shift+Enter: show the "PR" that brought the selected commit in —
    /// drill into its nearest mainline merge (merge pinned on top as the
    /// header row, branch commits below) with the ORIGINAL commit
    /// pre-selected. Backspace restores the outer view exactly.
    pub(crate) fn drill_into_merge_of_selected(&mut self) {
        if self.full_history_required("finding the merge point") {
            return;
        }
        let Some(commit_idx) = self.selected_commit_index() else {
            return;
        };
        let Some(merge_idx) = self.merge_point_of(commit_idx) else {
            self.status_message = Some("Commit is on the mainline — no containing merge.".into());
            return;
        };
        let merge_hash = self.commits[merge_idx].hash.clone();
        // That merge is already open, at this level or an outer one: go to
        // it instead of stacking a second copy of the same screen.
        if let Some(depth) = self
            .drill_stack
            .iter()
            .position(|frame| frame.merge_hash == merge_hash)
        {
            let was_current = depth + 1 == self.drill_stack.len();
            self.drill_back_to(depth + 1);
            if let Some(row) = self.visible_row_of(commit_idx) {
                self.list_state.select(Some(row));
            }
            self.schedule_selected_stats_load();
            let short = &merge_hash[..merge_hash.len().min(8)];
            self.status_message = Some(if was_current {
                format!("Already showing the merge of this commit ({short}). Backspace: back.")
            } else {
                format!("Back at the merge of this commit ({short}). Backspace: back.")
            });
            return;
        }
        let count = self.drill_scope_depths(&merge_hash).len();
        if count == 0 {
            self.status_message = Some("No branch commits under the containing merge.".into());
            return;
        }
        self.breadcrumb_start = None;
        self.drill_stack.push(DrillFrame {
            merge_hash: merge_hash.clone(),
            count,
            saved_selected: self.list_state.selected(),
            saved_offset: self.list_state.offset(),
        });
        self.rebuild_visible();
        if !self
            .visible
            .iter()
            .any(|row| matches!(row, VisibleRow::Commit { .. }))
        {
            self.drill_stack.pop();
            self.rebuild_visible();
            self.status_message = Some("No commits in this branch.".into());
            return;
        }
        // Land on the commit the user came from (falls back to the top when
        // it sits under a collapsed sub-merge inside the drill view).
        let row = self.visible_row_of(commit_idx);
        self.list_state.select(row.or(Some(0)));
        self.schedule_selected_stats_load();
        let short = &merge_hash[..merge_hash.len().min(8)];
        self.status_message = Some(format!(
            "Merged via {short}: {count} commit{}. Backspace: back.",
            if count == 1 { "" } else { "s" }
        ));
    }

    /// Back to a given drill depth (0 = outer list) by unwinding one level
    /// at a time, so every level's saved cursor/scroll restores properly.
    pub(crate) fn drill_back_to(&mut self, target_depth: usize) {
        while self.drill_stack.len() > target_depth {
            self.drill_back();
        }
    }

    /// Leave the current drill level and restore the outer view's cursor.
    pub(crate) fn drill_back(&mut self) {
        self.breadcrumb_start = None;
        let Some(frame) = self.drill_stack.pop() else {
            return;
        };
        self.rebuild_visible();
        let target = collapsed_row_position(&self.visible, &frame.merge_hash)
            .or(frame.saved_selected)
            .map(|i| i.min(self.visible.len().saturating_sub(1)));
        if self.visible.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(target.or(Some(0)));
            // Restore the scroll position too: with the same offset and the
            // same selection the outer view renders exactly as it did at
            // drill time (the viewport only moves when the selection would
            // fall outside it).
            *self.list_state.offset_mut() =
                frame.saved_offset.min(self.visible.len().saturating_sub(1));
        }
        self.schedule_selected_stats_load();
        let short = &frame.merge_hash[..frame.merge_hash.len().min(8)];
        self.status_message = Some(format!("Back from branch of {short}."));
    }
}

/// Inputs of the pure visible-row builder, gathered by rebuild_visible
/// (cached lookups need &mut App; assembling rows does not).
pub(crate) struct VisibleSpec {
    /// Rows hidden by collapsed branches (index-aligned with commits).
    pub hidden_flags: Vec<bool>,
    /// merge idx -> hidden branch size, for the "N commits hidden" marker.
    pub merge_hidden_counts: HashMap<usize, usize>,
    /// The drilled-into merge (pinned on top as a read-only header).
    pub header_idx: Option<usize>,
    /// Drill scope: commit idx -> nesting depth. None = not drilled.
    pub scope_depths: Option<HashMap<usize, usize>>,
    /// Sorted match indices while the search filter is on. None = off.
    pub filter_matches: Option<Vec<usize>>,
    /// Fast path allowed: no collapsed merges at all.
    pub no_overrides: bool,
}

/// Pure: assemble the visible rows and the final hidden mask (the mask the
/// graph engines honor for collapse/drill/filter).
pub(crate) fn build_visible_rows(
    commits: &[crate::git::CommitInfo],
    spec: VisibleSpec,
) -> (Vec<VisibleRow>, Vec<bool>) {
    let VisibleSpec {
        mut hidden_flags,
        merge_hidden_counts,
        header_idx,
        scope_depths,
        filter_matches,
        no_overrides,
    } = spec;
    let filter_on = filter_matches.is_some();

    // Search filter: non-matching commits become hidden, same mask the
    // graph engines already honor. The drill header stays visible — it is
    // the view's context, not a list entry.
    if let Some(matches) = &filter_matches {
        for (i, flag) in hidden_flags.iter_mut().enumerate() {
            if Some(i) != header_idx && matches.binary_search(&i).is_err() {
                *flag = true;
            }
        }
    }

    let mut visible: Vec<VisibleRow> = Vec::new();
    if no_overrides && scope_depths.is_none() && !filter_on {
        // Fast path: everything visible, no mask to keep.
        visible.extend((0..commits.len()).map(|i| VisibleRow::Commit {
            idx: i,
            prefix_override: None,
            depth: 0,
        }));
        return (visible, Vec::new());
    }

    // Drill view: pin the drilled-into merge on top as a read-only group
    // header (DrillHeader rows can't be drilled again).
    if let Some(merge_idx) = header_idx {
        visible.push(VisibleRow::DrillHeader {
            idx: merge_idx,
            prefix_override: None,
        });
    }
    let indices: Vec<usize> = match &scope_depths {
        Some(depths) => {
            // Everything outside the drill scope is hidden, so the graph
            // engines mask lanes the same way collapse does. The header
            // merge stays visible so the tree connects it to its rows.
            let mut scope_indices: Vec<usize> = depths.keys().copied().collect();
            scope_indices.sort_unstable();
            let mut in_scope = vec![false; commits.len()];
            for &i in &scope_indices {
                if let Some(flag) = in_scope.get_mut(i) {
                    *flag = true;
                }
            }
            if let Some(merge_idx) = header_idx
                && let Some(flag) = in_scope.get_mut(merge_idx)
            {
                *flag = true;
            }
            for (i, flag) in hidden_flags.iter_mut().enumerate() {
                if !in_scope[i] {
                    *flag = true;
                }
            }
            scope_indices
        }
        None => (0..commits.len()).collect(),
    };
    for i in indices {
        if hidden_flags.get(i).copied().unwrap_or(false) {
            continue;
        }
        visible.push(VisibleRow::Commit {
            idx: i,
            prefix_override: None,
            depth: scope_depths
                .as_ref()
                .and_then(|depths| depths.get(&i).copied())
                .unwrap_or(0),
        });
        if let Some(&count) = merge_hidden_counts.get(&i) {
            visible.push(VisibleRow::CollapsedBranch {
                count,
                merge_hash: commits[i].hash.clone(),
            });
        }
    }
    (visible, hidden_flags)
}

#[cfg(test)]
mod tests {
    use super::collapsed_row_position;
    use crate::app::models::VisibleRow;

    #[test]
    fn finds_placeholder_row_by_merge_hash() {
        let visible = vec![
            VisibleRow::Commit {
                idx: 0,
                prefix_override: None,
                depth: 0,
            },
            VisibleRow::CollapsedBranch {
                count: 3,
                merge_hash: "abc".into(),
            },
            VisibleRow::Commit {
                idx: 4,
                prefix_override: None,
                depth: 0,
            },
        ];
        assert_eq!(collapsed_row_position(&visible, "abc"), Some(1));
        assert_eq!(collapsed_row_position(&visible, "zzz"), None);
    }
}

#[cfg(test)]
mod visible_rows_tests {
    use super::{VisibleSpec, build_visible_rows};
    use crate::app::models::VisibleRow;
    use std::collections::HashMap;

    fn commits(n: usize) -> Vec<crate::git::CommitInfo> {
        (0..n)
            .map(|i| crate::git::CommitInfo {
                hash: format!("hash{i}"),
                message: String::new(),
                full_message: None,
                author_name: String::new(),
                author_email: String::new(),
                timestamp: 0,
                offset_minutes: 0,
                parent_indices: Some(vec![]),
                is_merge: false,
                graph_prefix: String::new(),
                graph_prefix_lanes: Vec::new(),
            })
            .collect()
    }

    fn spec(n: usize) -> VisibleSpec {
        VisibleSpec {
            hidden_flags: vec![false; n],
            merge_hidden_counts: HashMap::new(),
            header_idx: None,
            scope_depths: None,
            filter_matches: None,
            no_overrides: true,
        }
    }

    fn commit_indices(rows: &[VisibleRow]) -> Vec<usize> {
        rows.iter()
            .filter_map(|r| match r {
                VisibleRow::Commit { idx, .. } => Some(*idx),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn fast_path_lists_everything_in_order() {
        let (rows, mask) = build_visible_rows(&commits(5), spec(5));
        assert_eq!(commit_indices(&rows), vec![0, 1, 2, 3, 4]);
        assert!(mask.is_empty(), "fast path keeps no mask");
    }

    #[test]
    fn collapsed_branch_hides_rows_and_places_marker() {
        let cs = commits(5);
        let mut sp = spec(5);
        sp.no_overrides = false;
        // Merge at 1 hides 2 and 3.
        sp.hidden_flags[2] = true;
        sp.hidden_flags[3] = true;
        sp.merge_hidden_counts.insert(1, 2);
        let (rows, mask) = build_visible_rows(&cs, sp);
        assert_eq!(commit_indices(&rows), vec![0, 1, 4]);
        let marker = rows
            .iter()
            .find_map(|r| match r {
                VisibleRow::CollapsedBranch { count, merge_hash } => {
                    Some((*count, merge_hash.clone()))
                }
                _ => None,
            })
            .expect("marker row");
        assert_eq!(marker, (2, "hash1".into()));
        // Marker sits directly after its merge row.
        let pos = rows
            .iter()
            .position(|r| matches!(r, VisibleRow::Commit { idx: 1, .. }))
            .unwrap();
        assert!(matches!(rows[pos + 1], VisibleRow::CollapsedBranch { .. }));
        assert_eq!(mask, vec![false, false, true, true, false]);
    }

    #[test]
    fn drill_scope_pins_header_and_applies_depths() {
        let cs = commits(6);
        let mut sp = spec(6);
        sp.no_overrides = false;
        sp.header_idx = Some(1);
        sp.scope_depths = Some([(2usize, 0usize), (3, 1)].into());
        let (rows, mask) = build_visible_rows(&cs, sp);
        assert!(matches!(rows[0], VisibleRow::DrillHeader { idx: 1, .. }));
        assert_eq!(commit_indices(&rows), vec![2, 3]);
        let depths: Vec<usize> = rows
            .iter()
            .filter_map(|r| match r {
                VisibleRow::Commit { depth, .. } => Some(*depth),
                _ => None,
            })
            .collect();
        assert_eq!(depths, vec![0, 1]);
        // Everything outside the scope is masked for the graph engines —
        // EXCEPT the header merge, which stays unmasked so the tree
        // connects it to its branch rows.
        assert_eq!(mask, vec![true, false, false, false, true, true]);
    }

    #[test]
    fn filter_keeps_matches_and_header_only() {
        let cs = commits(5);
        let mut sp = spec(5);
        sp.no_overrides = false;
        sp.filter_matches = Some(vec![1, 3]);
        let (rows, _) = build_visible_rows(&cs, sp);
        assert_eq!(commit_indices(&rows), vec![1, 3]);
        // Drilled + filtered: the header survives even when unmatched
        // (headers always come with a scope in practice).
        let mut sp = spec(5);
        sp.no_overrides = false;
        sp.header_idx = Some(0);
        sp.scope_depths = Some([(3usize, 0usize), (4, 0)].into());
        sp.filter_matches = Some(vec![3]);
        let (rows, _) = build_visible_rows(&cs, sp);
        assert!(matches!(rows[0], VisibleRow::DrillHeader { idx: 0, .. }));
        assert_eq!(
            commit_indices(&rows),
            vec![3],
            "4 filtered out, header kept"
        );
    }

    /// Invariant: no commit index appears twice, whatever the spec.
    #[test]
    fn no_duplicate_rows_across_specs() {
        let cs = commits(8);
        for (hidden, header, matches) in [
            (vec![1, 2], Some(0), None),
            (vec![], None, Some(vec![0, 4, 7])),
            (vec![3], Some(2), Some(vec![2, 3, 5])),
        ] {
            let mut sp = spec(8);
            sp.no_overrides = false;
            for i in hidden {
                sp.hidden_flags[i] = true;
            }
            sp.header_idx = header;
            sp.filter_matches = matches;
            let (rows, _) = build_visible_rows(&cs, sp);
            let mut seen = std::collections::HashSet::new();
            for idx in commit_indices(&rows) {
                assert!(seen.insert(idx), "commit {idx} listed twice");
            }
        }
    }
}
