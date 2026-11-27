//! The "Uncommitted changes" row on top of the commit list: what is staged,
//! unstaged and untracked right now. Status is read on a background thread
//! (`repo_watch.rs`);
//! the patch for the diff pane is built when the row is selected.

use crate::app::App;
use crate::app::models::{VisibleRow, parse_diff_files};
use crate::git::{self, StageState};

/// Key of the working tree patch in `commit_diff_cache` /
/// `commit_diff_file_cache`, so the diff pane code is the same as for a
/// commit. Not a valid hash, so it cannot clash with one.
pub(crate) const WORKING_TREE_KEY: &str = "working-tree";

#[derive(Default)]
pub struct WorkingTreeState {
    /// Empty = clean working tree, no row.
    pub files: Vec<git::FileStatus>,
    /// Patch line where the unstaged / untracked part starts (set when the
    /// patch is loaded).
    pub unstaged_from: usize,
    pub untracked_from: usize,
    /// First line shown in the bottom Details pane.
    pub details_scroll: usize,
}

impl WorkingTreeState {
    /// (staged, unstaged, untracked) file counts.
    pub fn counts(&self) -> (usize, usize, usize) {
        let count =
            |state: StageState| self.files.iter().filter(|f| f.stage_state == state).count();
        (
            count(StageState::Staged),
            count(StageState::Unstaged),
            count(StageState::Untracked),
        )
    }

    /// "Uncommitted changes: 2 staged, 3 unstaged, 1 untracked"
    pub fn title(&self) -> String {
        let (staged, unstaged, untracked) = self.counts();
        let parts: Vec<String> = [
            (staged, "staged"),
            (unstaged, "unstaged"),
            (untracked, "untracked"),
        ]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} {what}"))
        .collect();
        format!("Uncommitted changes: {}", parts.join(", "))
    }

    /// Which part of the patch a line belongs to.
    pub fn section_of(&self, diff_line: usize) -> &'static str {
        if diff_line >= self.untracked_from {
            "untracked"
        } else if diff_line >= self.unstaged_from {
            "unstaged"
        } else {
            "staged"
        }
    }
}

impl App {
    /// New status: add, drop or keep the row, and keep the selection on the
    /// commit it was on (the row sits above every commit).
    pub(crate) fn set_working_tree(&mut self, files: Vec<git::FileStatus>) {
        let was_selected = self.working_tree_selected();
        let had_row = self.working_tree_row_shown();
        self.working_tree.files = files;
        self.working_tree.details_scroll = 0;
        self.commit_diff_cache.remove(WORKING_TREE_KEY);
        self.commit_diff_file_cache.remove(WORKING_TREE_KEY);
        let selected = self.list_state.selected();
        let offset = self.list_state.offset();
        match (had_row, self.working_tree_row_wanted()) {
            (false, true) => {
                self.visible.insert(0, VisibleRow::WorkingTree);
                self.list_state.select(selected.map(|row| row + 1));
                if offset > 0 {
                    *self.list_state.offset_mut() = offset + 1;
                }
            }
            (true, false) => {
                self.visible.remove(0);
                self.list_state.select(
                    selected
                        .filter(|_| !self.visible.is_empty())
                        .map(|row| row.saturating_sub(1)),
                );
                *self.list_state.offset_mut() = offset.saturating_sub(1);
            }
            _ => {}
        }
        if was_selected || self.working_tree_selected() {
            self.schedule_selected_stats_load();
        }
    }

    /// Scroll the bottom Details pane by `delta` lines, stopping when the
    /// last file is on its last line.
    pub(crate) fn scroll_working_tree_details(&mut self, delta: isize) {
        // One summary line, then one line per file; borders take two rows.
        let lines = self.working_tree.files.len() + 1;
        let shown = self
            .commit_details_region()
            .map_or(1, |rect| usize::from(rect.height.saturating_sub(2)).max(1));
        self.working_tree.details_scroll = self
            .working_tree
            .details_scroll
            .saturating_add_signed(delta)
            .min(lines.saturating_sub(shown));
    }

    /// Top-level list only: a drill view or a filtered list shows commits
    /// of one merge / one search, the working tree is not part of either.
    fn working_tree_row_wanted(&self) -> bool {
        !self.working_tree.files.is_empty()
            && self.drill_stack.is_empty()
            && !(self.search.filter_active && !self.search.matches.is_empty())
    }

    /// Put the row on top of a freshly built `visible` when it belongs
    /// there. Selection is not touched: callers select after building.
    pub(crate) fn ensure_working_tree_row(&mut self) {
        if self.working_tree_row_wanted() && !self.working_tree_row_shown() {
            self.visible.insert(0, VisibleRow::WorkingTree);
        }
    }

    pub(crate) fn working_tree_row_shown(&self) -> bool {
        matches!(self.visible.first(), Some(VisibleRow::WorkingTree))
    }

    pub(crate) fn working_tree_selected(&self) -> bool {
        matches!(
            self.list_state.selected().and_then(|i| self.visible.get(i)),
            Some(VisibleRow::WorkingTree)
        )
    }

    /// Number printed in front of a list row: the working tree row is 0,
    /// so commits keep the numbers they have without it.
    pub(crate) fn row_number(&self, row_idx: usize) -> usize {
        row_idx + 1 - usize::from(self.working_tree_row_shown())
    }

    /// Cache key of what the diff pane shows for the selected row.
    pub(crate) fn selected_diff_key(&self) -> Option<&str> {
        if self.working_tree_selected() {
            Some(WORKING_TREE_KEY)
        } else {
            self.selected_commit().map(|commit| commit.hash.as_str())
        }
    }

    pub(crate) fn load_working_tree_diff(&mut self) {
        if self.commit_diff_cache.contains_key(WORKING_TREE_KEY) {
            return;
        }
        let lines = match git::working_tree_patch(&self.repo_path, &self.working_tree.files) {
            Ok(patch) => {
                self.working_tree.unstaged_from = patch.unstaged_from;
                self.working_tree.untracked_from = patch.untracked_from;
                patch.lines
            }
            Err(e) => vec![format!("Error loading diff: {e}")],
        };
        self.commit_diff_file_cache
            .insert(WORKING_TREE_KEY.into(), parse_diff_files(&lines));
        self.commit_diff_cache
            .insert(WORKING_TREE_KEY.into(), lines);
    }
}
