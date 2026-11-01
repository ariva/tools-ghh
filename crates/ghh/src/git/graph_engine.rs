//! [`GraphEngine`] — common interface every commit-list graph view
//! implements. `Graph` uses [`ClassicEngine`] (first-free-lane,
//! `graph.rs`), `Focused` uses [`TreeEngine`] (gitk-style,
//! `tree_layout.rs`). New views plug in by implementing this trait.
//!
//! Both engines write their output into the shared per-commit storage
//! (`CommitInfo::graph_prefix` / `graph_prefix_lanes`), so only one engine's
//! incremental cache is valid at a time — the app resets the incoming engine
//! when the active view changes (see `App::ensure_graph_prefixes_for_visible_range`).

use crate::git::graph::{extend_graph_prefixes, for_each_visible_prefix_in_window};
use crate::git::models::CommitInfo;
use crate::git::tree_layout::{TreeLaneEngine, TreeLayout, tree_prefixes_in_window};

/// Which engine owns the shared per-commit prefix storage right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphEngineKind {
    Classic,
    Tree,
}

pub trait GraphEngine {
    /// Drop the incremental prefix cache; next `extend` recomputes from row 0.
    /// Cheap caches that only depend on commit topology may survive.
    fn reset(&mut self);

    /// Drop everything, including topology-derived caches. Call when the
    /// commit list itself was replaced.
    fn reset_full(&mut self) {
        self.reset();
    }

    /// Rows `0..computed_until()` have valid prefixes in the commits.
    fn computed_until(&self) -> usize;

    /// Extend prefixes forward to `end` (uncollapsed view: row == commit
    /// index), writing `graph_prefix`/`graph_prefix_lanes` on the commits.
    /// Requires `parent_indices` populated for the processed range.
    fn extend(&mut self, commits: &mut [CommitInfo], end: usize);

    /// Collapse-aware pass: walk the visible sequence (hidden rows skipped)
    /// and emit prefixes for rows in `visible_start..visible_end`. Stateless
    /// with respect to the incremental cache.
    fn prefixes_in_window(
        &mut self,
        commits: &[CommitInfo],
        hidden_flags: &[bool],
        visible_start: usize,
        visible_end: usize,
        on_visible: &mut dyn FnMut(usize, String, Vec<u8>),
    );
}

/// Classic `git log --graph`-style lanes (first free column, color = lane).
#[derive(Default)]
pub struct ClassicEngine {
    columns: Vec<Option<usize>>,
    computed_until: usize,
}

impl GraphEngine for ClassicEngine {
    fn reset(&mut self) {
        self.columns.clear();
        self.computed_until = 0;
    }

    fn computed_until(&self) -> usize {
        self.computed_until
    }

    fn extend(&mut self, commits: &mut [CommitInfo], end: usize) {
        let end = end.min(commits.len());
        if end <= self.computed_until {
            return;
        }
        extend_graph_prefixes(commits, self.computed_until, end, &mut self.columns);
        self.computed_until = end;
    }

    fn prefixes_in_window(
        &mut self,
        commits: &[CommitInfo],
        hidden_flags: &[bool],
        visible_start: usize,
        visible_end: usize,
        on_visible: &mut dyn FnMut(usize, String, Vec<u8>),
    ) {
        for_each_visible_prefix_in_window(
            commits,
            hidden_flags,
            visible_start,
            visible_end,
            on_visible,
        );
    }
}

/// gitk-style engine: token-ordered lanes, arrow cuts, per-line colors.
/// The rank table ([`TreeLayout`]) depends only on topology and survives
/// `reset`; `reset_full` rebuilds it.
#[derive(Default)]
pub struct TreeEngine {
    layout: Option<TreeLayout>,
    lanes: TreeLaneEngine,
}

impl TreeEngine {
    fn layout(&mut self, commits: &[CommitInfo]) -> &TreeLayout {
        self.layout
            .get_or_insert_with(|| TreeLayout::build(commits))
    }

    /// Seed the engine with state precomputed off the UI thread (the history
    /// loader runs layout + lanes for the full list before the final batch).
    pub fn from_parts(layout: TreeLayout, lanes: TreeLaneEngine) -> Self {
        Self {
            layout: Some(layout),
            lanes,
        }
    }
}

impl GraphEngine for TreeEngine {
    fn reset(&mut self) {
        self.lanes = TreeLaneEngine::new();
    }

    fn reset_full(&mut self) {
        self.reset();
        self.layout = None;
    }

    fn computed_until(&self) -> usize {
        self.lanes.computed_until()
    }

    fn extend(&mut self, commits: &mut [CommitInfo], end: usize) {
        let layout = self
            .layout
            .get_or_insert_with(|| TreeLayout::build(commits));
        self.lanes.extend(commits, layout, end);
    }

    fn prefixes_in_window(
        &mut self,
        commits: &[CommitInfo],
        hidden_flags: &[bool],
        visible_start: usize,
        visible_end: usize,
        on_visible: &mut dyn FnMut(usize, String, Vec<u8>),
    ) {
        let layout = self.layout(commits);
        tree_prefixes_in_window(
            commits,
            layout,
            hidden_flags,
            visible_start,
            visible_end,
            on_visible,
        );
    }
}
