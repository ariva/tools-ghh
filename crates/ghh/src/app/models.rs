use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmAction {
    /// Rewrite commit history via git::modify_commit.
    SaveEdit,
    /// Rewrite commit history after confirming from the multiline message editor.
    SaveEditFromMessageEditor,
    /// Discard changes in commit-edit multiline message editor.
    DiscardEditMessageEditor,
    /// Discard changes in staging multiline commit message editor.
    DiscardStagingMessageEditor,
    /// Discard changes in squash multiline message editor.
    DiscardSquashMessageEditor,
    /// Commit staged files with the given message.
    Commit { msg: String },
    /// Remove one OR-joined segment (joiner + text) from a search input.
    RemoveOrSegment {
        field_index: usize,
        segment_index: usize,
        preview: String,
    },
    /// Squash selected commits into the base commit.
    Squash {
        base_hash: String,
        squash_hashes: Vec<String>,
        message: String,
    },
    /// Reorder commits: rebase from `lowest_hash` with `todo` (oldest first).
    Move {
        lowest_hash: String,
        todo: Vec<String>,
        /// "a1b2c3d 2 positions up" — for the prompt.
        summary: String,
    },
    /// A move (rebase) stopped on a conflict: Yes = continue, No = abort.
    MoveConflict { stopped_at: String },
    /// Apply a stash to the working tree and remove it from the stash list.
    StashPop { entry: crate::git::StashEntry },
    /// Delete a stash without applying it.
    StashDrop { entry: crate::git::StashEntry },
}

impl ConfirmAction {
    pub fn is_dangerous(&self) -> bool {
        matches!(
            self,
            ConfirmAction::SaveEdit
                | ConfirmAction::SaveEditFromMessageEditor
                | ConfirmAction::Squash { .. }
                | ConfirmAction::Move { .. }
                | ConfirmAction::StashDrop { .. }
        )
    }

