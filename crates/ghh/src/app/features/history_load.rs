//! Background history loading: progressive commit batches, total-count
//! lookup, and commit-graph warm-up, all delivered over mpsc channels and
//! drained from the event loop via `maybe_finish_history_load`.

use crate::app::App;
use crate::app::models::VisibleRow;
use crate::git;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

/// First tiny page so the UI paints immediately, then a bigger batch, then
/// the rest of the history.
const INITIAL_COMMIT_LOAD: usize = 5_000;
const INITIAL_PAGE_BATCH: usize = 300;

pub(crate) type InitialHistoryResult = std::result::Result<HistoryBatch, String>;
pub(crate) type CommitGraphWarmupResult = std::result::Result<Option<Duration>, String>;
pub(crate) type HistoryCountResult = std::result::Result<(usize, Duration), String>;

pub(crate) enum HistoryBatch {
    FirstPage(Vec<git::CommitInfo>, Duration, git::ProgressiveLoadTimings),
    Initial(Vec<git::CommitInfo>, Duration, git::ProgressiveLoadTimings),
    Full(
        Vec<git::CommitInfo>,
        Duration,
        Duration,
        git::ProgressiveLoadTimings,
        /// Tree rank table precomputed on the loader thread so the first
        /// full-history draw never runs an O(n) pass on the UI thread.
        /// Lanes stay lazy: materialising prefix strings for every commit
        /// costs ~250 MB on a 568k repo for rows mostly never viewed.
        Option<Box<git::TreeLayout>>,
    ),
}

/// Spawn the progressive history loader thread; batches arrive on the
/// returned channel and are applied by [`App::maybe_finish_history_load`].
pub(crate) fn spawn_initial_history_loader(
    repo_path: PathBuf,
    sort: git::CommitSort,
) -> Receiver<InitialHistoryResult> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let history_started = Instant::now();
        let mut last_batch = Instant::now();
        let result = git::load_commits_progressive(
            &repo_path,
            sort,
            &[INITIAL_PAGE_BATCH, INITIAL_COMMIT_LOAD],
            |commits, complete, timings| {
                let duration = last_batch.elapsed();
                last_batch = Instant::now();
                let batch = if complete {
                    // Precompute the default (Focused) view's rank table here,
                    // off the UI thread; ~100 ms for 568k commits.
                    let layout = git::TreeLayout::build(&commits);
                    HistoryBatch::Full(
                        commits,
                        duration,
                        history_started.elapsed(),
                        timings,
                        Some(Box::new(layout)),
                    )
                } else if commits.len() == INITIAL_PAGE_BATCH {
                    HistoryBatch::FirstPage(commits, duration, timings)
                } else {
                    HistoryBatch::Initial(commits, duration, timings)
                };
                tx.send(Ok(batch)).is_ok()
            },
        );
        if let Err(error) = result {
            let _ = tx.send(Err(error.to_string()));
        }
    });
    rx
}

impl App {
    pub fn maybe_finish_history_load(&mut self) {
        self.maybe_poll_repo();
        if let Some(rx) = self.initial_history_loader_rx.take() {
            match rx.try_recv() {
                Ok(Ok(HistoryBatch::FirstPage(commits, load_duration, timings))) => {
                    self.record_history_load_timings("first_page", load_duration, timings);
                    self.apply_progressive_commits(
                        commits,
                        "startup.history.chunk.first_page.ui_apply",
                    );
                    self.status_message = Some(format!(
                        "Loaded {} commits, loading more...",
                        self.commits.len()
                    ));
                    self.spawn_history_count_loader();
                    self.initial_history_loader_rx = Some(rx);
                }
                Ok(Ok(HistoryBatch::Initial(commits, load_duration, timings))) => {
                    self.record_history_load_timings("initial_5000", load_duration, timings);
                    self.apply_progressive_commits(
                        commits,
                        "startup.history.chunk.initial_5000.ui_apply",
                    );
                    self.status_message = Some(format!(
                        "Loaded {} commits, loading more...",
                        self.commits.len()
                    ));
                    self.initial_history_loader_rx = Some(rx);
                }
                Ok(Ok(HistoryBatch::Full(
                    commits,
                    load_duration,
                    total_load_duration,
                    timings,
                    precomputed,
                ))) => {
                    self.record_history_load_timings("full", load_duration, timings);
                    self.push_perf_metric("startup.history.total_worker", total_load_duration);
                    self.apply_progressive_commits(commits, "startup.history.chunk.full.ui_apply");
                    // Seed the rank table so the first Focused draw skips the
                    // O(n) TreeLayout::build; lane prefixes fill lazily per
                    // viewport batch.
                    if let Some(layout) = precomputed {
                        self.tree_engine =
                            git::TreeEngine::from_parts(*layout, git::TreeLaneEngine::new());
                        self.prefix_writer = Some(git::GraphEngineKind::Tree);
                    }
                    // Loading briefly doubles memory (loader buffers, libgit2
                    // caches); glibc keeps those freed arenas forever unless
                    // told otherwise. One trim returns them to the OS.
                    #[cfg(target_env = "gnu")]
                    unsafe {
                        libc::malloc_trim(0);
                    }
                    self.history_loading = false;
                    self.history_total_count = Some(self.commits.len());
                    self.initial_history_loader_rx = None;
                    tracing::info!(
                        "history loaded: {} commits in {:.2}s",
                        self.commits.len(),
                        total_load_duration.as_secs_f64(),
                    );
                    self.status_message = Some(format!(
                        "Full history loaded in {:.2}s: {} commits.",
                        total_load_duration.as_secs_f64(),
                        self.commits.len(),
                    ));
                    self.spawn_commit_graph_warmup();
                }
                Ok(Err(err)) => {
                    self.history_loading = false;
                    tracing::warn!("history load failed: {err}");
                    self.status_message = Some(format!("Error loading history: {err}"));
                }
                Err(TryRecvError::Empty) => {
                    self.initial_history_loader_rx = Some(rx);
                }
                Err(TryRecvError::Disconnected) => {
                    self.history_loading = false;
                    self.status_message =
                        Some("Error loading history: worker disconnected.".into());
                }
            }
        }
        self.maybe_finish_history_count_load();
        self.maybe_finish_commit_graph_warmup();
    }

    /// Switch between topological and strict-date ordering (gitk's "Strictly
    /// sort by date") and reload history from scratch.
    pub(crate) fn toggle_commit_sort(&mut self) {
        if self.full_history_required("changing the commit order") {
            return;
        }
        self.commit_sort = self.commit_sort.toggled();
        self.reload_history(format!(
            "Reloading history in {} order...",
            self.commit_sort.label()
        ));
    }

    /// Throw the list away and load it again in the background, the way it
    /// loads at startup; the selected commit is selected again when a batch
    /// brings it back.
    pub(crate) fn reload_history(&mut self, status: String) {
        let selected_hash = self.selected_commit().map(|commit| commit.hash.clone());
        // Drop the whole list: the new one may share no prefix with the old
        // one, so `apply_progressive_commits` must start from empty.
        self.commits.clear();
        self.reindex_commits();
        self.visible.clear();
        self.collapsed_merges.clear();
        self.collapsed_hidden_flags.clear();
        self.drill_stack.clear();
        self.reset_graph_cache();
        // Search matches are positions in the list that just went away.
        self.search.clear_matches();
        self.search.filter_active = false;
        self.list_state = Default::default();
        self.history_total_count = None;
        self.history_loading = true;
        self.pending_reselect_hash = selected_hash;
        self.head_hash = git::head_commit_hash(&self.repo_path);
        self.ref_labels = git::load_ref_labels(&self.repo_path);
        self.repo_watch.notice = None;
        self.initial_history_loader_rx = Some(spawn_initial_history_loader(
            self.repo_path.clone(),
            self.commit_sort,
        ));
        self.refresh_working_tree();
        self.status_message = Some(status);
    }

    /// After a reload the previously selected commit reappears in a different
    /// row; jump back to it as soon as a batch contains it.
    fn apply_pending_reselect(&mut self) {
        let Some(hash) = self.pending_reselect_hash.clone() else {
            return;
        };
        if let Some(idx) = self.commit_position(&hash) {
            // Row space, not commit index: the two differ under a mask or
            // with the working tree row on top.
            self.select_commit_idx(idx);
            self.pending_reselect_hash = None;
            self.schedule_selected_stats_load();
        } else if !self.history_loading {
            // Commit is gone (or filtered out) — stop looking.
            self.pending_reselect_hash = None;
        }
    }

