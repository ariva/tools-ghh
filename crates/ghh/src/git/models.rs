use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub hash: String,
    /// First line / subject, used in lists.
    pub message: String,
    /// Full commit message, loaded lazily only when edit flow needs it.
    pub full_message: Option<String>,
    pub author_name: String,
    pub author_email: String,
    pub timestamp: i64,
    pub offset_minutes: i32,
    pub parent_indices: Option<Vec<usize>>,
    pub is_merge: bool,
    pub graph_prefix: String,
    /// Lane index per character in `graph_prefix`; 255 = separator/no-lane
    pub graph_prefix_lanes: Vec<u8>,
}

impl CommitInfo {
    /// Empty slot for preallocated buffers that parallel loaders fill in place.
    pub(crate) fn placeholder() -> Self {
        Self {
            hash: String::new(),
            message: String::new(),
            full_message: None,
            author_name: String::new(),
            author_email: String::new(),
            timestamp: 0,
            offset_minutes: 0,
            parent_indices: None,
            is_merge: false,
            graph_prefix: String::new(),
            graph_prefix_lanes: Vec::new(),
        }
    }

    /// First 7 hex chars of the hash, gitk-style.
    pub fn short_hash(&self) -> &str {
        &self.hash[..7.min(self.hash.len())]
    }

    pub fn date_display_text(&self) -> Cow<'_, str> {
        self.date_text("%Y-%m-%d %H:%M")
    }

    /// Full precision for the edit popup: what is shown is exactly what a
    /// save round-trips (seconds included).
    pub fn date_edit_text(&self) -> Cow<'_, str> {
        self.date_text("%Y-%m-%d %H:%M:%S")
    }

    fn date_text(&self, fmt: &str) -> Cow<'_, str> {
        let offset_secs = self.offset_minutes * 60;
        let fixed = chrono::FixedOffset::east_opt(offset_secs)
            .unwrap_or_else(|| chrono::FixedOffset::east_opt(0).unwrap());
        match chrono::TimeZone::timestamp_opt(&fixed, self.timestamp, 0) {
            chrono::LocalResult::Single(dt) => Cow::Owned(dt.format(fmt).to_string()),
            _ => Cow::Owned(format!("{}", self.timestamp)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    Branch,
    RemoteBranch,
    Tag,
}

/// A branch/tag decoration shown next to a commit, gitk-style.
#[derive(Debug, Clone, PartialEq)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
    /// True for the currently checked-out branch.
    pub is_head: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitStats {
    pub files_added: usize,
    pub files_modified: usize,
    pub files_deleted: usize,
    pub lines_inserted: usize,
    pub lines_deleted: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StageState {
    Staged,
    Unstaged,
    Untracked,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileStatus {
    pub path: String,
    pub stage_state: StageState,
    pub status_char: char,
}
