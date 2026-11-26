//! The [`App`] struct itself: shared state, construction, and the few
//! cross-cutting helpers every screen uses. Behaviour lives in the sibling
//! modules (`input`, `mouse`, `layout`, `commit_list`, `history_load`, …).

use crate::app::editor::MultilineEditor;
use crate::app::features::commit_list::DrillFrame;
use crate::app::features::edit::EditState;
use crate::app::features::history_load::{
    CommitGraphWarmupResult, HistoryCountResult, InitialHistoryResult, spawn_initial_history_loader,
};
use crate::app::features::search::SearchState;
use crate::app::features::squash::SquashMessageState;
use crate::app::features::staging::StagingState;
use crate::app::models::{
    CommitDiffFile, CommitListFocus, CommitListView, ConfirmAction, ContextMenuState, Mode,
    ResizeTarget, SquashState, TextSelection, VisibleRow,
};
use crate::app::nav::Navigator;
use crate::events::{EventBus, ScreenId, TraceLevel};
use crate::git;
use crate::settings::{RepoSettings, Settings, Splits};
use arboard::Clipboard;
use ratatui::widgets::ListState;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

pub struct App {
    pub repo_path: PathBuf,
    pub commits: Vec<git::CommitInfo>,
    /// Fast hash → index lookup into `commits`; rebuilt whenever the commit
    /// list is replaced. Avoids O(n) scans on every lazy-load/copy action.
    pub(crate) commit_index: HashMap<String, usize>,
    pub list_state: ListState,
    pub visible: Vec<VisibleRow>,
    pub collapsed_merges: HashSet<String>,
    pub collapsed_hidden_flags: Vec<bool>,
    /// Masked views (collapse/drill/filter) compute row prefix overrides in
    /// one hidden-aware walk; true once that walk ran for the current mask.
    pub(crate) masked_prefixes_ready: bool,
    /// Drill-down levels into collapsed branches (Enter on a placeholder row);
    /// top frame scopes the visible list to that branch's hidden commits.
    pub drill_stack: Vec<DrillFrame>,
    /// Breadcrumb window start after the user paged it with "…"; None
    /// follows the deepest levels. Reset whenever the drill stack changes.
    pub(crate) breadcrumb_start: Option<usize>,
    pub branch_commit_cache: HashMap<String, Vec<usize>>,
    /// commit idx -> mainline commit that merged it in (u32::MAX = on the
    /// mainline). Computed lazily from in-memory topology; see
    /// `git::compute_merge_points`.
    pub(crate) merge_points: Option<Vec<u32>>,
    /// Hash of the commit the last `m` jump started from; `b` returns to
    /// it. A hash, not an index: it survives a reload of the list.
    pub(crate) merge_jump_origin: Option<String>,
    /// Incremental lane engine for the Graph view.
    pub(crate) classic_engine: git::ClassicEngine,
    /// gitk-style engine for the Focused view.
    pub(crate) tree_engine: git::TreeEngine,
    /// Which engine last wrote the shared per-commit prefix storage.
    pub(crate) prefix_writer: Option<git::GraphEngineKind>,
    pub commit_list_view: CommitListView,
    /// Commit ordering passed to `git log` (gitk default is topological).
    pub commit_sort: git::CommitSort,
    /// Hash of the commit HEAD points at; drawn with gitk's HEAD node glyph.
    pub head_hash: Option<String>,
    /// Full hash → branch/tag labels drawn before the commit message, gitk-style.
    pub ref_labels: HashMap<String, Vec<git::RefLabel>>,
    /// Commit to re-select once a reload batch contains it.
    pub(crate) pending_reselect_hash: Option<String>,
    /// Screen/popup navigation stack; read the active mode via [`App::mode`].
    pub nav: Navigator,
    pub edit: Option<EditState>,
    pub staging: Option<StagingState>,
    pub squash: Option<SquashState>,
    pub stash: Option<crate::app::features::stash::StashState>,
    pub working_tree: crate::app::features::working_tree::WorkingTreeState,
    /// Scroll positions of the last drawn frame (see `DrawnScroll`).
    pub(crate) drawn_scroll: crate::app::layout::DrawnScroll,
    pub move_state: Option<crate::app::models::MoveState>,
    pub squash_msg: Option<SquashMessageState>,
    pub commit_msg_editor: Option<MultilineEditor>,
    pub editor_save_selected: bool,
    pub stats_cache: HashMap<String, git::CommitStats>,
    pub commit_diff_cache: HashMap<String, Vec<String>>,
    pub commit_diff_file_cache: HashMap<String, Vec<CommitDiffFile>>,
    pub commit_diff_scroll: usize,
    pub commit_diff_info_scroll: usize,
    /// File the patch pane is narrowed to, picked in the Files column, with
    /// the diff key it was picked for; `None` = all files.
    pub diff_file_only: Option<(String, usize)>,
    pub diff_files_state: ListState,
    pub show_commit_diff: bool,
    pub commit_list_focus: CommitListFocus,
    pub show_help: bool,
    /// First help line shown, when the text is longer than the popup.
    pub(crate) help_scroll: usize,
    pub context_menu: Option<ContextMenuState>,
    pub text_selection: Option<TextSelection>,
    pub(crate) text_selection_drag_start: Option<(u16, u16)>,
    pub(crate) last_left_click: Option<(Instant, u16, u16)>,
    pub(crate) clipboard: Option<Clipboard>,
    pub search: SearchState,
    /// hash → A/M/D/R for the active file search (see search.active_file_patterns).
    pub(crate) file_status_cache: HashMap<String, Option<char>>,
    pub search_file_candidates: Vec<String>,
    pub status_message: Option<String>,
    pub selected_stats_due_at: Option<Instant>,
    pub pending_perf_action: Option<&'static str>,
    pub pending_perf_metrics: Vec<(&'static str, Duration)>,
    pub history_loading: bool,
    pub history_total_count: Option<usize>,
    pub(crate) initial_history_loader_rx: Option<Receiver<InitialHistoryResult>>,
    pub(crate) history_count_rx: Option<Receiver<HistoryCountResult>>,
    pub(crate) commit_graph_warmup_rx: Option<Receiver<CommitGraphWarmupResult>>,
    /// When true, skip confirmation dialogs and execute destructive actions immediately.
    pub yes: bool,
    /// One-shot request for a full terminal repaint (Ctrl+L). Heals stale
    /// cells when the terminal's real content drifted from ratatui's diff
    /// state (seen on macOS Terminal.app under heavy update bursts).
    pub force_redraw: bool,
    pub event_bus: EventBus,
    /// Loaded settings (global + local override) and where to save them back.
    pub settings: Settings,
    /// Live pane split preferences; mouse border drags mutate these.
    pub splits: Splits,
    /// Active border drag, if the user is resizing a pane.
    pub(crate) resize_drag: Option<ResizeTarget>,
    /// Minimum trace level shown on the debugger screen (settings `debug`).
    pub trace_min_level: TraceLevel,
    /// Show app events (KEY/SCRN/ACT) on the debugger screen.
    pub debug_show_events: bool,
}

impl App {
    pub fn new(repo_path: PathBuf, yes: bool) -> Self {
        // History loads in the background; the UI starts empty and fills in
        // as batches arrive (see `history_load`).
        let commit_sort = git::CommitSort::default();
        let head_hash = git::head_commit_hash(&repo_path);
        let ref_labels = git::load_ref_labels(&repo_path);
        let initial_history_loader_rx =
            spawn_initial_history_loader(repo_path.clone(), commit_sort);
        let settings = Settings::load(&repo_path);
        let commit_list_view = match settings.resolved.view.as_deref() {
            Some("simple") => CommitListView::Simple,
            Some("graph") => CommitListView::Graph,
            _ => CommitListView::Focused,
        };
        let show_commit_diff = settings.resolved.show_details.unwrap_or(false);
        let splits = settings.resolved.splits.unwrap_or_default().clamped();
        let trace_min_level = settings
            .resolved
            .debug_level()
            .and_then(TraceLevel::from_setting)
            .unwrap_or(TraceLevel::Info);
        let debug_show_events = settings.resolved.debug_events().unwrap_or(true);
        tracing::info!("opening repository {}", repo_path.display());
        let mut app = Self {
            repo_path,
            commits: Vec::new(),
            commit_index: HashMap::new(),
            list_state: ListState::default(),
            visible: Vec::new(),
            collapsed_merges: HashSet::new(),
            collapsed_hidden_flags: Vec::new(),
            masked_prefixes_ready: false,
            drill_stack: Vec::new(),
            breadcrumb_start: None,
            branch_commit_cache: HashMap::new(),
            merge_points: None,
            merge_jump_origin: None,
            classic_engine: git::ClassicEngine::default(),
            tree_engine: git::TreeEngine::default(),
            prefix_writer: None,
            commit_list_view,
            commit_sort,
            head_hash,
            ref_labels,
            pending_reselect_hash: None,
            nav: Navigator::new(),
            edit: None,
            staging: None,
            squash: None,
            move_state: None,
            stash: None,
            working_tree: Default::default(),
            drawn_scroll: Default::default(),
            squash_msg: None,
            commit_msg_editor: None,
            editor_save_selected: true,
            stats_cache: HashMap::new(),
            commit_diff_cache: HashMap::new(),
            commit_diff_file_cache: HashMap::new(),
            commit_diff_scroll: 0,
            commit_diff_info_scroll: 0,
            diff_file_only: None,
            diff_files_state: ListState::default(),
            show_commit_diff,
            commit_list_focus: CommitListFocus::List,
            show_help: false,
            help_scroll: 0,
            context_menu: None,
            text_selection: None,
            text_selection_drag_start: None,
            last_left_click: None,
            clipboard: Clipboard::new().ok(),
            search: SearchState::default(),
            file_status_cache: HashMap::new(),
            search_file_candidates: Vec::new(),
            status_message: None,
            selected_stats_due_at: None,
            pending_perf_action: None,
            pending_perf_metrics: Vec::new(),
            history_loading: true,
            history_total_count: None,
            initial_history_loader_rx: Some(initial_history_loader_rx),
            history_count_rx: None,
            commit_graph_warmup_rx: None,
            yes,
            force_redraw: false,
            event_bus: EventBus::new(500),
            settings,
            splits,
            resize_drag: None,
            trace_min_level,
            debug_show_events,
        };
        app.status_message = Some("Loading initial history...".into());
        app.refresh_working_tree();
        app.schedule_selected_stats_load();
        app
    }

