pub mod error;
pub mod graph;
pub mod graph_engine;
pub mod history;
pub mod models;
pub mod reorder;
pub mod staging;
pub mod stash;
pub mod tree_layout;

pub use error::{Error, Result};
pub use graph::{compute_merge_points, find_branch_commit_indices};
pub use graph_engine::{ClassicEngine, GraphEngine, GraphEngineKind, TreeEngine};
pub use history::{
    CommitSort, ProgressiveLoadTimings, count_head_commits, ensure_commit_graph,
    file_status_in_commit, get_commit_author_email, get_commit_diff, get_commit_full_message,
    get_commit_patch, get_commit_stats, head_commit_hash, list_repo_files, load_commits,
    load_commits_progressive, load_ref_labels, matching_commit_hashes_for_file, modify_commit,
    populate_commit_parents, squash_commits,
};
pub use models::{CommitInfo, CommitStats, FileStatus, RefKind, RefLabel, StageState};
pub use reorder::{
    RebaseOutcome, movable_chain, move_commit, rebase_abort, rebase_continue, rebase_in_progress,
};
pub use staging::{
    commit_staged, get_file_diff, load_status, stage_all, stage_file, unstage_all, unstage_file,
};
pub use stash::{StashEntry, list_stashes, stash_apply, stash_drop, stash_patch, stash_stat};
pub use tree_layout::{TreeLaneEngine, TreeLayout};

pub(crate) use history::has_stable_commit_prefix;