    pub fn prompt_lines(&self) -> Vec<String> {
        match self {
            ConfirmAction::SaveEdit => vec![
                "Rewrite commit history?".into(),
                "This will amend the selected commit and rebase all descendants.".into(),
            ],
            ConfirmAction::SaveEditFromMessageEditor => vec![
                "Save edited commit message?".into(),
                "This will rewrite the selected commit and rebase all descendants.".into(),
            ],
            ConfirmAction::DiscardEditMessageEditor => vec![
                "Discard edited commit message?".into(),
                "Current message editor changes will be lost.".into(),
            ],
            ConfirmAction::DiscardStagingMessageEditor => vec![
                "Discard commit message changes?".into(),
                "Current message editor changes will be lost.".into(),
            ],
            ConfirmAction::DiscardSquashMessageEditor => vec![
                "Discard squash message changes?".into(),
                "Current message editor changes will be lost.".into(),
            ],
            ConfirmAction::Commit { msg } => vec![
                "Create a new commit?".into(),
                format!("Message: \"{}\"", msg),
            ],
            ConfirmAction::RemoveOrSegment { preview, .. } => vec![
                format!("Remove \"{preview}\" from the search input?"),
                "The joiner and its text are removed together.".into(),
            ],
            ConfirmAction::Squash {
                squash_hashes,
                message,
                ..
            } => vec![
                format!(
                    "Squash {} commit{} into base?",
                    squash_hashes.len(),
                    if squash_hashes.len() == 1 { "" } else { "s" }
                ),
                format!("Message: \"{}\"", message.lines().next().unwrap_or("")),
                "This will rewrite commit history and cannot be undone.".into(),
            ],
            // Popup inner width is 54 chars, 4 lines max — keep these short.
            ConfirmAction::Move { todo, summary, .. } => vec![
                format!("Move {summary}?"),
                format!("Rewrites history: {} commits get new hashes", todo.len()),
                "(every commit from the lowest moved row up to HEAD).".into(),
                "Already pushed? A force-push will be needed.".into(),
            ],
            ConfirmAction::MoveConflict { stopped_at } => vec![
                format!(
                    "Rebase stopped at {}",
                    stopped_at.chars().take(36).collect::<String>()
                ),
                "Fix the conflict and stage the files (git add).".into(),
                "Yes: continue    No: abort, restore old history".into(),
                "Esc: decide later (m reopens this)".into(),
            ],
            ConfirmAction::StashPop { entry } => vec![
                format!("Pop {}?", entry.name),
                entry.message.chars().take(52).collect(),
                "Applies it to the working tree, then removes it".into(),
                "from the stash list (kept if it conflicts).".into(),
            ],
            ConfirmAction::StashDrop { entry } => vec![
                format!("Drop {}?", entry.name),
                entry.message.chars().take(52).collect(),
                "Its changes are deleted without being applied.".into(),
                "This cannot be undone from this tool.".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Normal,
    Search,
    Edit,
    Staging,
    /// Interactive squash: pick base + commits to squash into it.
    Squash,
    /// Multi-line commit message editor shown before a squash is executed.
    SquashMessage,
    /// Reordering: the picked commit follows ↑/↓ until Enter/Esc.
    Move,
    /// Stash browser: list, patch, details; apply / pop / drop.
    Stash,
    /// Multi-line commit message editor opened from the staging view.
    EditCommitMessage,
    /// Multi-line message editor opened from within the commit-edit popup.
    EditCommitField,
    /// Debugger event-log overlay.
    Debugger,
    /// Waiting for confirmation before executing a destructive action.
    /// The `bool` is `true` when the **Yes** button is selected (default).
    Confirm(ConfirmAction, bool),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditField {
    Message,
    AuthorName,
    AuthorEmail,
    Date,
}

impl EditField {
    pub fn next(&self) -> Self {
        match self {
            EditField::Message => EditField::AuthorName,
            EditField::AuthorName => EditField::AuthorEmail,
            EditField::AuthorEmail => EditField::Date,
            EditField::Date => EditField::Message,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            EditField::Message => EditField::Date,
            EditField::AuthorName => EditField::Message,
            EditField::AuthorEmail => EditField::AuthorName,
            EditField::Date => EditField::AuthorEmail,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            EditField::Message => "Message",
            EditField::AuthorName => "Author Name",
            EditField::AuthorEmail => "Author Email",
            EditField::Date => "Date (YYYY-MM-DD HH:MM[:SS])",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CommitListFocus {
    List,
    DiffInfo,
    /// Changed files column left of the patch.
    Files,
    Diff,
    /// Bottom Details pane; only for the "Uncommitted changes" row, whose
    /// file list can be longer than the pane.
    Details,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommitDiffFile {
    pub path: String,
    pub diff_line: usize,
    pub additions: usize,
    pub deletions: usize,
    /// What happened to the file: A(dded) M(odified) D(eleted) R(enamed).
    pub status: char,
}

pub fn parse_diff_files(lines: &[String]) -> Vec<CommitDiffFile> {
    let mut files = Vec::new();
    let mut current: Option<CommitDiffFile> = None;

    for (line_index, line) in lines.iter().enumerate() {
        if let Some(path) = diff_header_path(line) {
            if let Some(file) = current.take() {
                files.push(file);
            }
            current = Some(CommitDiffFile {
                path,
                diff_line: line_index,
                additions: 0,
                deletions: 0,
                status: 'M',
            });
            continue;
        }

        let Some(file) = current.as_mut() else {
            continue;
        };
        if let Some(path) = line.strip_prefix("+++ b/") {
            file.path = path.to_string();
        } else if line.starts_with("new file mode") {
            file.status = 'A';
        } else if line.starts_with("deleted file mode") {
            file.status = 'D';
        } else if line.starts_with("rename from ") {
            file.status = 'R';
        } else if line.starts_with('+') && !line.starts_with("+++") {
            file.additions += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            file.deletions += 1;
        }
    }

    if let Some(file) = current {
        files.push(file);
    }
    files
}

fn diff_header_path(line: &str) -> Option<String> {
    let header = line.strip_prefix("diff --git a/")?;
    header.split_once(" b/").map(|(_, path)| path.to_string())
}

/// Which pane border a mouse drag is resizing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeTarget {
    /// Vertical border between the commit/file list and the diff column
    /// (shared by the commit list and staging screens).
    ListDiff,
    /// Horizontal border between the commit-info and patch panes.
    InfoPatch,
    /// Vertical border between the Files column and the patch pane.
    FilesPatch,
    /// Top border of the details footer.
    DetailsTop,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CommitListView {
    Simple,
    Graph,
    /// gitk-style layout: token-ordered lanes, arrow cuts, per-line colors.
    Focused,
}

impl CommitListView {
    pub fn next(self) -> Self {
        match self {
            CommitListView::Focused => CommitListView::Graph,
            CommitListView::Graph => CommitListView::Simple,
            CommitListView::Simple => CommitListView::Focused,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            CommitListView::Focused => CommitListView::Simple,
            CommitListView::Graph => CommitListView::Focused,
            CommitListView::Simple => CommitListView::Graph,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CommitListView::Simple => "simple",
            CommitListView::Graph => "graph",
            CommitListView::Focused => "focused",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContextMenuAction {
    CopySelection,
    ViewDetails,
    HideDetails,
    CopyHash,
    CopyAuthor,
    CopyCommitMessage,
    CopyChangedFiles,
    CopyAllInfo,
    CopyDiffPatch,
    /// Jump the cursor to the mainline merge that brought this commit in (m).
    GotoMergeCommit,
    /// Drill into that merge with this commit selected (M / Shift+Enter).
    DrillIntoMerge,
    PasteClipboard,
    EditCommit,
    SquashFromHere,
}

impl ContextMenuAction {
    pub fn is_dynamic(self) -> bool {
        matches!(self, Self::CopySelection | Self::PasteClipboard)
    }

    /// Rewrites history — rendered below a divider so the eye learns the
    /// fence between "look" and "touch".
    pub fn is_destructive(self) -> bool {
        matches!(self, Self::EditCommit | Self::SquashFromHere)
    }

    pub fn label(self, clipboard_available: bool) -> &'static str {
        match self {
            Self::CopySelection => "Copy selection  Ctrl+C",
            Self::ViewDetails => "View details",
            Self::HideDetails => "Hide details",
            Self::CopyHash => "Copy hash",
            Self::CopyAuthor => "Copy author",
            Self::CopyCommitMessage => "Copy commit message",
            Self::CopyChangedFiles => "Copy changed files info",
            Self::CopyAllInfo => "Copy info",
            Self::CopyDiffPatch => "Copy diff/patch",
            Self::GotoMergeCommit => "Go to merge commit  m",
            Self::DrillIntoMerge => "Drill into merge  M",
            Self::EditCommit => "Edit commit\u{2026}",
            Self::SquashFromHere => "Squash from here\u{2026}",
            Self::PasteClipboard if clipboard_available => "Paste from clipboard",
            Self::PasteClipboard => "No clipboard",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextMenuState {
    pub x: u16,
    pub y: u16,
    pub actions: Vec<ContextMenuAction>,
    pub selected: usize,
    pub clipboard_available: bool,
}

impl ContextMenuState {
    pub fn has_separator_after(&self, index: usize) -> bool {
        index + 1 < self.actions.len()
            && (self.actions[index].is_dynamic()
                || matches!(
                    self.actions[index + 1],
                    ContextMenuAction::ViewDetails | ContextMenuAction::HideDetails
                )
                // The danger fence: safe actions above, history-rewriting below.
                || (!self.actions[index].is_destructive()
                    && self.actions[index + 1].is_destructive()))
    }

    pub fn content_line_count(&self) -> usize {
        self.actions.len()
            + (0..self.actions.len())
                .filter(|&index| self.has_separator_after(index))
                .count()
    }

    pub fn action_index_at_content_row(&self, row: usize) -> Option<usize> {
        let mut content_row = 0;
        for index in 0..self.actions.len() {
            if row == content_row {
                return Some(index);
            }
            content_row += 1;
            if self.has_separator_after(index) {
                if row == content_row {
                    return None;
                }
                content_row += 1;
            }
        }
        None
    }

    pub fn shortcut_for(&self, index: usize) -> usize {
        if self
            .actions
            .first()
            .is_some_and(|action| action.is_dynamic())
        {
            index
        } else {
            index + 1
        }
    }

    pub fn action_index_for_shortcut(&self, shortcut: usize) -> Option<usize> {
        self.actions
            .iter()
            .enumerate()
            .find_map(|(index, _)| (self.shortcut_for(index) == shortcut).then_some(index))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextSelection {
    pub start_column: u16,
    pub start_row: u16,
    pub end_column: u16,
    pub end_row: u16,
    pub text: String,
    pub select_word: bool,
    pub expand_next_word: bool,
    pub extend_to_row_end: bool,
    pub extend_to_panel_end: bool,
    pub select_all_panel: bool,
    pub word_selection_level: u8,
    pub panel_start_column: u16,
    pub panel_start_row: u16,
    pub panel_end_column: u16,
    pub panel_end_row: u16,
    pub all_panel_text: Option<String>,
}

/// A row in the visible commit list — either a real commit or a collapsed-branch placeholder.
#[derive(Debug, Clone)]
pub enum VisibleRow {
    Commit {
        idx: usize,
        /// Masked views (collapse/drill/filter) compute their own window
        /// prefixes; `None` means "read the commit's own prefix" — the plain
        /// list never clones prefix strings into rows.
        prefix_override: Option<(String, Vec<u8>)>,
        /// Nesting level under a group header (0 = top level). The renderer
        /// indents purely from this value, so any row producer (full list,
        /// drill scope, future filters) renders correctly.
        depth: usize,
    },
    CollapsedBranch {
        count: usize,
        merge_hash: String,
    },
    /// The drilled-into merge commit pinned at the top of a drill view.
    /// Read-only: not a `Commit` row, so drill/collapse/edit ignore it.
    /// Carries a graph prefix so the tree connects it to its branch rows.
    DrillHeader {
        idx: usize,
        prefix_override: Option<(String, Vec<u8>)>,
    },
    /// "Uncommitted changes" on top of the top-level list (see
    /// `features/working_tree.rs`). Not a commit: every commit action
    /// ignores it.
    WorkingTree,
}

/// State for the interactive squash mode.
pub struct SquashState {
    /// Index into `App::commits` of the commit that will be kept.
    pub base_idx: usize,
    /// Indices into `App::commits` of commits to absorb into the base.
    pub squash_set: HashSet<usize>,
}

/// State for move mode: a live preview only, git is untouched until confirm.
pub struct MoveState {
    /// Indices into `App::commits` of the movable run, newest first.
    pub chain: Vec<usize>,
    /// Chain position the commit came from / currently previews at.
    pub from: usize,
    pub pos: usize,
}

impl MoveState {
    /// Which commit a row shows while the move is previewed: the row that
    /// normally shows `idx` shows the returned commit instead.
    pub fn display_idx(&self, idx: usize) -> usize {
        let (lo, hi) = (self.from.min(self.pos), self.from.max(self.pos));
        let Some(k) = self.chain[lo..=hi]
            .iter()
            .position(|&i| i == idx)
            .map(|k| k + lo)
        else {
            return idx;
        };
        if k == self.pos {
            self.chain[self.from]
        } else if self.from < self.pos {
            self.chain[k + 1]
        } else {
            self.chain[k - 1]
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn move_state_previews_shifted_rows() {
        let chain = vec![10, 11, 12, 13];
        let shown = |from, pos| {
            let m = super::MoveState {
                chain: chain.clone(),
                from,
                pos,
            };
            chain.iter().map(|&i| m.display_idx(i)).collect::<Vec<_>>()
        };
        assert_eq!(shown(1, 1), [10, 11, 12, 13]);
        assert_eq!(shown(0, 2), [11, 12, 10, 13]); // moved down
        assert_eq!(shown(3, 1), [10, 13, 11, 12]); // moved up
    }

    #[test]
    fn diff_parser_detects_file_statuses() {
        let lines: Vec<String> = [
            "diff --git a/new.rs b/new.rs",
            "new file mode 100644",
            "+++ b/new.rs",
            "+hello",
            "diff --git a/gone.rs b/gone.rs",
            "deleted file mode 100644",
            "--- a/gone.rs",
            "-bye",
            "diff --git a/old.rs b/renamed.rs",
            "rename from old.rs",
            "rename to renamed.rs",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let statuses: Vec<char> = super::parse_diff_files(&lines)
            .into_iter()
            .map(|file| file.status)
            .collect();
        assert_eq!(statuses, vec!['A', 'D', 'R']);
    }

    use super::{ContextMenuAction, ContextMenuState};

    /// Destructive entries sit last, fenced by a separator, and keep working
    /// number shortcuts.
    #[test]
    fn context_menu_fences_destructive_actions() {
        let menu = ContextMenuState {
            x: 0,
            y: 0,
            actions: vec![
                ContextMenuAction::CopyHash,
                ContextMenuAction::ViewDetails,
                ContextMenuAction::EditCommit,
                ContextMenuAction::SquashFromHere,
            ],
            selected: 0,
            clipboard_available: false,
        };
        assert!(ContextMenuAction::EditCommit.is_destructive());
        assert!(ContextMenuAction::SquashFromHere.is_destructive());
        assert!(!ContextMenuAction::CopyHash.is_destructive());
        // Fence right before the first destructive entry...
        assert!(menu.has_separator_after(1));
        // ...but not between the two destructive entries.
        assert!(!menu.has_separator_after(2));
        // Shortcuts stay continuous across the fence.
        assert_eq!(menu.action_index_for_shortcut(3), Some(2));
        assert_eq!(menu.action_index_for_shortcut(4), Some(3));
    }

    use super::parse_diff_files;

    #[test]
    fn parses_file_sections_and_line_counts() {
        let lines = vec![
            "diff --git a/src/a.rs b/src/a.rs".to_string(),
            "--- a/src/a.rs".to_string(),
            "+++ b/src/a.rs".to_string(),
            "+added".to_string(),
            "-removed".to_string(),
            "diff --git a/src/b.rs b/src/b.rs".to_string(),
            "+another".to_string(),
        ];

        assert_eq!(
            parse_diff_files(&lines),
            vec![
                super::CommitDiffFile {
                    path: "src/a.rs".into(),
                    diff_line: 0,
                    additions: 1,
                    deletions: 1,
                    status: 'M',
                },
                super::CommitDiffFile {
                    path: "src/b.rs".into(),
                    diff_line: 5,
                    additions: 1,
                    deletions: 0,
                    status: 'M',
                },
            ]
        );
    }
}