    pub(crate) fn spawn_history_count_loader(&mut self) {
        if self.history_count_rx.is_some() || self.history_total_count.is_some() {
            return;
        }
        let repo_path = self.repo_path.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let started = Instant::now();
            let result = git::count_head_commits(&repo_path)
                .map(|count| (count, started.elapsed()))
                .map_err(|error| error.to_string());
            let _ = tx.send(result);
        });
        self.history_count_rx = Some(rx);
    }

    pub(crate) fn maybe_finish_history_count_load(&mut self) {
        let Some(rx) = self.history_count_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok((count, duration))) => {
                self.history_total_count = Some(count);
                self.push_perf_metric("startup.history.count_head", duration);
            }
            Ok(Err(_)) | Err(TryRecvError::Disconnected) => {}
            Err(TryRecvError::Empty) => self.history_count_rx = Some(rx),
        }
    }

    pub(crate) fn spawn_commit_graph_warmup(&mut self) {
        if self.commit_graph_warmup_rx.is_some() {
            return;
        }
        let repo_path = self.repo_path.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let started = Instant::now();
            let result = git::ensure_commit_graph(&repo_path)
                .map(|created| created.then(|| started.elapsed()))
                .map_err(|error| error.to_string());
            let _ = tx.send(result);
        });
        self.commit_graph_warmup_rx = Some(rx);
    }

    pub(crate) fn maybe_finish_commit_graph_warmup(&mut self) {
        let Some(rx) = self.commit_graph_warmup_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(Some(duration))) => {
                self.push_perf_metric("startup.history.commit_graph_warmup", duration);
                self.status_message = Some(format!(
                    "Future history startup optimized in {:.1}s.",
                    duration.as_secs_f64()
                ));
            }
            Ok(Ok(None)) | Ok(Err(_)) | Err(TryRecvError::Disconnected) => {}
            Err(TryRecvError::Empty) => self.commit_graph_warmup_rx = Some(rx),
        }
    }

    pub(crate) fn record_history_load_timings(
        &mut self,
        stage: &'static str,
        worker_elapsed: Duration,
        timings: git::ProgressiveLoadTimings,
    ) {
        if let Some(duration) = timings.git_spawn {
            self.push_perf_metric("startup.history.git_spawn", duration);
        }
        if let Some(duration) = timings.data_init {
            self.push_perf_metric("startup.history.data_init", duration);
        }
        let (worker, read_parse, snapshot_clone, process_wait) = match stage {
            "first_page" => (
                "startup.history.chunk.first_page.worker",
                "startup.history.chunk.first_page.read_parse",
                "startup.history.chunk.first_page.snapshot_clone",
                "startup.history.chunk.first_page.process_wait",
            ),
            "initial_5000" => (
                "startup.history.chunk.initial_5000.worker",
                "startup.history.chunk.initial_5000.read_parse",
                "startup.history.chunk.initial_5000.snapshot_clone",
                "startup.history.chunk.initial_5000.process_wait",
            ),
            _ => (
                "startup.history.chunk.full.worker",
                "startup.history.chunk.full.read_parse",
                "startup.history.chunk.full.snapshot_clone",
                "startup.history.chunk.full.process_wait",
            ),
        };
        self.push_perf_metric(worker, worker_elapsed);
        self.push_perf_metric(read_parse, timings.read_parse);
        if !timings.snapshot_clone.is_zero() {
            self.push_perf_metric(snapshot_clone, timings.snapshot_clone);
        }
        if !timings.process_wait.is_zero() {
            self.push_perf_metric(process_wait, timings.process_wait);
        }
    }

    pub(crate) fn apply_progressive_commits(
        &mut self,
        commits: Vec<git::CommitInfo>,
        apply_metric: &'static str,
    ) {
        let started = Instant::now();
        let selected_hash = self.selected_commit().map(|commit| commit.hash.clone());
        let on_working_tree = self.working_tree_selected();
        let old_offset = self.list_state.offset();
        let old_len = self.commits.len();
        let stable_prefix = git::has_stable_commit_prefix(&self.commits, &commits);
        debug_assert!(
            stable_prefix,
            "progressive history batch changed canonical prefix"
        );
        self.commits = commits;
        self.reindex_commits();
        // The new array's prefix fields are empty, so the engines'
        // computed_until would lie about rows 0..N being valid; any
        // rebuild_visible would then copy empty prefixes into rows that
        // never get recomputed (uncolored graph until the full batch).
        self.reset_graph_cache();
        let masked = !self.drill_stack.is_empty()
            || !self.collapsed_merges.is_empty()
            || (self.search.filter_active && !self.search.matches.is_empty());
        if masked {
            // Drill/collapse/filter views scope `visible` to a subset; blindly
            // appending the new commits here dumped the rest of the history
            // below the drilled scope. Rebuild through the mask instead, and
            // never clear the user's collapse/drill state just because a
            // batch (or the final one) arrived.
            self.rebuild_visible();
        } else if stable_prefix {
            self.visible
                .reserve(self.commits.len().saturating_sub(old_len));
            self.visible
                .extend((old_len..self.commits.len()).map(|idx| VisibleRow::Commit {
                    idx,
                    prefix_override: None,
                    depth: 0,
                }));
        } else {
            self.visible = self
                .commits
                .iter()
                .enumerate()
                .map(|(idx, _)| VisibleRow::Commit {
                    idx,
                    prefix_override: None,
                    depth: 0,
                })
                .collect();
        }
        self.ensure_working_tree_row();
        if on_working_tree && self.working_tree_row_shown() {
            self.list_state.select(Some(0));
        } else if let Some(hash) = selected_hash
            && let Some(selected_idx) = self.commit_position(&hash)
        {
            // Row-space selection: in masked views the commit index is NOT
            // the row index (the old code selected the wrong row there).
            self.select_commit_idx(selected_idx);
        } else if !self.visible.is_empty() {
            // Newest commit, not the working tree row above it.
            let first_commit = usize::from(self.working_tree_row_shown());
            self.list_state
                .select(Some(first_commit.min(self.visible.len() - 1)));
        }
        *self.list_state.offset_mut() = old_offset.min(self.visible.len().saturating_sub(1));
        self.apply_pending_reselect();
        self.push_perf_metric(apply_metric, started.elapsed());
    }

    pub(crate) fn full_history_required(&mut self, action: &str) -> bool {
        if self.history_loading {
            self.status_message = Some(format!("History still loading. Wait before {action}."));
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::models::CommitListView;

    fn commit(idx: usize, parents: Vec<usize>) -> git::CommitInfo {
        git::CommitInfo {
            hash: format!("hash{idx}"),
            message: String::new(),
            full_message: None,
            author_name: String::new(),
            author_email: String::new(),
            timestamp: 0,
            offset_minutes: 0,
            parent_indices: Some(parents),
            is_merge: false,
            graph_prefix: String::new(),
            graph_prefix_lanes: Vec::new(),
        }
    }

    /// Merge topology for drill tests (index -> first-parent order):
    ///   0 head -> [1]
    ///   1 M    -> [4, 2]   merge with side branch (2, 3)
    ///   2 side -> [3]
    ///   3 side -> [4]
    ///   4 base -> []
    fn merge_chain() -> Vec<git::CommitInfo> {
        let parents = [vec![1], vec![4, 2], vec![3], vec![4], vec![]];
        parents
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut c = commit(i, p.clone());
                c.is_merge = p.len() > 1;
                c
            })
            .collect()
    }

    /// Drilling in mid-load must survive the next batch: the old code
    /// appended every newly loaded commit flat below the drilled scope,
    /// spilling the rest of the history into the drill view.
    #[test]
    fn progressive_batch_preserves_drill_scope() {
        let dir = std::env::temp_dir().join(format!("ghh-drill-batch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir, true);
        app.initial_history_loader_rx = None;

        app.apply_progressive_commits(merge_chain(), "test.apply");
        // Drill into the merge at row 1.
        app.list_state.select(Some(1));
        app.drill_into_selected();
        assert!(!app.drill_stack.is_empty(), "drill did not engage");
        let scoped_rows = app.visible.len();
        assert!(scoped_rows < 5, "drill view should be a subset");

        // Next batch extends history below the currently loaded commits.
        let mut extended = merge_chain();
        extended.push(commit(5, vec![]));
        extended.push(commit(6, vec![]));
        app.apply_progressive_commits(extended, "test.apply");

        assert!(
            !app.drill_stack.is_empty(),
            "drill state lost on batch apply"
        );
        assert_eq!(
            app.visible.len(),
            scoped_rows,
            "batch apply leaked unscoped commits into the drill view"
        );
    }

    /// Backspace after drilling must restore the outer viewport exactly:
    /// same selected row AND same scroll offset — otherwise the row the
    /// user drilled from reappears at a different screen position.
    #[test]
    fn drill_back_restores_selection_and_scroll_offset() {
        let dir = std::env::temp_dir().join(format!("ghh-drill-offset-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir, true);
        app.initial_history_loader_rx = None;
        app.apply_progressive_commits(merge_chain(), "test.apply");

        app.list_state.select(Some(1));
        *app.list_state.offset_mut() = 1;
        app.drill_into_selected();
        assert!(!app.drill_stack.is_empty(), "drill did not engage");
        // The drill view moves cursor and scroll somewhere else entirely.
        app.list_state.select(Some(2));
        *app.list_state.offset_mut() = 3;

        app.drill_back();
        assert_eq!(
            app.list_state.selected(),
            Some(1),
            "cursor must be restored"
        );
        assert_eq!(app.list_state.offset(), 1, "scroll offset must be restored");
    }

    /// Deeper topology for merge-point jump tests:
    ///   0 head -> [1]
    ///   1 M    -> [4, 2]    mainline merge (brought 2,3,5 in)
    ///   2 M2   -> [3, 5]    sub-merge inside the branch
    ///   5 sub  -> [3]
    ///   3 side -> [4]
    ///   4 base -> []
    fn nested_merge_chain() -> Vec<git::CommitInfo> {
        let parents = [vec![1], vec![4, 2], vec![3, 5], vec![4], vec![], vec![3]];
        parents
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut c = commit(i, p.clone());
                c.is_merge = p.len() > 1;
                c
            })
            .collect()
    }

    fn test_app_with(commits: Vec<git::CommitInfo>, name: &str) -> App {
        let dir = std::env::temp_dir().join(format!("ghh-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir, true);
        app.initial_history_loader_rx = None;
        app.history_loading = false; // merge_point_of is gated on full load
        app.apply_progressive_commits(commits, "test.apply");
        app.head_hash = Some(app.commits[0].hash.clone());
        app
    }

    /// After a history rewrite (edit/squash/commit) the hashes change and
    /// every cache misses — replace_commits must schedule the details
    /// reload, or the pane shows "Loading commit data..." forever.
    #[test]
    fn replace_commits_schedules_details_reload() {
        let mut app = test_app_with(merge_chain(), "replace-reload");
        app.selected_stats_due_at = None;
        app.replace_commits(merge_chain());
        assert!(
            app.selected_stats_due_at.is_some(),
            "details reload must be scheduled after a rewrite"
        );
    }

    /// Breadcrumb click: drill_back_to unwinds level by level, so jumping
    /// from a deep drill straight to the outer list restores the original
    /// cursor and scroll.
    #[test]
    fn drill_back_to_unwinds_multiple_levels() {
        let mut app = test_app_with(nested_merge_chain(), "drill-unwind");
        app.list_state.select(Some(1));
        *app.list_state.offset_mut() = 1;
        app.drill_into_selected(); // into merge 1
        // Drill one level deeper via the sub-merge (commit 2).
        let sub_row = app
            .visible
            .iter()
            .position(|row| matches!(row, crate::app::models::VisibleRow::Commit { idx: 2, .. }))
            .expect("sub-merge visible in drill view");
        app.list_state.select(Some(sub_row));
        app.drill_into_selected();
        assert_eq!(app.drill_stack.len(), 2);

        app.drill_back_to(0);
        assert!(app.drill_stack.is_empty());
        assert_eq!(app.list_state.selected(), Some(1));
        assert_eq!(app.list_state.offset(), 1);
    }

    /// M / Shift+Enter: drill into the containing merge — merge pinned as
    /// the header row, ORIGINAL commit selected inside; Backspace restores
    /// the outer view (cursor + scroll) exactly.
    #[test]
    fn drill_into_containing_merge_selects_original_commit() {
        use crate::app::models::VisibleRow;
        let mut app = test_app_with(nested_merge_chain(), "drill-merge-of");
        app.select_commit_idx(5); // branch commit merged in via commit 1
        let before_selected = app.list_state.selected();
        let before_offset = app.list_state.offset();

        app.drill_into_merge_of_selected();
        assert_eq!(
            app.drill_stack.last().map(|f| f.merge_hash.as_str()),
            Some("hash1"),
            "must drill into the mainline merge"
        );
        assert!(
            matches!(
                app.visible.first(),
                Some(VisibleRow::DrillHeader { idx: 1, .. })
            ),
            "merge commit pinned as the first row"
        );
        let selected_row = app.list_state.selected().unwrap();
        assert!(
            matches!(app.visible[selected_row], VisibleRow::Commit { idx: 5, .. }),
            "original commit selected inside the drill view"
        );

        app.drill_back();
        assert_eq!(app.list_state.selected(), before_selected);
        assert_eq!(app.list_state.offset(), before_offset);

        // Mainline commit: no containing merge, no drill.
        app.select_commit_idx(4);
        app.drill_into_merge_of_selected();
        assert!(app.drill_stack.is_empty());
        assert!(
            app.status_message
                .as_deref()
                .unwrap_or("")
                .contains("mainline")
        );
    }

    fn dirty_files() -> Vec<git::FileStatus> {
        [
            ("a.rs", git::StageState::Staged, 'M'),
            ("b.rs", git::StageState::Unstaged, 'M'),
            ("c.rs", git::StageState::Untracked, '?'),
        ]
        .into_iter()
        .map(|(path, stage_state, status_char)| git::FileStatus {
            path: path.into(),
            stage_state,
            status_char,
        })
        .collect()
    }

    /// Dirty working tree: a row appears above the commits as number 0,
    /// the selection stays on its commit, and the row goes when clean.
    #[test]
    fn working_tree_row_appears_on_top_and_goes_when_clean() {
        use crate::app::models::VisibleRow;
        let mut app = test_app_with(merge_chain(), "worktree-row");
        app.select_commit_idx(2);

        app.set_working_tree(dirty_files());
        assert!(matches!(app.visible[0], VisibleRow::WorkingTree));
        assert_eq!(app.visible.len(), app.commits.len() + 1);
        assert_eq!(app.selected_commit_index(), Some(2), "selection moved");
        assert_eq!(app.row_number(1), 1, "newest commit keeps number 1");
        assert_eq!(
            app.working_tree.title(),
            "Uncommitted changes: 1 staged, 1 unstaged, 1 untracked"
        );

        app.set_working_tree(Vec::new());
        assert!(matches!(app.visible[0], VisibleRow::Commit { idx: 0, .. }));
        assert_eq!(app.selected_commit_index(), Some(2));
        assert_eq!(app.row_number(0), 1);
    }

    /// Row 0 in the Files column: an untracked file is "?", not the "A"
    /// its made-up "new file" patch would give; a staged new file is "A".
    #[test]
    fn files_column_marks_untracked_with_a_question_mark() {
        use crate::app::features::working_tree::WORKING_TREE_KEY;
        use crate::app::models::CommitDiffFile;
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = test_app_with(merge_chain(), "worktree-untracked-mark");
        app.set_working_tree(dirty_files());
        app.list_state.select(Some(0));
        app.show_commit_diff = true;
        let file = |path: &str, diff_line| CommitDiffFile {
            path: path.to_string(),
            diff_line,
            additions: 1,
            deletions: 0,
            status: 'A',
        };
        app.commit_diff_file_cache.insert(
            WORKING_TREE_KEY.to_string(),
            vec![
                file("staged.txt", 1),
                file("edited.txt", 6),
                file("notes.txt", 11),
            ],
        );
        app.commit_diff_cache.insert(
            WORKING_TREE_KEY.to_string(),
            (0..20).map(|i| format!("+line {i}")).collect(),
        );
        app.working_tree.unstaged_from = 5;
        app.working_tree.untracked_from = 10;

        let mut terminal = Terminal::new(TestBackend::new(140, 45)).unwrap();
        terminal
            .draw(|frame| crate::ui::render(frame, &mut app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let screen: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(
            screen.contains("A s staged.txt"),
            "staged new file is added"
        );
        assert!(screen.contains("?   notes.txt"));
        // Status and state mark take the colour of the state, as in the
        // staging view: staged green, unstaged red.
        let color_before = |text: &str| {
            // Byte offset in `screen` back to the cell it came from.
            let mut offset = screen.find(text).unwrap();
            let cell = buffer.content().iter().find(|cell| {
                let here = offset == 0;
                offset = offset.saturating_sub(cell.symbol().len());
                here
            });
            cell.unwrap().fg
        };
        use ratatui::style::Color;
        assert_eq!(color_before("A s staged.txt"), Color::Green);
        assert_eq!(color_before("s staged.txt"), Color::Green);
        assert_eq!(color_before("A u edited.txt"), Color::Red);
        assert_eq!(color_before("u edited.txt"), Color::Red);
        assert!(!screen.contains("A   notes.txt") && !screen.contains("A ? notes.txt"));
    }

    /// The row is not a commit: commit actions do nothing on it, and a
    /// drill view (commits of one merge) does not show it.
    #[test]
    fn working_tree_row_is_not_a_commit_and_stays_out_of_drill_views() {
        let mut app = test_app_with(merge_chain(), "worktree-not-commit");
        app.set_working_tree(dirty_files());
        app.list_state.select(Some(0));
        assert!(app.working_tree_selected());
        assert!(app.selected_commit().is_none());
        app.jump_to_merge_point();
        app.drill_into_merge_of_selected();
        app.toggle_collapse();
        assert!(app.drill_stack.is_empty());
        assert!(app.working_tree_selected(), "still on the row");

        app.select_commit_idx(1);
        app.drill_into_selected();
        assert!(!app.working_tree_row_shown(), "no row inside a drill view");
        app.drill_back();
        assert!(app.working_tree_row_shown(), "row is back at the top level");
    }

    /// A history batch arriving later keeps the row and the newest commit
    /// selected, not the row.
    #[test]
    fn working_tree_row_survives_history_batches() {
        let mut app = test_app_with(merge_chain(), "worktree-batch");
        app.set_working_tree(dirty_files());
        app.list_state.select(None);
        app.apply_progressive_commits(merge_chain(), "test.apply");
        assert!(app.working_tree_row_shown());
        assert_eq!(app.visible.len(), app.commits.len() + 1);
        assert_eq!(app.selected_commit_index(), Some(0));

        app.list_state.select(Some(0));
        app.apply_progressive_commits(merge_chain(), "test.apply");
        assert!(app.working_tree_selected(), "user's choice of the row kept");
    }

    /// Tab reaches the Details pane only on the working tree row; there
    /// j/k scroll its file list and never run past the last file.
    #[test]
    fn working_tree_details_pane_takes_focus_and_scrolls() {
        use crate::app::models::CommitListFocus;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let mut app = test_app_with(merge_chain(), "worktree-details");
        app.show_commit_diff = false;
        let mut files = dirty_files();
        files.extend((0..40).map(|i| git::FileStatus {
            path: format!("new{i}.txt"),
            stage_state: git::StageState::Untracked,
            status_char: '?',
        }));
        app.set_working_tree(files);

        app.handle_normal(key(KeyCode::Tab));
        assert_eq!(app.commit_list_focus, CommitListFocus::List, "commit row");

        app.list_state.select(Some(0));
        app.handle_normal(key(KeyCode::Tab));
        assert_eq!(app.commit_list_focus, CommitListFocus::Details);
        app.handle_normal(key(KeyCode::Char('j')));
        app.handle_normal(key(KeyCode::Char('j')));
        assert_eq!(app.working_tree.details_scroll, 2);
        assert!(app.working_tree_selected(), "j scrolls, selection stays");
        app.handle_normal(key(KeyCode::Char('G')));
        let bottom = app.working_tree.details_scroll;
        assert!(
            bottom > 2 && bottom <= 43,
            "stops at the last file: {bottom}"
        );
        app.handle_normal(key(KeyCode::Char('j')));
        assert_eq!(app.working_tree.details_scroll, bottom);
        app.handle_normal(key(KeyCode::Char('g')));
        assert_eq!(app.working_tree.details_scroll, 0);

        app.handle_normal(key(KeyCode::Tab));
        assert_eq!(app.commit_list_focus, CommitListFocus::List);
    }

    /// Scrolling the diff or info pane past its end stops with the last
    /// line at the bottom of the pane, however far the keys pushed.
    #[test]
    fn diff_panes_do_not_scroll_past_the_last_line() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = test_app_with(merge_chain(), "diff-scroll-end");
        app.show_commit_diff = true;
        app.select_commit_idx(0);
        let hash = app.commits[0].hash.clone();
        let lines: Vec<String> = (0..100).map(|i| format!("+line {i}")).collect();
        app.commit_diff_file_cache.insert(hash.clone(), Vec::new());
        app.commit_diff_cache.insert(hash, lines);
        app.commit_diff_scroll = 5_000;
        app.commit_diff_info_scroll = 5_000;

        let mut terminal = Terminal::new(TestBackend::new(140, 45)).unwrap();
        terminal
            .draw(|frame| crate::ui::render(frame, &mut app))
            .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("+line 99"), "last diff line is on screen");
        assert!(app.commit_diff_scroll < 100, "{}", app.commit_diff_scroll);
        assert!(
            screen.contains("+line 90"),
            "pane is still full, not one line at the top"
        );
        assert!(app.commit_diff_info_scroll < 50);
        // The wheel handlers measure bursts from what this frame showed.
        assert_eq!(app.drawn_scroll.diff, app.commit_diff_scroll);
        assert_eq!(app.drawn_scroll.info, app.commit_diff_info_scroll);
    }

    /// The changed files sit in a column beside the patch; picking a file
    /// there narrows the patch pane to it.
    #[test]
    fn files_column_lists_files_and_moves_the_patch() {
        use crate::app::models::{CommitDiffFile, CommitListFocus};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = test_app_with(merge_chain(), "files-column");
        app.show_commit_diff = true;
        app.select_commit_idx(0);
        let hash = app.commits[0].hash.clone();
        let file = |path: &str, diff_line| CommitDiffFile {
            path: path.to_string(),
            diff_line,
            additions: 1,
            deletions: 0,
            status: 'M',
        };
        // Three files; the last one starts inside the final page.
        let files = vec![
            file("src/one.rs", 0),
            file("src/two.rs", 100),
            file("src/three.rs", 195),
        ];
        app.commit_diff_file_cache.insert(hash.clone(), files);
        app.commit_diff_cache
            .insert(hash, (0..200).map(|i| format!("+line {i}")).collect());
        let mut terminal = Terminal::new(TestBackend::new(140, 45)).unwrap();
        let mut draw = |app: &mut App| -> String {
            terminal
                .draw(|frame| crate::ui::render(frame, app))
                .unwrap();
            let buffer = terminal.backend().buffer();
            buffer.content().iter().map(|cell| cell.symbol()).collect()
        };
        let press = |app: &mut App, code| {
            app.handle_normal(KeyEvent::new(code, KeyModifiers::NONE));
        };

        let column = draw(&mut app);
        assert!(column.contains(" Files [3] ") && column.contains(">All files"));
        assert!(column.contains("M src/one.rs"));
        assert!(!column.contains("Files (click to jump)"), "no copy in info");
        assert!(column.contains("+line 0") && !column.contains("\u{b7} src/"));

        // Tab order: list, info, files, patch.
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.commit_list_focus, CommitListFocus::Files);

        // A file picked: the patch pane holds its changes and nothing else.
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!((app.diff_file_only(), app.commit_diff_scroll), (Some(1), 0));
        app.commit_diff_scroll = 5_000;
        let two = draw(&mut app);
        assert!(two.contains(">M src/two.rs") && two.contains("\u{b7} src/two.rs"));
        assert!(two.contains("+line 194") && !two.contains("+line 195"));
        assert!(!two.contains("+line 99 "), "file one's lines are not shown");
        assert_eq!(app.shown_diff_range(), Some(100..195));
        // What a copy from the pane takes.
        assert_eq!(app.shown_diff_lines().map(<[String]>::len), Some(95));

        // Back on "All files": everything again, at the file just left, and
        // the file the patch is scrolled into is told from the scroll.
        press(&mut app, KeyCode::Char('g'));
        assert_eq!((app.diff_file_only(), app.commit_diff_scroll), (None, 100));
        assert_eq!(app.scrolled_into_diff_file(), Some(1));
        app.commit_diff_scroll = 20;
        assert_eq!(app.scrolled_into_diff_file(), Some(0));
        assert!(draw(&mut app).contains(">All files"));

        // Scrolled to the end, the short last file is at the top of the
        // pane and so it is the one highlighted, not the file before it.
        app.commit_diff_scroll = 5_000;
        draw(&mut app);
        assert_eq!(app.commit_diff_scroll, 195);
        assert_eq!(app.scrolled_into_diff_file(), Some(2));

        // A pick does not carry over to another commit.
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.diff_file_only(), Some(2));
        app.select_commit_idx(1);
        assert_eq!(app.diff_file_only(), None);
        app.select_commit_idx(0);
    }

    /// M pressed again inside the view it opened: same merge, so no second
    /// level is stacked; the commit stays selected.
    #[test]
    fn drill_into_containing_merge_twice_does_not_stack() {
        let mut app = test_app_with(nested_merge_chain(), "drill-merge-twice");
        app.select_commit_idx(5);
        app.drill_into_merge_of_selected();
        assert_eq!(app.drill_stack.len(), 1);

        app.drill_into_merge_of_selected();
        app.drill_into_merge_of_selected();
        assert_eq!(
            app.drill_stack.len(),
            1,
            "same merge must not be drilled again"
        );
        assert_eq!(app.selected_commit_index(), Some(5));

        // One Backspace is enough to be out.
        app.drill_back();
        assert!(app.drill_stack.is_empty());
    }

    /// M from a deeper level whose containing merge is an OUTER level
    /// already on the stack: return to that level, do not nest it again.
    #[test]
    fn drill_into_containing_merge_returns_to_open_outer_level() {
        use crate::app::models::VisibleRow;
        let mut app = test_app_with(nested_merge_chain(), "drill-merge-outer");
        app.select_commit_idx(1);
        app.drill_into_selected(); // level 1: mainline merge 1
        let sub_row = app
            .visible
            .iter()
            .position(|row| matches!(row, VisibleRow::Commit { idx: 2, .. }))
            .expect("sub-merge visible in drill view");
        app.list_state.select(Some(sub_row));
        app.drill_into_selected(); // level 2: sub-merge 2
        assert_eq!(app.drill_stack.len(), 2);
        let inner_row = app
            .visible
            .iter()
            .position(|row| matches!(row, VisibleRow::Commit { idx: 5, .. }))
            .expect("commit 5 in sub-merge scope");
        app.list_state.select(Some(inner_row));

        app.drill_into_merge_of_selected();
        assert_eq!(
            app.drill_stack.len(),
            1,
            "back at the open level of merge 1"
        );
        assert_eq!(
            app.drill_stack.last().map(|f| f.merge_hash.as_str()),
            Some("hash1")
        );
        assert_eq!(app.selected_commit_index(), Some(5));
    }

    /// m in the flat view: selection lands on the merge row.
    #[test]
    fn merge_jump_selects_merge_in_flat_view() {
        let mut app = test_app_with(nested_merge_chain(), "mp-flat");
        app.select_commit_idx(5);
        app.jump_to_merge_point();
        assert_eq!(app.list_state.selected(), Some(1));
    }

    /// m while drilled into the mainline merge itself: the merge is the
    /// DrillHeader row — selection lands on it, drill view preserved.
    #[test]
    fn merge_jump_selects_drill_header_when_merge_is_the_scope() {
        let mut app = test_app_with(nested_merge_chain(), "mp-header");
        app.select_commit_idx(1);
        app.drill_into_selected();
        assert!(!app.drill_stack.is_empty());
        // Select a commit inside the scope whose merge point is the drilled merge.
        let inner_row = app
            .visible
            .iter()
            .position(|row| matches!(row, crate::app::models::VisibleRow::Commit { idx, .. } if *idx == 5))
            .expect("commit 5 in drill scope");
        app.list_state.select(Some(inner_row));
        app.jump_to_merge_point();
        assert!(!app.drill_stack.is_empty(), "should stay drilled");
        assert_eq!(
            app.list_state.selected(),
            Some(0),
            "DrillHeader row selected"
        );
    }

    /// m while drilled into a sub-merge whose PR merge lives outside the
    /// scope: the drill view is exited and the merge row selected.
    #[test]
    fn merge_jump_leaves_drill_when_merge_is_outside_scope() {
        let mut app = test_app_with(nested_merge_chain(), "mp-outside");
        app.select_commit_idx(2); // sub-merge M2
        app.drill_into_selected();
        assert!(!app.drill_stack.is_empty());
        let inner_row = app
            .visible
            .iter()
            .position(|row| matches!(row, crate::app::models::VisibleRow::Commit { idx, .. } if *idx == 5))
            .expect("commit 5 in M2 scope");
        app.list_state.select(Some(inner_row));
        app.jump_to_merge_point();
        assert!(app.drill_stack.is_empty(), "drill should be exited");
        assert_eq!(
            app.list_state.selected(),
            Some(1),
            "merge row selected in flat view"
        );
    }

    /// m in a filtered view that hides the merge: filter dropped, merge selected.
    #[test]
    fn merge_jump_drops_filter_hiding_the_merge() {
        let mut app = test_app_with(nested_merge_chain(), "mp-filter");
        app.search.matches = vec![5];
        app.search.filter_active = true;
        app.rebuild_visible();
        assert_eq!(app.visible.len(), 1, "filter shows only the match");
        app.list_state.select(Some(0));
        app.jump_to_merge_point();
        assert!(!app.search.filter_active, "filter should be dropped");
        assert_eq!(app.list_state.selected(), Some(1), "merge row selected");
    }

    /// p / P page the commit list like PgDn / PgUp, and no longer hop
    /// search matches (n / N do that).
    #[test]
    fn p_and_shift_p_page_the_commit_list() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let press = |app: &mut App, code| {
            app.handle_normal(KeyEvent::new(code, KeyModifiers::NONE));
        };
        let mut by_p = test_app_with(nested_merge_chain(), "page-p");
        let mut by_page_key = test_app_with(nested_merge_chain(), "page-key");
        // A search match on the top row: the old `p` (previous match) would
        // have jumped to it instead of paging.
        by_p.search.matches = vec![0];
        by_p.select_commit_idx(0);
        by_page_key.select_commit_idx(0);

        press(&mut by_p, KeyCode::Char('p'));
        press(&mut by_page_key, KeyCode::PageDown);
        assert_ne!(by_p.list_state.selected(), Some(0), "p must move down");
        assert_eq!(
            by_p.list_state.selected(),
            by_page_key.list_state.selected()
        );

        press(&mut by_p, KeyCode::Char('P'));
        press(&mut by_page_key, KeyCode::PageUp);
        assert_eq!(by_p.list_state.selected(), Some(0));
        assert_eq!(
            by_p.list_state.selected(),
            by_page_key.list_state.selected()
        );
    }

    /// The three merge keys sit together under their own divider in the
    /// commit list help, with nothing between them.
    #[test]
    fn commit_list_help_groups_merge_keys() {
        let app = test_app_with(merge_chain(), "help-groups");
        let lines = app.help_lines();
        let at = |start: &str| {
            lines
                .iter()
                .position(|line| line.starts_with(start))
                .unwrap_or_else(|| panic!("{start:?} missing from help"))
        };
        let header = at("── Merges and branches");
        assert_eq!(at("m  "), header + 1);
        assert_eq!(at("b  "), header + 2);
        assert_eq!(at("M, S-Enter"), header + 3);
    }

    /// b after m: back on the commit the jump started from, once.
    #[test]
    fn merge_jump_back_returns_to_origin_once() {
        let mut app = test_app_with(nested_merge_chain(), "mp-back");
        app.select_commit_idx(5);
        let origin_row = app.list_state.selected();
        app.jump_to_merge_point();
        assert_eq!(app.list_state.selected(), Some(1));

        app.jump_back_from_merge_point();
        assert_eq!(app.list_state.selected(), origin_row, "back on the origin");

        // The origin is used up: a second b stays put and says why.
        app.select_commit_idx(4);
        app.jump_back_from_merge_point();
        assert_eq!(app.list_state.selected(), Some(4), "selection unchanged");
        assert!(
            app.status_message
                .as_deref()
                .unwrap_or("")
                .contains("Nothing to return to")
        );
    }

    /// b when m dropped a filter to reach the merge: the origin is selected
    /// in the unfiltered list.
    #[test]
    fn merge_jump_back_works_after_filter_was_dropped() {
        let mut app = test_app_with(nested_merge_chain(), "mp-back-filter");
        app.search.matches = vec![5];
        app.search.filter_active = true;
        app.rebuild_visible();
        app.list_state.select(Some(0));
        app.jump_to_merge_point();
        app.jump_back_from_merge_point();
        assert_eq!(app.selected_commit_index(), Some(5));
    }

    /// m on a mainline commit jumps nowhere, so it must not arm b.
    #[test]
    fn merge_jump_on_mainline_does_not_arm_return() {
        let mut app = test_app_with(nested_merge_chain(), "mp-back-mainline");
        app.select_commit_idx(4);
        app.jump_to_merge_point();
        assert!(app.merge_jump_origin.is_none());
    }

    /// m on a mainline commit: no movement, explanatory status.
    #[test]
    fn merge_jump_reports_mainline_commits() {
        let mut app = test_app_with(nested_merge_chain(), "mp-mainline");
        app.select_commit_idx(4);
        app.jump_to_merge_point();
        assert_eq!(app.list_state.selected(), Some(4), "selection unchanged");
        assert!(
            app.status_message
                .as_deref()
                .unwrap_or("")
                .contains("mainline")
        );
    }

    /// Tag search matches the commits tags POINT AT — n/p hops tag to tag
    /// and the filter shows only tagged commits (reachable-set semantics made
    /// a tag near HEAD match the whole history).
    #[test]
    fn tag_search_matches_ref_targets_only() {
        let mut app = test_app_with(nested_merge_chain(), "tag-search");
        let tagged_hash = app.commits[3].hash.clone();
        app.ref_labels.insert(
            tagged_hash,
            vec![git::RefLabel {
                name: "v1.0".into(),
                kind: git::RefKind::Tag,
                is_head: false,
            }],
        );
        app.search
            .set_field(crate::app::features::search::SearchField::Tag);
        *app.search.value_mut() = "v1".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert_eq!(
            app.search.matches,
            vec![3],
            "only the tagged commit matches"
        );
        // Filter shows just that commit (plus nothing else).
        app.toggle_search_filter();
        assert!(app.search.filter_active);
        assert_eq!(
            app.visible.len(),
            1,
            "filter should show only the tagged commit"
        );
    }

    /// Branch search: same point-at semantics, local + remote branch labels.
    #[test]
    fn branch_search_matches_ref_targets_only() {
        let mut app = test_app_with(nested_merge_chain(), "branch-search");
        let hash = app.commits[2].hash.clone();
        app.ref_labels.insert(
            hash,
            vec![git::RefLabel {
                name: "origin/feature-x".into(),
                kind: git::RefKind::RemoteBranch,
                is_head: false,
            }],
        );
        app.search
            .set_field(crate::app::features::search::SearchField::Branch);
        *app.search.value_mut() = "feature".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert_eq!(app.search.matches, vec![2]);
    }

    /// No matches: popup stays open with an in-popup notice (the status line
    /// is hidden behind it). Matches: popup closes as before.
    #[test]
    fn empty_search_result_keeps_popup_open() {
        let mut app = test_app_with(nested_merge_chain(), "no-match-popup");
        app.search
            .set_field(crate::app::features::search::SearchField::Message);
        *app.search.value_mut() = "zzz-no-such-commit".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert!(
            matches!(app.mode(), crate::app::models::Mode::Search),
            "popup must stay open on no matches"
        );
        assert!(app.search.notice.is_some(), "notice must inform the user");

        // Editing clears the notice; a matching search closes the popup.
        app.handle_search(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Backspace,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(app.search.notice.is_none());
        *app.search.value_mut() = String::new(); // clear the message query
        app.search
            .set_field(crate::app::features::search::SearchField::Hash);
        *app.search.value_mut() = "hash1".into();
        app.execute_search();
        assert!(!matches!(app.mode(), crate::app::models::Mode::Search));
    }

    /// Ctrl+U clears the active input.
    #[test]
    fn ctrl_u_clears_active_search_field() {
        let mut app = test_app_with(nested_merge_chain(), "ctrl-u");
        app.nav.push(crate::app::models::Mode::Search);
        app.search
            .set_field(crate::app::features::search::SearchField::Quick);
        *app.search.value_mut() = "something".into();
        app.search.cursor = 4;
        app.handle_search(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('u'),
            crossterm::event::KeyModifiers::CONTROL,
        ));
        assert!(
            app.search
                .value(crate::app::features::search::SearchField::Quick)
                .is_empty()
        );
        assert_eq!(app.search.cursor, 0);
    }

    /// Cancel (Esc / button / outside click) discards edits made while the
    /// popup was open; a successful Search keeps them.
    #[test]
    fn cancel_restores_values_from_popup_open() {
        let mut app = test_app_with(nested_merge_chain(), "cancel-restore");
        app.search
            .set_field(crate::app::features::search::SearchField::Message);
        *app.search.value_mut() = "original".into();
        app.open_search();
        assert!(matches!(app.mode(), crate::app::models::Mode::Search));
        app.search
            .set_field(crate::app::features::search::SearchField::Message);
        *app.search.value_mut() = "edited beyond recognition".into();
        app.handle_search(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(!matches!(app.mode(), crate::app::models::Mode::Search));
        assert_eq!(
            app.search
                .value(crate::app::features::search::SearchField::Message),
            "original",
            "cancel must restore the values from popup open"
        );
    }

    /// Reset clears inputs AND the live results: matches (n/p) and filter.
    #[test]
    fn reset_clears_matches_and_filter() {
        let mut app = test_app_with(nested_merge_chain(), "reset-clears");
        app.search.matches = vec![1, 2];
        app.search.filter_active = true;
        app.rebuild_visible();
        app.nav.push(crate::app::models::Mode::Search);
        app.activate_search_button(2); // Reset
        assert!(app.search.matches.is_empty(), "matches must be dropped");
        assert!(!app.search.filter_active, "filter must be off");
        assert!(app.search.notice.is_some(), "user must be told");
        assert!(
            matches!(app.mode(), crate::app::models::Mode::Search),
            "popup stays open"
        );
    }

    /// Search popup opens during progressive loading; executing tells the
    /// user instead of silently failing (and does not close the popup).
    #[test]
    fn search_during_load_opens_and_notices() {
        let mut app = test_app_with(nested_merge_chain(), "search-loading");
        app.history_loading = true;
        app.open_search();
        assert!(
            matches!(app.mode(), crate::app::models::Mode::Search),
            "popup must open"
        );
        app.search
            .set_field(crate::app::features::search::SearchField::Message);
        *app.search.value_mut() = "anything".into();
        app.execute_search();
        assert!(matches!(app.mode(), crate::app::models::Mode::Search));
        assert!(
            app.search
                .notice
                .as_deref()
                .unwrap_or("")
                .contains("still loading"),
            "notice must explain the wait"
        );
    }

    /// Reset then Esc keeps the cleared state (Reset is an immediate action,
    /// not an edit Cancel can revert). The reported bug: reset → esc →
    /// reopen showed the old inputs again.
    #[test]
    fn reset_then_esc_keeps_cleared_inputs() {
        let mut app = test_app_with(nested_merge_chain(), "reset-esc");
        app.search
            .set_field(crate::app::features::search::SearchField::Message);
        *app.search.value_mut() = "old data".into();
        app.open_search();
        app.activate_search_button(2); // Reset
        app.handle_search(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        app.open_search();
        assert!(
            app.search
                .value(crate::app::features::search::SearchField::Message)
                .is_empty(),
            "reset must survive Esc"
        );
    }

    /// F in the list clears matches and filter in one press.
    #[test]
    fn capital_f_clears_search_results() {
        let mut app = test_app_with(nested_merge_chain(), "cap-f");
        app.search.matches = vec![1, 2];
        app.search.filter_active = true;
        app.rebuild_visible();
        app.clear_search_results();
        assert!(app.search.matches.is_empty());
        assert!(!app.search.filter_active);
        assert_eq!(app.visible.len(), 6, "full list restored");
    }

    /// The status line's search segment is derived from live state: present
    /// with matches, gone after Reset+Esc (the reported staleness) or F.
    #[test]
    fn status_line_search_segment_follows_state() {
        let mut app = test_app_with(nested_merge_chain(), "status-derived");
        let text_of = |app: &crate::app::App| {
            crate::ui::screens::details::status_line_content(app)
                .spans
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>()
        };
        app.search.matches = vec![1, 2, 3];
        app.search.filter_active = true;
        assert!(text_of(&app).contains("Search 1/3"));
        assert!(text_of(&app).contains("[filtered]"));

        // Reported flow: open popup, Reset, Esc — no stale match info left.
        app.open_search();
        app.activate_search_button(2); // Reset (drops matches + filter)
        app.handle_search(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        let text = text_of(&app);
        assert!(!text.contains("Search 1/"), "stale match info: {text}");
        assert!(!text.contains("[filtered]"), "stale filter info: {text}");
    }

    /// Fuzzy suggestions work for every candidate-backed field, with the
    /// min-query gates on the full-scan fields.
    #[test]
    fn fuzzy_suggestions_cover_all_fields() {
        use crate::app::features::search::SearchField;
        let mut commits = nested_merge_chain();
        commits[2].message = "fix parser edge case".into();
        commits[2].author_name = "Alice Wonder".into();
        commits[3].message = "add parser module".into();
        commits[3].author_name = "Bob Builder".into();
        let mut app = test_app_with(commits, "fuzzy-fields");
        app.ref_labels.insert(
            app.commits[1].hash.clone(),
            vec![git::RefLabel {
                name: "v2.0-rc1".into(),
                kind: git::RefKind::Tag,
                is_head: false,
            }],
        );

        let texts = |suggestions: Vec<(String, Vec<usize>)>| {
            suggestions.into_iter().map(|(c, _)| c).collect::<Vec<_>>()
        };

        app.search.set_field(SearchField::Message);
        *app.search.value_mut() = "parser fix".into();
        assert_eq!(
            texts(app.search_suggestions()),
            vec!["fix parser edge case"]
        );

        app.search.set_field(SearchField::Author);
        *app.search.value_mut() = "bob".into();
        assert_eq!(texts(app.search_suggestions()), vec!["Bob Builder"]);

        app.search.set_field(SearchField::Tag);
        *app.search.value_mut() = "rc".into();
        assert_eq!(texts(app.search_suggestions()), vec!["v2.0-rc1"]);

        // Hash: gated below 3 chars, fuzzy at 3+.
        app.search.set_field(SearchField::Hash);
        *app.search.value_mut() = "ha".into();
        assert!(app.search_suggestions().is_empty(), "min query gate");
        *app.search.value_mut() = "hash2".into();
        assert!(texts(app.search_suggestions()).contains(&"hash2".to_string()));

        // Cache: same query returns without recompute (observable via cache key).
        let cached = app.search.suggestion_cache.clone();
        let again = app.search_suggestions();
        assert_eq!(app.search.suggestion_cache, cached);
        assert!(!again.is_empty());
    }

    /// Regression: authors repeat per commit — empty query must list many
    /// UNIQUE authors (it collapsed to one), and one char must fuzzy-match
    /// like the File field does.
    #[test]
    fn author_suggestions_unique_and_from_first_char() {
        use crate::app::features::search::SearchField;
        let mut commits = nested_merge_chain();
        let authors = ["Sam", "Sam", "Sasha", "Bob", "Steve", "Alice"];
        for (commit, author) in commits.iter_mut().zip(authors) {
            commit.author_name = author.into();
        }
        let mut app = test_app_with(commits, "author-suggest");
        app.search.set_field(SearchField::Author);

        let texts = |suggestions: Vec<(String, Vec<usize>)>| {
            suggestions.into_iter().map(|(c, _)| c).collect::<Vec<_>>()
        };
        // Empty query: unique authors, several of them.
        let empty = texts(app.search_suggestions());
        assert!(empty.len() >= 4, "want many unique authors, got {empty:?}");
        assert_eq!(
            empty.iter().filter(|a| a.as_str() == "Sam").count(),
            1,
            "no duplicates: {empty:?}"
        );
        // Single char: everyone containing an s, files-style.
        *app.search.value_mut() = "s".into();
        let with_s = texts(app.search_suggestions());
        for name in ["Sam", "Sasha", "Steve"] {
            assert!(
                with_s.contains(&name.to_string()),
                "{with_s:?} missing {name}"
            );
        }
        assert!(!with_s.contains(&"Bob".to_string()));
    }

    /// Branch search must find UNMERGED branches too: their tip is not in
    /// the HEAD-only history, so it resolves to the branch point (reported
    /// as "search by branch doesn't find").
    #[test]
    fn branch_search_finds_unmerged_branch_via_branch_point() {
        let dir = std::env::temp_dir().join(format!("ghh-unmerged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = git2::Repository::init(&dir).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let base = repo
            .commit(Some("HEAD"), &sig, &sig, "base", &tree, &[])
            .unwrap();
        let base_commit = repo.find_commit(base).unwrap();
        // Unmerged branch with its own commit on top of base.
        let side = repo
            .commit(None, &sig, &sig, "side work", &tree, &[&base_commit])
            .unwrap();
        repo.branch("feature-x", &repo.find_commit(side).unwrap(), false)
            .unwrap();
        let head2 = repo
            .commit(
                Some("HEAD"),
                &sig,
                &sig,
                "main work",
                &tree,
                &[&base_commit],
            )
            .unwrap();
        let _ = head2;

        let mut app = crate::app::App::new(dir.clone(), true);
        // Pump the real loader until the (tiny) history is in.
        for _ in 0..200 {
            app.maybe_finish_history_load();
            if !app.history_loading {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!app.history_loading, "loader did not finish");
        app.search
            .set_field(crate::app::features::search::SearchField::Branch);
        *app.search.value_mut() = "feature".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        let base_idx = app.commit_position(&base.to_string()).unwrap();
        assert_eq!(
            app.search.matches,
            vec![base_idx],
            "unmerged branch resolves to its branch point; notice: {:?}",
            app.search.notice
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Clicking a suggestion (or Tab) fills the active input; File keeps the
    /// comma-separated head segments.
    #[test]
    fn apply_completion_fills_inputs() {
        use crate::app::features::search::SearchField;
        let mut app = test_app_with(nested_merge_chain(), "apply-completion");
        app.search.set_field(SearchField::Author);
        *app.search.value_mut() = "al".into();
        app.apply_completion("Alice Wonder");
        assert_eq!(app.search.value(SearchField::Author), "Alice Wonder");

        app.search.set_field(SearchField::File);
        *app.search.value_mut() = "src/a.rs, gi".into();
        app.apply_completion("src/git/history.rs");
        assert_eq!(
            app.search.value(SearchField::File),
            "src/a.rs, src/git/history.rs"
        );
    }

    /// Double-click selection: Backspace clears the selected value, typing
    /// replaces it, navigation just deselects.
    #[test]
    fn select_all_backspace_clears_and_typing_replaces() {
        use crate::app::features::search::SearchField;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = test_app_with(nested_merge_chain(), "select-all");
        app.nav.push(crate::app::models::Mode::Search);
        app.search.set_field(SearchField::Message);
        *app.search.value_mut() = "some old query".into();

        app.search.select_all = true;
        app.handle_search(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.search.value(SearchField::Message), "");
        assert!(!app.search.select_all);

        *app.search.value_mut() = "another".into();
        app.search.select_all = true;
        app.handle_search(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(app.search.value(SearchField::Message), "x");
        assert_eq!(app.search.cursor, 1);

        *app.search.value_mut() = "kept".into();
        app.search.select_all = true;
        app.handle_search(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(
            app.search.value(SearchField::Message),
            "kept",
            "deselect keeps value"
        );
        assert!(!app.search.select_all);
    }

    /// Date-row [ clear ] stages: first click empties To, second empties From.
    #[test]
    fn date_clear_is_staged_to_then_from() {
        use crate::app::features::search::SearchField;
        let mut app = test_app_with(nested_merge_chain(), "date-staged-clear");
        *app.search
            .values
            .get_mut(SearchField::DateFrom.index())
            .unwrap() = "2024-01-01".into();
        *app.search
            .values
            .get_mut(SearchField::DateTo.index())
            .unwrap() = "2024-06-30".into();

        app.clear_date_row_staged();
        assert_eq!(app.search.value(SearchField::DateTo), "");
        assert_eq!(app.search.value(SearchField::DateFrom), "2024-01-01");

        app.clear_date_row_staged();
        assert_eq!(app.search.value(SearchField::DateFrom), "");
    }

    /// The To-prefill arrives selected: typing replaces it instead of
    /// appending; Backspace clears straight to END.
    #[test]
    fn prefilled_to_date_is_replaced_by_typing() {
        use crate::app::features::search::SearchField;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = test_app_with(nested_merge_chain(), "prefill-select");
        app.nav.push(crate::app::models::Mode::Search);
        app.search.set_field(SearchField::DateFrom);
        *app.search.value_mut() = "2024-01-15".into();
        app.search.set_field(SearchField::DateTo);
        assert_eq!(app.search.value(SearchField::DateTo), "2024-01-15");
        assert!(app.search.select_all, "prefill must arrive selected");

        app.handle_search(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
        assert_eq!(
            app.search.value(SearchField::DateTo),
            "2",
            "typing replaces, not appends"
        );

        // Same flow, Backspace: straight back to empty (END token).
        *app.search.value_mut() = String::new(); // reset To for a fresh focus
        app.search.set_field(SearchField::DateFrom);
        app.search.set_field(SearchField::DateTo);
        assert!(app.search.select_all);
        app.handle_search(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.search.value(SearchField::DateTo), "");
    }

    /// Left/Right hop over " OR " joiners as one unit; Backspace right after
    /// a joiner removes it and joins the neighbouring texts.
    #[test]
    fn arrows_skip_or_joiner_and_backspace_joins() {
        use crate::app::features::search::SearchField;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = test_app_with(nested_merge_chain(), "or-arrow-skip");
        app.nav.push(crate::app::models::Mode::Search);
        app.search.set_field(SearchField::Author);
        *app.search.value_mut() = "alice OR bob".into();

        app.search.cursor = 9; // "alice OR |bob"
        app.handle_search(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.search.cursor, 5, "left hops the whole joiner");
        app.handle_search(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.search.cursor, 9, "right hops the whole joiner");

        app.handle_search(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(
            app.search.value(SearchField::Author),
            "alicebob",
            "joiner gone, texts joined"
        );
        assert_eq!(app.search.cursor, 5);
    }

    /// Deleting the last char of one OR block (others present) asks whether
    /// to drop the block entirely, via the common confirm dialog.
    #[test]
    fn emptying_or_block_prompts_removal() {
        use crate::app::features::search::SearchField;
        use crate::app::models::{ConfirmAction, Mode};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = test_app_with(nested_merge_chain(), "or-empty-block");
        app.nav.push(Mode::Search);
        app.search.set_field(SearchField::Author);
        *app.search.value_mut() = "a OR bob".into();

        app.search.cursor = 1;
        app.handle_search(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.search.value(SearchField::Author), " OR bob");
        match app.mode() {
            Mode::Confirm(
                ConfirmAction::RemoveOrSegment {
                    field_index,
                    segment_index,
                    ..
                },
                _,
            ) => {
                assert_eq!(*field_index, SearchField::Author.index());
                assert_eq!(*segment_index, 0);
            }
            other => panic!("expected RemoveOrSegment confirm, got {other:?}"),
        }
        // Confirming removes joiner + empty block, leaving the rest intact.
        app.remove_or_segment_confirmed(SearchField::Author.index(), 0);
        assert_eq!(app.search.value(SearchField::Author), "bob");

        // Single block emptied → no prompt.
        let mut app = test_app_with(nested_merge_chain(), "or-empty-single");
        app.nav.push(Mode::Search);
        app.search.set_field(SearchField::Author);
        *app.search.value_mut() = "x".into();
        app.search.cursor = 1;
        app.handle_search(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert!(
            matches!(app.mode(), Mode::Search),
            "no confirm for lone block"
        );
    }

    /// Executed search stores its terms for highlighting; clearing drops them.
    #[test]
    fn executed_terms_snapshot_and_clear() {
        use crate::app::features::search::SearchField;
        let mut app = test_app_with(nested_merge_chain(), "executed-terms");
        app.nav.push(crate::app::models::Mode::Search);
        app.search.set_field(SearchField::Message);
        *app.search.value_mut() = "merge OR feat".into();
        app.execute_search();
        let t = app.search.executed_terms.as_ref().expect("terms stored");
        assert_eq!(t.message, vec!["merge", "feat"]);
        app.search.clear_matches();
        assert!(app.search.executed_terms.is_none());
    }

    /// Regression: matched-part highlights in the list must survive opening
    /// the commit-diff (details) panels, and the Commit Info panel must mark
    /// the searched parts too.
    #[test]
    fn list_highlights_survive_details_toggle() {
        use crate::app::features::search::SearchField;
        use ratatui::style::{Color, Modifier};
        use ratatui::{Terminal, backend::TestBackend};
        let mut commits = nested_merge_chain();
        commits[0].message = "fix search bug".into();
        let mut app = test_app_with(commits, "hl-toggle");
        app.nav.push(crate::app::models::Mode::Search);
        app.search.set_field(SearchField::Message);
        *app.search.value_mut() = "search".into();
        app.execute_search();
        assert!(!app.search.matches.is_empty());

        let count_marks = |app: &mut App| {
            let backend = TestBackend::new(140, 45);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| crate::ui::render(frame, app))
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            let mut marks = 0usize;
            for cell in buffer.content() {
                if cell.style().fg == Some(Color::Yellow)
                    && cell.style().add_modifier.contains(Modifier::BOLD)
                    && "search".contains(cell.symbol())
                {
                    marks += 1;
                }
            }
            marks
        };
        let before = count_marks(&mut app);
        assert!(
            before >= 6,
            "highlighted chars visible in plain list, got {before}"
        );
        app.toggle_commit_diff();
        assert!(app.show_commit_diff);
        let after = count_marks(&mut app);
        assert!(
            after >= before,
            "details toggle lost highlights: {before} -> {after}"
        );
    }

    /// Date From=To covers the WHOLE day: commits at any time that day match,
    /// neighbouring days don't (the user scenario: single-day search at 18:31).
    #[test]
    fn date_range_single_day_is_inclusive_all_day() {
        use crate::app::features::search::SearchField;
        const DEC25: i64 = 1608854400; // 2020-12-25T00:00:00Z
        let mut commits = nested_merge_chain();
        commits[0].timestamp = DEC25 - 60; // 24th 23:59
        commits[1].timestamp = DEC25 + 60; // 25th 00:01
        commits[2].timestamp = DEC25 + 18 * 3600 + 31 * 60; // 25th 18:31
        commits[3].timestamp = DEC25 + 86_400 + 60; // 26th 00:01
        commits[4].timestamp = DEC25 + 12 * 3600;
        commits[5].timestamp = DEC25 - 86_400;
        let mut app = test_app_with(commits, "date-single-day");
        *app.search
            .values
            .get_mut(SearchField::DateFrom.index())
            .unwrap() = "2020-12-25".into();
        *app.search
            .values
            .get_mut(SearchField::DateTo.index())
            .unwrap() = "2020-12-25".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert_eq!(
            app.search.matches,
            vec![1, 2, 4],
            "whole 25th, nothing else"
        );
    }

    /// The reported combo: message OR (with a multi-word part) AND single-day
    /// date AND file — a merge commit satisfying all three must match.
    #[test]
    fn message_or_date_and_file_combo_matches_merge() {
        use crate::app::features::search::SearchField;
        const DEC25: i64 = 1608854400;
        let mut commits = nested_merge_chain();
        commits[1].message = "Merge branch 'feature/user-auth' into fix/pagination".into();
        commits[1].timestamp = DEC25 + 18 * 3600; // 25th 18:00
        commits[2].message = "add api endpoints".into();
        commits[2].timestamp = DEC25 + 3600;
        commits[3].message = "feat: unrelated day".into();
        commits[3].timestamp = DEC25 - 86_400;
        let merge_hash = commits[1].hash.clone();
        let api_hash = commits[2].hash.clone();
        let mut app = test_app_with(commits, "combo-merge");
        *app.search
            .values
            .get_mut(SearchField::Message.index())
            .unwrap() = "feat OR api endpoints".into();
        *app.search
            .values
            .get_mut(SearchField::DateFrom.index())
            .unwrap() = "2020-12-25".into();
        *app.search
            .values
            .get_mut(SearchField::DateTo.index())
            .unwrap() = "2020-12-25".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert_eq!(
            app.search.matches,
            vec![1, 2],
            "merge via 'feat', api via phrase"
        );
        // Multi-word part is a phrase: "endpoints api" alone must NOT match.
        let mut commits = nested_merge_chain();
        commits[0].message = "endpoints api swapped".into();
        let mut app = test_app_with(commits, "combo-phrase");
        *app.search
            .values
            .get_mut(SearchField::Message.index())
            .unwrap() = "api endpoints".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert!(app.search.matches.is_empty(), "phrase order matters");
        let _ = (merge_hash, api_hash);
    }

    /// Every populated field must hold (AND): a commit passing message but
    /// failing author is out; case-insensitive throughout.
    #[test]
    fn combined_fields_are_anded_and_case_insensitive() {
        use crate::app::features::search::SearchField;
        let mut commits = nested_merge_chain();
        commits[0].message = "FEAT: shouting".into();
        commits[0].author_name = "Alice".into();
        commits[1].message = "feat: quiet".into();
        commits[1].author_name = "Bob".into();
        let mut app = test_app_with(commits, "and-fields");
        *app.search
            .values
            .get_mut(SearchField::Message.index())
            .unwrap() = "feat".into();
        *app.search
            .values
            .get_mut(SearchField::Author.index())
            .unwrap() = "ALICE".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert_eq!(app.search.matches, vec![0], "message AND author, any case");
    }

    /// Quick search spans hash, short hash, message, author and date text.
    #[test]
    fn quick_search_spans_all_commit_fields() {
        use crate::app::features::search::SearchField;
        const DEC25: i64 = 1608854400;
        let mut commits = nested_merge_chain();
        commits[0].author_name = "Zoidberg".into();
        commits[3].timestamp = DEC25 + 3600;
        let mut app = test_app_with(commits, "quick-span");
        let run = |query: &str, app: &mut App| {
            app.search.values = Default::default();
            *app.search
                .values
                .get_mut(SearchField::Quick.index())
                .unwrap() = query.into();
            app.nav.push(crate::app::models::Mode::Search);
            app.execute_search();
            app.search.matches.clone()
        };
        assert_eq!(run("hash2", &mut app), vec![2], "by hash");
        assert_eq!(run("zoidberg", &mut app), vec![0], "by author");
        assert_eq!(run("2020-12-25", &mut app), vec![3], "by date text");
        // OR in quick: two hashes.
        assert_eq!(run("hash1 OR hash4", &mut app), vec![1, 4]);
    }

    /// Wildcards work inside field parts: * spans, ? is a single char.
    #[test]
    fn wildcards_in_message_parts() {
        use crate::app::features::search::SearchField;
        let mut commits = nested_merge_chain();
        commits[0].message = "fix: broken pagination".into();
        commits[1].message = "fix: pagination".into();
        commits[2].message = "feat: pagination".into();
        let mut app = test_app_with(commits, "wildcards");
        *app.search
            .values
            .get_mut(SearchField::Message.index())
            .unwrap() = "fix*pagination".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert_eq!(app.search.matches, vec![0, 1], "* bridges the gap");
        // Wildcard patterns are whole-string globs: without a trailing * the
        // pattern must cover the full message.
        *app.search
            .values
            .get_mut(SearchField::Message.index())
            .unwrap() = "f?x:*".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert_eq!(
            app.search.matches,
            vec![0, 1],
            "? matches one char, * the rest"
        );
        *app.search
            .values
            .get_mut(SearchField::Message.index())
            .unwrap() = "f?x:".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert!(app.search.matches.is_empty(), "wildcard globs are anchored");
    }

    /// Invalid dates never execute: notice set, old matches untouched.
    #[test]
    fn invalid_date_keeps_previous_matches() {
        use crate::app::features::search::SearchField;
        let mut commits = nested_merge_chain();
        commits[0].message = "feat: keepme".into();
        let mut app = test_app_with(commits, "bad-date");
        *app.search
            .values
            .get_mut(SearchField::Message.index())
            .unwrap() = "keepme".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert_eq!(app.search.matches, vec![0]);
        *app.search
            .values
            .get_mut(SearchField::DateFrom.index())
            .unwrap() = "2020-13".into();
        app.execute_search();
        assert!(app.search.notice.is_some(), "invalid date -> notice");
        assert_eq!(app.search.matches, vec![0], "old matches survive");
    }

    /// Full-history checkbox: 4th ring slot only with file text; Enter/Space
    /// toggles it; Right from Reset stops at it (or at Reset when hidden).
    #[test]
    fn full_history_checkbox_focus_and_toggle() {
        use crate::app::features::search::SearchField;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = test_app_with(nested_merge_chain(), "fullhist-ring");
        app.nav.push(crate::app::models::Mode::Search);
        assert!(app.search.file_full_history, "on by default");

        // No file text: Right from Reset reaches Help (slot 3) and stops.
        app.search.button_focus = Some(2);
        app.handle_search(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(
            app.search.button_focus,
            Some(3),
            "Help is the last base slot"
        );
        app.handle_search(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(
            app.search.button_focus,
            Some(3),
            "no file slots without file text"
        );
        // Enter on Help opens the overlay ('?' types into inputs, so this is
        // the only way in from the popup).
        app.handle_search(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.show_help, "Help button opens the help overlay");
        app.show_help = false;

        app.search.set_field(SearchField::File);
        *app.search.value_mut() = "src/a.rs".into();
        app.search.button_focus = Some(3);
        app.handle_search(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.search.button_focus, Some(4), "checkbox reachable now");
        app.handle_search(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!app.search.file_full_history, "Enter toggles off");
        app.handle_search(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(app.search.file_full_history, "Space toggles back on");

        // Slot 5: the action selector; Enter/Space cycles through the actions.
        use crate::app::features::search::FileAction;
        app.handle_search(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(
            app.search.button_focus,
            Some(5),
            "action selector reachable"
        );
        assert_eq!(app.search.file_action, FileAction::All);
        app.handle_search(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.search.file_action, FileAction::Added);
        app.handle_search(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert_eq!(app.search.file_action, FileAction::Modified);
        for _ in 0..3 {
            app.handle_search(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        }
        assert_eq!(app.search.file_action, FileAction::All, "cycle wraps");
    }

    /// Search records timing: a search.execute perf metric for --stats and a
    /// one-shot status message with the duration; no-match notices carry it too.
    #[test]
    fn search_records_timing_metric_and_status() {
        use crate::app::features::search::SearchField;
        let mut commits = nested_merge_chain();
        commits[0].message = "feat: timed".into();
        let mut app = test_app_with(commits, "search-timing");
        app.drain_perf_metrics();
        *app.search
            .values
            .get_mut(SearchField::Message.index())
            .unwrap() = "timed".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert_eq!(app.search.matches, vec![0]);
        let metrics = app.drain_perf_metrics();
        assert!(
            metrics.iter().any(|(name, _)| *name == "search.execute"),
            "search.execute metric missing: {metrics:?}"
        );
        let status = app.status_message.as_deref().unwrap_or("");
        assert!(
            status.starts_with("Search: 1 matches in") && status.contains("ms"),
            "timing status missing: {status:?}"
        );

        // No-match: the notice carries the duration instead.
        *app.search
            .values
            .get_mut(SearchField::Message.index())
            .unwrap() = "nosuch".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        let notice = app.search.notice.as_deref().unwrap_or("");
        assert!(
            notice.starts_with("No matches in") && notice.contains("ms"),
            "timed notice missing: {notice:?}"
        );
    }

    /// Search help derives its field rules from hint_for — same text as the
    /// popup hint line, so the two can't drift apart again.
    #[test]
    fn search_help_lines_follow_hint_for() {
        let mut app = test_app_with(nested_merge_chain(), "help-hints");
        app.nav.push(crate::app::models::Mode::Search);
        let lines = app.help_lines().join("\n");
        for field in crate::app::SearchField::ALL {
            if field == crate::app::SearchField::DateTo {
                continue; // shares the Date line with DateFrom
            }
            let rules = crate::ui::popups::search::hint_for(field)
                .trim_start_matches("Ctrl+U: clear field. ");
            assert!(lines.contains(rules), "{field:?} rules missing from help");
        }
        assert!(
            lines.contains("Tab        autocomplete"),
            "key bindings intact"
        );
        assert!(
            !lines.contains("comma-separated = match any"),
            "stale hand-written field line back in help"
        );
    }

    /// Regression: [ Help ] in the search popup must actually SHOW the help
    /// overlay — the Search render branch returns early and used to skip it.
    #[test]
    fn help_overlay_renders_over_search_popup() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = test_app_with(nested_merge_chain(), "search-help-render");
        app.nav.push(crate::app::models::Mode::Search);
        let render_text = |app: &mut App| {
            let backend = TestBackend::new(120, 35);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| crate::ui::render(frame, app))
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            (0..35)
                .map(|y| {
                    (0..120)
                        .map(|x| buffer[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(
            render_text(&mut app).contains("Search Commits"),
            "popup visible"
        );
        // Activate the Help button like a click/Enter would.
        app.activate_search_button(3);
        assert!(app.show_help);
        let with_help = render_text(&mut app);
        assert!(with_help.contains("Search Help"), "help overlay drawn");
        assert!(
            !with_help.contains("Search Commits"),
            "search popup yields to the overlay"
        );
    }

    /// Click semantics of every popup element, driven through
    /// apply_search_popup_hit (what a mouse click resolves to).
    #[test]
    fn search_popup_clicks_cover_all_elements() {
        use crate::app::features::search::{FileAction, SearchField};
        use crate::app::layout::SearchPopupHit as Hit;
        use crate::app::models::{ConfirmAction, Mode};
        let mut commits = nested_merge_chain();
        commits[0].message = "feat: clickable".into();
        let mut app = test_app_with(commits, "popup-clicks");
        app.nav.push(Mode::Search);
        let width = 78u16;

        // Field click: focus + cursor at the clicked char; double-click selects.
        app.search.set_field(SearchField::Message);
        *app.search.value_mut() = "clickable".into();
        app.apply_search_popup_hit(
            Hit::Field(SearchField::Message.index()),
            Some(3),
            false,
            width,
        );
        assert_eq!(app.search.field, SearchField::Message);
        assert_eq!(app.search.cursor, 3);
        app.apply_search_popup_hit(
            Hit::Field(SearchField::Message.index()),
            Some(3),
            true,
            width,
        );
        assert!(app.search.select_all, "double-click selects all");
        app.search.select_all = false;

        // Click inside an " OR " joiner: remove-block confirm, cursor untouched.
        *app.search.value_mut() = "abc OR def".into();
        app.search.cursor = 2;
        app.apply_search_popup_hit(
            Hit::Field(SearchField::Message.index()),
            Some(5),
            false,
            width,
        );
        assert!(matches!(
            app.mode(),
            Mode::Confirm(
                ConfirmAction::RemoveOrSegment {
                    segment_index: 1,
                    ..
                },
                _
            )
        ));
        app.nav.pop();

        // [ OR ] appends the separator; [ clear ] strips the last OR segment.
        app.apply_search_popup_hit(
            Hit::OrAppend(SearchField::Message.index()),
            None,
            false,
            width,
        );
        assert_eq!(app.search.value(SearchField::Message), "abc OR def OR ");
        app.apply_search_popup_hit(
            Hit::ClearField(SearchField::Message.index()),
            None,
            false,
            width,
        );
        assert_eq!(app.search.value(SearchField::Message), "abc OR def");

        // Value scroll pages the cursor by half the popup width, clamped.
        app.search.cursor = 0;
        app.apply_search_popup_hit(
            Hit::ValueScrollRight(SearchField::Message.index()),
            None,
            false,
            width,
        );
        assert_eq!(app.search.cursor, 10, "clamped to value length");
        app.apply_search_popup_hit(
            Hit::ValueScrollLeft(SearchField::Message.index()),
            None,
            false,
            width,
        );
        assert_eq!(app.search.cursor, 0);

        // File-option clicks.
        app.search.set_field(SearchField::File);
        *app.search.value_mut() = "src/a.rs".into();
        app.apply_search_popup_hit(Hit::FullHistoryToggle, None, false, width);
        assert!(!app.search.file_full_history);
        app.apply_search_popup_hit(Hit::FileAction(3), None, false, width);
        assert_eq!(app.search.file_action, FileAction::Deleted);
        *app.search
            .values
            .get_mut(SearchField::File.index())
            .unwrap() = String::new();
        app.search.file_full_history = true;
        app.search.file_action = FileAction::All;

        // Buttons: Help opens overlay; Reset stages a backup and Undo Reset
        // restores it; Cancel restores the popup-open snapshot and closes;
        // Search executes and closes on a hit.
        app.apply_search_popup_hit(Hit::Button(3), None, false, width);
        assert!(app.show_help, "Help button");
        app.show_help = false;

        let before_reset = app.search.values.clone();
        app.apply_search_popup_hit(Hit::Button(2), None, false, width);
        assert!(
            app.search.values.iter().all(String::is_empty),
            "Reset clears"
        );
        assert!(app.search.reset_backup.is_some());
        app.apply_search_popup_hit(Hit::Button(2), None, false, width);
        assert_eq!(app.search.values, before_reset, "Undo Reset restores");

        app.search.open_backup = Some(Default::default());
        app.apply_search_popup_hit(Hit::Button(1), None, false, width);
        assert!(
            matches!(app.mode(), Mode::Normal),
            "Cancel closes the popup"
        );
        assert!(
            app.search.values.iter().all(String::is_empty),
            "Cancel restored the open snapshot"
        );

        app.nav.push(Mode::Search);
        app.search.set_field(SearchField::Message);
        *app.search.value_mut() = "clickable".into();
        app.apply_search_popup_hit(Hit::Button(0), None, false, width);
        assert_eq!(app.search.matches, vec![0], "Search button executes");
        assert!(matches!(app.mode(), Mode::Normal), "popup closes on a hit");

        // Suggestion click applies the completion.
        app.nav.push(Mode::Search);
        app.search.set_field(SearchField::Author);
        *app.search.value_mut() = "al".into();
        app.search.suggestion_cache = Some((
            SearchField::Author,
            "al".into(),
            app.commits.len(),
            vec![("Alice Wonder".into(), vec![0, 1])],
        ));
        app.apply_search_popup_hit(Hit::Suggestion(0), None, false, width);
        assert_eq!(app.search.value(SearchField::Author), "Alice Wonder");
    }

    /// Tab on an EMPTY input moves focus; with text it completes; with an
    /// empty tail segment after " OR " it moves focus too.
    #[test]
    fn tab_on_empty_input_moves_focus_instead_of_completing() {
        use crate::app::features::search::SearchField;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut commits = nested_merge_chain();
        commits[0].author_name = "Alice".into();
        let mut app = test_app_with(commits, "tab-empty");
        app.nav.push(crate::app::models::Mode::Search);
        app.search.set_field(SearchField::Author);

        // Empty Author field lists suggestions — Tab must still move on.
        app.handle_search(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.search.field, SearchField::DateFrom, "focus moved");
        assert_eq!(
            app.search.value(SearchField::Author),
            "",
            "no auto-completion"
        );

        // With text typed, Tab completes.
        app.search.set_field(SearchField::Author);
        *app.search.value_mut() = "ali".into();
        app.handle_search(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.search.value(SearchField::Author), "Alice");

        // Empty tail segment after " OR ": Tab moves focus, not completes.
        *app.search.value_mut() = "Alice OR ".into();
        app.search.cursor = 9;
        app.handle_search(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.search.value(SearchField::Author), "Alice OR ");
        assert_eq!(app.search.field, SearchField::DateFrom);
    }

    /// commit_row_spans (extracted hot row path): matched rows mark only the
    /// hit parts; unmatched rows stay plain; A/M/D/R letters prefix on file
    /// searches.
    #[test]
    fn commit_row_spans_highlights_and_file_letters() {
        use crate::app::features::search::SearchField;
        use crate::ui::screens::commit_list::commit_row_spans;
        use ratatui::style::{Color, Modifier};
        let mut commits = nested_merge_chain();
        commits[0].message = "fix search bug".into();
        commits[1].message = "unrelated".into();
        let mut app = test_app_with(commits, "row-spans");
        *app.search
            .values
            .get_mut(SearchField::Message.index())
            .unwrap() = "search".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert_eq!(app.search.matches, vec![0]);

        let statuses = std::collections::HashMap::new();
        let spans = commit_row_spans(&app, 0, 0, None, 0, 7, 60, &statuses, false, None);
        let marked: String = spans
            .iter()
            .filter(|s| {
                s.style.fg == Some(Color::Yellow) && s.style.add_modifier.contains(Modifier::BOLD)
            })
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(marked, "search", "only the hit substring is marked");
        // Unmatched row: no yellow-bold marks, no whole-row bold message.
        let plain = commit_row_spans(&app, 1, 1, None, 0, 7, 60, &statuses, false, None);
        assert!(plain.iter().all(|s| {
            !(s.style.fg == Some(Color::Yellow) && s.style.add_modifier.contains(Modifier::BOLD))
        }));

        // File letters: matched row gets its status char, colored + bold.
        let statuses: std::collections::HashMap<usize, char> = [(0usize, 'M')].into();
        let lettered = commit_row_spans(&app, 0, 0, None, 0, 7, 60, &statuses, true, None);
        assert!(
            lettered.iter().any(
                |s| s.content.as_ref() == "M " && s.style.add_modifier.contains(Modifier::BOLD)
            ),
            "status letter after the row number"
        );
    }

    /// OR-joined values: authors match any part; [OR] button appends the
    /// separator; tag search ORs too.
    #[test]
    fn or_joined_multi_value_search() {
        use crate::app::features::search::SearchField;
        let mut commits = nested_merge_chain();
        commits[2].author_name = "Alice".into();
        commits[3].author_name = "Bob".into();
        commits[5].author_name = "Carol".into();
        let mut app = test_app_with(commits, "or-multi");

        app.search.set_field(SearchField::Author);
        *app.search.value_mut() = "alice".into();
        app.append_or_separator(SearchField::Author);
        assert_eq!(app.search.value(SearchField::Author), "alice OR ");
        *app.search.value_mut() = "alice OR bob".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert_eq!(app.search.matches, vec![2, 3], "either author matches");

        // Tags OR: two tags, query matches both via OR.
        for (idx, name) in [(1usize, "v1.0"), (4usize, "v2.0")] {
            let hash = app.commits[idx].hash.clone();
            app.ref_labels.insert(
                hash,
                vec![git::RefLabel {
                    name: name.into(),
                    kind: git::RefKind::Tag,
                    is_head: false,
                }],
            );
        }
        app.search.values = Default::default();
        app.search.set_field(SearchField::Tag);
        *app.search.value_mut() = "v1.0 OR v2.0".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert_eq!(app.search.matches, vec![1, 4]);
    }

    /// Completion in OR-joined fields keeps the head segments.
    #[test]
    fn apply_completion_keeps_or_head() {
        use crate::app::features::search::SearchField;
        let mut app = test_app_with(nested_merge_chain(), "or-completion");
        app.search.set_field(SearchField::Author);
        *app.search.value_mut() = "Alice OR bo".into();
        app.apply_completion("Bob Builder");
        assert_eq!(
            app.search.value(SearchField::Author),
            "Alice OR Bob Builder"
        );

        app.search.set_field(SearchField::File);
        *app.search.value_mut() = "a.rs OR b.rs, gi".into();
        app.apply_completion("src/git/history.rs");
        assert_eq!(
            app.search.value(SearchField::File),
            "a.rs OR b.rs, src/git/history.rs"
        );
    }

    /// Backspace right after " OR " removes the whole separator at once.
    #[test]
    fn backspace_deletes_or_separator_atomically() {
        use crate::app::features::search::SearchField;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = test_app_with(nested_merge_chain(), "or-backspace");
        app.nav.push(crate::app::models::Mode::Search);
        app.search.set_field(SearchField::Author);
        *app.search.value_mut() = "alice OR ".into();
        app.search.cursor = 9;
        app.handle_search(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.search.value(SearchField::Author), "alice");
        assert_eq!(app.search.cursor, 5);
        // Plain text still deletes one char at a time.
        app.handle_search(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.search.value(SearchField::Author), "alic");
    }

    /// Joiner-click flow: confirm dialog removes "OR text..."; cancel keeps
    /// the value. Runs through the COMMON confirm machinery (same dialog as
    /// commit/squash), so Tab/y/n/mouse all apply.
    #[test]
    fn remove_or_segment_via_confirm_dialog() {
        use crate::app::features::search::SearchField;
        use crate::app::models::{ConfirmAction, Mode};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = test_app_with(nested_merge_chain(), "or-remove-confirm");
        app.nav.push(Mode::Search);
        app.search.set_field(SearchField::Author);
        *app.search.value_mut() = "alice OR bob OR carol".into();

        // As the mouse would: push the confirm for segment 1 ("OR bob").
        app.nav.push(Mode::Confirm(
            ConfirmAction::RemoveOrSegment {
                field_index: SearchField::Author.index(),
                segment_index: 1,
                preview: "OR bob".into(),
            },
            true,
        ));
        // Tab toggles to No and back to Yes (common dialog behavior).
        app.handle_confirm(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.handle_confirm(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.handle_confirm(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.search.value(SearchField::Author), "alice OR carol");
        assert!(matches!(app.mode(), Mode::Search), "back in the popup");

        // Cancel path leaves the value alone.
        app.nav.push(Mode::Confirm(
            ConfirmAction::RemoveOrSegment {
                field_index: SearchField::Author.index(),
                segment_index: 0,
                preview: "OR alice".into(),
            },
            true,
        ));
        app.handle_confirm(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.search.value(SearchField::Author), "alice OR carol");
    }

    /// [ clear ] on OR-joined values strips from the right, one per click.
    #[test]
    fn clear_button_strips_or_segments_right_to_left() {
        use crate::app::features::search::SearchField;
        let mut app = test_app_with(nested_merge_chain(), "staged-or-clear");
        app.search.set_field(SearchField::Author);
        *app.search.value_mut() = "a OR b OR c".into();
        app.clear_field_staged(SearchField::Author);
        assert_eq!(app.search.value(SearchField::Author), "a OR b");
        app.clear_field_staged(SearchField::Author);
        assert_eq!(app.search.value(SearchField::Author), "a");
        app.clear_field_staged(SearchField::Author);
        assert_eq!(app.search.value(SearchField::Author), "");
        // Non-multi fields clear whole in one go.
        app.search.set_field(SearchField::Message);
        *app.search.value_mut() = "whole thing".into();
        app.clear_field_staged(SearchField::Message);
        assert_eq!(app.search.value(SearchField::Message), "");
    }

    /// OR works in hash, message and quick fields too (every field except
    /// dates carries OR + fuzzy now).
    #[test]
    fn or_applies_to_hash_message_and_quick() {
        use crate::app::features::search::SearchField;
        let mut commits = nested_merge_chain();
        commits[1].message = "fix the parser".into();
        commits[3].message = "improve docs".into();
        let mut app = test_app_with(commits, "or-everywhere");

        app.search.set_field(SearchField::Message);
        *app.search.value_mut() = "parser OR docs".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert_eq!(app.search.matches, vec![1, 3]);

        app.search.values = Default::default();
        app.search.set_field(SearchField::Hash);
        *app.search.value_mut() = "hash0 OR hash4".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert_eq!(app.search.matches, vec![0, 4]);

        app.search.values = Default::default();
        app.search.set_field(SearchField::Quick);
        *app.search.value_mut() = "parser OR hash4".into();
        app.nav.push(crate::app::models::Mode::Search);
        app.execute_search();
        assert_eq!(app.search.matches, vec![1, 4]);
    }

    /// Move mode is a pure preview: rows show the shifted order, the moved
    /// row is tagged, keys drive it, Esc restores — git is never touched.
    #[test]
    fn move_mode_previews_shifted_rows() {
        use crate::app::models::{Mode, MoveState};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use ratatui::{Terminal, backend::TestBackend};
        let mut commits = linear_chain(4);
        for (i, c) in commits.iter_mut().enumerate() {
            c.message = format!("subject-{i}");
        }
        let mut app = test_app_with(commits, "move-preview");
        app.list_state.select(Some(0));
        app.move_state = Some(MoveState {
            chain: vec![0, 1, 2, 3],
            from: 0,
            pos: 0,
        });
        app.nav.push(Mode::Move);
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down)); // clamped at the chain end
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.list_state.selected(), Some(2));
        assert_eq!(app.selected_commit().unwrap().message, "subject-0");

        let screen = |app: &mut App| {
            let mut terminal = Terminal::new(TestBackend::new(200, 30)).unwrap();
            terminal
                .draw(|frame| crate::ui::render(frame, app))
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            buffer
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };
        let text = screen(&mut app);
        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("missing {needle}"))
        };
        assert!(text.contains("MOVE MODE"));
        assert!(at("subject-1") < at("subject-2"));
        assert!(at("subject-2") < at("subject-0"));
        assert!(at("subject-0") < at("[MOVE]"));
        assert!(at("[MOVE]") < at("subject-3"));

        app.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(app.mode(), Mode::Confirm(_, false)),
            "confirm defaults to No"
        );
        assert!(screen(&mut app).contains("Rewrites history: 3 commits get new hashes"));
        app.handle_key(key(KeyCode::Char('n')));
        app.handle_key(key(KeyCode::Esc));
        assert!(app.move_state.is_none() && matches!(app.mode(), Mode::Normal));
        assert_eq!(app.list_state.selected(), Some(0));
    }

    fn linear_chain(len: usize) -> Vec<git::CommitInfo> {
        (0..len)
            .map(|i| commit(i, if i + 1 < len { vec![i + 1] } else { vec![] }))
            .collect()
    }

    /// A non-final batch replaces `commits` with fresh (empty-prefix) parses;
    /// the engines must recompute from row 0 or rows 0..computed_until keep
    /// empty prefixes forever after the next rebuild_visible.
    #[test]
    fn non_final_batch_recomputes_graph_prefixes() {
        let dir = std::env::temp_dir().join("ghh-progressive-reset-test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir, true);
        app.initial_history_loader_rx = None; // loader thread is irrelevant here
        app.commit_list_view = CommitListView::Graph; // classic engine, no git needed

        app.apply_progressive_commits(linear_chain(3), "test.apply");
        app.ensure_graph_prefixes_for_visible_range(0, 3);
        assert!(app.commits.iter().all(|c| !c.graph_prefix.is_empty()));

        // Chain grows: parent links of the old last commit change, so the
        // batch is a fresh parse of the same prefix plus one commit.
        app.apply_progressive_commits(linear_chain(4), "test.apply");
        app.ensure_graph_prefixes_for_visible_range(0, 4);
        for (i, c) in app.commits.iter().enumerate() {
            assert!(
                !c.graph_prefix.is_empty(),
                "commit {i} lost its graph prefix after a progressive batch"
            );
        }
    }
}