    /// Called once per drawn frame; the wheel handlers measure from this.
    pub fn remember_drawn_scroll(&mut self) {
        self.drawn_scroll = crate::app::layout::DrawnScroll {
            info: self.commit_diff_info_scroll,
            diff: self.commit_diff_scroll,
            details: self.working_tree.details_scroll,
            staging_diff: self.staging.as_ref().map_or(0, |s| s.diff_scroll),
            stash_diff: self.stash.as_ref().map_or(0, |s| s.diff_scroll),
            help: self.help_scroll,
        };
    }

    /// The active mode (top of the navigation stack).
    pub fn mode(&self) -> &Mode {
        self.nav.mode()
    }

    /// Persist the current view/details/splits for this repo (called on quit).
    pub fn save_settings(&self) {
        let canonical = self
            .repo_path
            .canonicalize()
            .unwrap_or_else(|_| self.repo_path.clone());
        let origin = crate::settings::origin_url(&canonical);
        let short_name = origin
            .as_deref()
            .map(crate::settings::short_name_of)
            .unwrap_or_else(|| self.settings.repo_key.clone());
        self.settings.save(RepoSettings {
            short_name: Some(short_name),
            path: Some(canonical.to_string_lossy().into_owned()),
            origin,
            view: Some(self.commit_list_view.label().into()),
            show_details: Some(self.show_commit_diff),
            debug: Some(crate::settings::DebugSetting::Full(
                crate::settings::DebugSettings {
                    log_level: Some(self.trace_min_level.setting_str().into()),
                    log_events: Some(self.debug_show_events),
                },
            )),
            logs: self.settings.resolved.logs.clone(),
            splits: Some(self.splits),
        });
    }

    /// O(1) lookup of a commit's index by full hash.
    pub(crate) fn commit_position(&self, hash: &str) -> Option<usize> {
        self.commit_index.get(hash).copied()
    }

    pub(crate) fn commit_by_hash(&self, hash: &str) -> Option<&git::CommitInfo> {
        self.commit_position(hash).map(|idx| &self.commits[idx])
    }

    /// Rebuild the hash → index map after `commits` changed.
    pub(crate) fn reindex_commits(&mut self) {
        self.commit_index = self
            .commits
            .iter()
            .enumerate()
            .map(|(idx, commit)| (commit.hash.clone(), idx))
            .collect();
    }

    /// Replace the full commit list after a history-rewriting operation
    /// (commit, squash, edit) and rebuild all derived state.
    pub(crate) fn replace_commits(&mut self, commits: Vec<git::CommitInfo>) {
        self.commits = commits;
        self.head_hash = git::head_commit_hash(&self.repo_path);
        self.ref_labels = git::load_ref_labels(&self.repo_path);
        self.reindex_commits();
        self.collapsed_merges.clear();
        self.drill_stack.clear();
        self.reset_graph_cache();
        self.visible = (0..self.commits.len())
            .map(|idx| VisibleRow::Commit {
                idx,
                prefix_override: None,
                depth: 0,
            })
            .collect();
        self.ensure_working_tree_row();
        self.refresh_working_tree();
        // The rewrite changed hashes, so the selected commit's stats/diff
        // caches no longer match — reload them or the details pane stays on
        // its "Loading commit data..." placeholder forever.
        self.schedule_selected_stats_load();
    }

    pub(crate) fn mark_perf_action(&mut self, name: &'static str) {
        self.pending_perf_action = Some(name);
    }

    pub(crate) fn push_perf_metric(&mut self, name: &'static str, duration: Duration) {
        self.pending_perf_metrics.push((name, duration));
    }

    pub fn take_perf_action(&mut self) -> Option<&'static str> {
        self.pending_perf_action.take()
    }

    pub fn drain_perf_metrics(&mut self) -> Vec<(&'static str, Duration)> {
        std::mem::take(&mut self.pending_perf_metrics)
    }

    /// Map current mode to a human-readable screen identifier.
    pub fn current_screen(&self) -> ScreenId {
        match self.mode() {
            Mode::Normal => ScreenId::CommitList,
            Mode::Search => ScreenId::CommitList,
            Mode::Edit => ScreenId::CommitEdit,
            Mode::EditCommitField => ScreenId::CommitEditMessage,
            Mode::Staging => ScreenId::Staging,
            Mode::Stash
            | Mode::Confirm(ConfirmAction::StashPop { .. } | ConfirmAction::StashDrop { .. }, _) => {
                ScreenId::Stash
            }
            Mode::EditCommitMessage => ScreenId::StagingMessageEditor,
            Mode::Squash => ScreenId::SquashPicker,
            Mode::Move
            | Mode::Confirm(ConfirmAction::Move { .. } | ConfirmAction::MoveConflict { .. }, _) => {
                ScreenId::CommitList
            }
            Mode::SquashMessage => ScreenId::SquashMessageEditor,
            Mode::Confirm(ConfirmAction::Commit { .. }, _) => ScreenId::Staging,
            Mode::Confirm(ConfirmAction::RemoveOrSegment { .. }, _) => ScreenId::CommitList,
            Mode::Confirm(ConfirmAction::SaveEdit, _) => ScreenId::CommitEdit,
            Mode::Confirm(ConfirmAction::SaveEditFromMessageEditor, _) => {
                ScreenId::CommitEditMessage
            }
            Mode::Confirm(ConfirmAction::DiscardEditMessageEditor, _) => {
                ScreenId::CommitEditMessage
            }
            Mode::Confirm(ConfirmAction::DiscardStagingMessageEditor, _) => {
                ScreenId::StagingMessageEditor
            }
            Mode::Confirm(ConfirmAction::DiscardSquashMessageEditor, _) => {
                ScreenId::SquashMessageEditor
            }
            Mode::Confirm(ConfirmAction::Squash { .. }, _) => ScreenId::SquashPicker,
            Mode::Debugger => ScreenId::Debugger,
        }
    }

    /// Toggle the debugger overlay on/off. It sits on the navigation stack,
    /// so closing it automatically restores whatever was underneath.
    pub fn toggle_debugger(&mut self) {
        if matches!(self.mode(), Mode::Debugger) {
            self.nav.pop();
        } else {
            self.nav.push(Mode::Debugger);
            self.event_bus.jump_to_end();
        }
    }

    /// Emit a Debug-level trace event.
    #[allow(dead_code)]
    pub fn trace(&mut self, msg: impl Into<String>) {
        self.event_bus.trace(msg);
    }

    pub(crate) fn multiline_editor_changed(&self) -> bool {
        match self.mode() {
            Mode::EditCommitMessage => self.commit_message_editor_has_changes(),
            Mode::EditCommitField => self.commit_field_editor_has_changes(),
            Mode::SquashMessage => self.squash_message_has_changes(),
            _ => false,
        }
    }

    pub(crate) fn cancel_multiline_editor(&mut self) {
        match self.mode() {
            Mode::EditCommitMessage => {
                if self.commit_message_editor_has_changes() {
                    self.nav.push(Mode::Confirm(
                        ConfirmAction::DiscardStagingMessageEditor,
                        false,
                    ));
                } else {
                    self.discard_commit_message_editor();
                }
            }
            Mode::EditCommitField => {
                if self.commit_field_editor_has_changes() {
                    self.nav.push(Mode::Confirm(
                        ConfirmAction::DiscardEditMessageEditor,
                        false,
                    ));
                } else {
                    self.discard_commit_field_editor();
                }
            }
            _ => {}
        }
    }

    pub(crate) fn discard_multiline_editor(&mut self) {
        match self.mode() {
            Mode::EditCommitMessage => self.discard_commit_message_editor(),
            Mode::EditCommitField => self.discard_commit_field_editor(),
            _ => {}
        }
    }

    pub(crate) fn escape_multiline_editor(&mut self) {
        if let Some(editor) = self.commit_msg_editor.as_mut()
            && editor.insert_mode
        {
            editor.exit_insert_mode();
        } else {
            self.discard_multiline_editor();
        }
    }

    pub(crate) fn activate_multiline_editor_action(&mut self) -> bool {
        if self.editor_save_selected {
            match self.mode() {
                Mode::EditCommitMessage => return self.commit_editor_confirm(),
                Mode::EditCommitField => self.confirm_commit_field_editor(),
                _ => {}
            }
        } else {
            self.cancel_multiline_editor();
        }
        false
    }
}
