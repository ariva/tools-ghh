//! Staging screen: stage/unstage files, review the diff, write the commit
//! message, and create the commit (`Mode::Staging` + `Mode::EditCommitMessage`).

use crate::app::App;
use crate::app::editor::MultilineEditor;
use crate::app::editor::{EditorKeyOutcome, handle_editor_key};
use crate::app::models::{ConfirmAction, Mode};
use crate::events::EventTime;
use crate::events::{ActionKind, AppEvent};
use crate::git;
use crate::git::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StagingFocus {
    FileList,
    Diff,
    CommitMessage,
}

pub struct StagingState {
    pub files: Vec<git::FileStatus>,
    pub list_state: ListState,
    pub diff_lines: Vec<String>,
    pub diff_scroll: usize,
    pub commit_msg: String,
    pub msg_cursor: usize,
    pub focus: StagingFocus,
    pub status_msg: Option<String>,
}

impl StagingState {
    pub fn new(repo_path: &std::path::Path) -> Result<Self> {
        let files = git::load_status(repo_path)?;
        let mut list_state = ListState::default();
        if !files.is_empty() {
            list_state.select(Some(0));
        }
        let mut state = Self {
            files,
            list_state,
            diff_lines: Vec::new(),
            diff_scroll: 0,
            commit_msg: String::new(),
            msg_cursor: 0,
            focus: StagingFocus::FileList,
            status_msg: None,
        };
        state.refresh_diff(repo_path);
        Ok(state)
    }

    pub fn refresh_diff(&mut self, repo_path: &std::path::Path) {
        self.diff_scroll = 0;
        if let Some(file) = self.selected_file() {
            self.diff_lines = match git::get_file_diff(repo_path, file) {
                Ok(diff) => diff.lines().map(String::from).collect(),
                Err(e) => vec![format!("Error loading diff: {e}")],
            };
        } else {
            self.diff_lines = Vec::new();
        }
    }

    pub fn reload(&mut self, repo_path: &std::path::Path) {
        match git::load_status(repo_path) {
            Ok(files) => {
                let prev = self.list_state.selected();
                self.files = files;
                let new_sel = if self.files.is_empty() {
                    None
                } else {
                    Some(prev.unwrap_or(0).min(self.files.len() - 1))
                };
                self.list_state.select(new_sel);
            }
            Err(e) => {
                self.status_msg = Some(format!("Reload error: {e}"));
            }
        }
        self.refresh_diff(repo_path);
    }

    pub fn selected_file(&self) -> Option<&git::FileStatus> {
        self.list_state.selected().and_then(|i| self.files.get(i))
    }

    pub fn has_staged(&self) -> bool {
        self.files
            .iter()
            .any(|f| f.stage_state == git::StageState::Staged)
    }

    /// Move the file selection by one and refresh the diff pane.
    pub fn select_next(&mut self, repo_path: &std::path::Path) {
        let len = self.files.len();
        if len == 0 {
            return;
        }
        let next = match self.list_state.selected() {
            Some(i) if i + 1 < len => i + 1,
            Some(i) => i,
            None => 0,
        };
        self.list_state.select(Some(next));
        self.refresh_diff(repo_path);
    }

    pub fn select_prev(&mut self, repo_path: &std::path::Path) {
        let prev = match self.list_state.selected() {
            Some(0) | None => 0,
            Some(i) => i - 1,
        };
        self.list_state.select(Some(prev));
        self.refresh_diff(repo_path);
    }

    /// Run a stage/unstage-style git op, surface its error, then reload.
    fn run_git_op(&mut self, repo_path: &std::path::Path, result: Result<()>) {
        self.status_msg = result.err().map(|e| format!("Error: {e}"));
        self.reload(repo_path);
    }

    pub fn msg_insert(&mut self, c: char) {
        let cursor = self.msg_cursor;
        let byte_pos = self
            .commit_msg
            .char_indices()
            .nth(cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.commit_msg.len());
        self.commit_msg.insert(byte_pos, c);
        self.msg_cursor += 1;
    }

    pub fn msg_delete_before(&mut self) {
        if self.msg_cursor == 0 {
            return;
        }
        let cursor = self.msg_cursor;
        let byte_pos = self
            .commit_msg
            .char_indices()
            .nth(cursor - 1)
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.commit_msg.remove(byte_pos);
        self.msg_cursor -= 1;
    }

    pub fn msg_delete_at(&mut self) {
        let cursor = self.msg_cursor;
        let len = self.commit_msg.chars().count();
        if cursor >= len {
            return;
        }
        let byte_pos = self
            .commit_msg
            .char_indices()
            .nth(cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.commit_msg.len());
        self.commit_msg.remove(byte_pos);
    }

    pub fn msg_move_left(&mut self) {
        if self.msg_cursor > 0 {
            self.msg_cursor -= 1;
        }
    }

    pub fn msg_move_right(&mut self) {
        let len = self.commit_msg.chars().count();
        if self.msg_cursor < len {
            self.msg_cursor += 1;
        }
    }

    pub fn msg_cursor_end(&mut self) {
        self.msg_cursor = self.commit_msg.chars().count();
    }

    pub fn msg_insert_text(&mut self, text: &str) {
        for ch in text.chars() {
            match ch {
                '\r' => {}
                '\n' => self.msg_insert(' '),
                c => self.msg_insert(c),
            }
        }
    }
}

// ── App impl: staging mode ────────────────────────────────────────────────────

impl App {
    pub(crate) fn enter_staging(&mut self) {
        match StagingState::new(&self.repo_path) {
            Ok(s) => {
                self.staging = Some(s);
                self.nav.push(Mode::Staging);
                self.status_message = None;
            }
            Err(e) => {
                self.status_message = Some(format!("Error loading status: {e}"));
            }
        }
    }

    pub(crate) fn handle_staging(&mut self, key: KeyEvent) -> bool {
        let repo_path = self.repo_path.clone();
        let Some(staging) = self.staging.as_mut() else {
            self.nav.reset(Mode::Normal);
            return false;
        };

        // Enter in the message field commits; Alt+Enter opens the big editor.
        if staging.focus == StagingFocus::CommitMessage && key.code == KeyCode::Enter {
            if key.modifiers.contains(KeyModifiers::ALT) {
                self.enter_commit_message_editor();
                return false;
            }
            let msg = staging.commit_msg.clone();
            let has_staged = staging.has_staged();
            return self.request_commit(msg, has_staged);
        }

        match staging.focus {
            StagingFocus::FileList => match key.code {
                KeyCode::Char('q') => return true,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
                KeyCode::Enter => {
                    if staging.has_staged() {
                        self.enter_commit_message_editor();
                    }
                }
                KeyCode::Esc => {
                    self.staging = None;
                    self.nav.pop();
                    self.refresh_working_tree();
                }
                KeyCode::Down | KeyCode::Char('j') => staging.select_next(&repo_path),
                KeyCode::Up | KeyCode::Char('k') => staging.select_prev(&repo_path),
                KeyCode::Char(' ') => {
                    if let Some(file) = staging.selected_file().cloned() {
                        let result = if file.stage_state == git::StageState::Staged {
                            git::unstage_file(&repo_path, &file)
                        } else {
                            git::stage_file(&repo_path, &file)
                        };
                        staging.run_git_op(&repo_path, result);
                    }
                }
                KeyCode::Char('a') => {
                    let result = git::stage_all(&repo_path);
                    staging.run_git_op(&repo_path, result);
                }
                KeyCode::Char('u') => {
                    let result = git::unstage_all(&repo_path);
                    staging.run_git_op(&repo_path, result);
                }
                KeyCode::Char('r') => {
                    staging.status_msg = None;
                    staging.reload(&repo_path);
                }
                KeyCode::Char('c') => staging.focus = StagingFocus::CommitMessage,
                KeyCode::Tab => staging.focus = StagingFocus::Diff,
                KeyCode::BackTab => staging.focus = StagingFocus::CommitMessage,
                _ => {}
            },
            StagingFocus::Diff => match key.code {
                KeyCode::Char('q') => return true,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
                KeyCode::Enter => {
                    if staging.has_staged() {
                        self.enter_commit_message_editor();
                    }
                }
                KeyCode::Esc => staging.focus = StagingFocus::FileList,
                KeyCode::Tab => staging.focus = StagingFocus::CommitMessage,
                KeyCode::BackTab => staging.focus = StagingFocus::FileList,
                KeyCode::Down | KeyCode::Char('j') | KeyCode::PageDown | KeyCode::Char('p') => {
                    staging.diff_scroll = staging.diff_scroll.saturating_add(10);
                }
                KeyCode::Up | KeyCode::Char('k') | KeyCode::PageUp | KeyCode::Char('P') => {
                    staging.diff_scroll = staging.diff_scroll.saturating_sub(10);
                }
                _ => {}
            },
            StagingFocus::CommitMessage => match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
                KeyCode::Esc => staging.focus = StagingFocus::FileList,
                KeyCode::Tab => staging.focus = StagingFocus::FileList,
                KeyCode::BackTab => staging.focus = StagingFocus::Diff,
                KeyCode::Char(c)
                    if key.modifiers == KeyModifiers::NONE
                        || key.modifiers == KeyModifiers::SHIFT =>
                {
                    staging.msg_insert(c);
                }
                KeyCode::Backspace => staging.msg_delete_before(),
                KeyCode::Delete => staging.msg_delete_at(),
                KeyCode::Left => staging.msg_move_left(),
                KeyCode::Right => staging.msg_move_right(),
                KeyCode::Home => staging.msg_cursor = 0,
                KeyCode::End => staging.msg_cursor_end(),
                _ => {}
            },
        }
        false
    }

    /// Validate the message/staged state, then either commit directly (`-y`)
    /// or push a confirmation popup.
    pub(crate) fn request_commit(&mut self, msg: String, has_staged: bool) -> bool {
        if msg.is_empty() {
            if let Some(s) = self.staging.as_mut() {
                s.status_msg = Some("Commit message is empty.".into());
            }
            return false;
        }
        if !has_staged {
            if let Some(s) = self.staging.as_mut() {
                s.status_msg = Some("Nothing staged to commit.".into());
            }
            return false;
        }
        if self.yes {
            return self.execute_commit(msg);
        }
        self.nav
            .push(Mode::Confirm(ConfirmAction::Commit { msg }, true));
        false
    }

    pub(crate) fn execute_commit(&mut self, msg: String) -> bool {
        match git::commit_staged(&self.repo_path, &msg) {
            Ok(()) => {
                if let Ok(commits) = git::load_commits(&self.repo_path, self.commit_sort) {
                    self.replace_commits(commits);
                }
                self.staging = None;
                self.nav.reset(Mode::Normal);
                self.status_message = Some("Committed successfully.".into());
                self.event_bus.emit(AppEvent::Action {
                    kind: ActionKind::CommitCreated,
                    screen: self.current_screen(),
                    at: EventTime::now(),
                });
            }
            Err(e) => {
                // Stay on the staging screen and show the error there.
                if let Some(s) = self.staging.as_mut() {
                    s.status_msg = Some(format!("Commit error: {e}"));
                }
            }
        }
        false
    }
}

// ── App impl: commit message editor (Mode::EditCommitMessage) ─────────────────

impl App {
    pub(crate) fn enter_commit_message_editor(&mut self) {
        let current = self
            .staging
            .as_ref()
            .map(|s| s.commit_msg.clone())
            .unwrap_or_default();
        let mut editor = MultilineEditor::new(&current);
        editor.insert_mode = true;
        editor.cursor_col = editor.line_len();
        self.commit_msg_editor = Some(editor);
        self.editor_save_selected = true;
        self.nav.push(Mode::EditCommitMessage);
    }

    pub(crate) fn commit_message_editor_has_changes(&self) -> bool {
        let editor_text = self
            .commit_msg_editor
            .as_ref()
            .map(|editor| editor.text())
            .unwrap_or_default();
        let current = self
            .staging
            .as_ref()
            .map(|s| s.commit_msg.clone())
            .unwrap_or_default();
        editor_text.trim() != current.trim()
    }

    pub(crate) fn handle_commit_message_editor(&mut self, key: KeyEvent) -> bool {
        let Some(editor) = self.commit_msg_editor.as_mut() else {
            self.nav.pop();
            return false;
        };
        match handle_editor_key(editor, key) {
            EditorKeyOutcome::Handled => {}
            EditorKeyOutcome::Quit => return true,
            EditorKeyOutcome::Escape => {
                if self.commit_message_editor_has_changes() {
                    self.nav.push(Mode::Confirm(
                        ConfirmAction::DiscardStagingMessageEditor,
                        false,
                    ));
                } else {
                    self.discard_commit_message_editor();
                }
            }
            EditorKeyOutcome::Submit => return self.activate_multiline_editor_action(),
            EditorKeyOutcome::SwitchButton => {
                self.editor_save_selected = !self.editor_save_selected;
            }
        }
        false
    }

    fn apply_commit_editor_text(&mut self, text: &str) {
        if let Some(s) = self.staging.as_mut() {
            // Show the first line in the single-line staging input; the full
            // text is used for the actual commit.
            s.commit_msg = text.lines().next().unwrap_or("").to_string();
            s.msg_cursor = s.commit_msg.chars().count();
        }
    }

    pub(crate) fn commit_editor_confirm(&mut self) -> bool {
        let text = match self.commit_msg_editor.take() {
            Some(e) => e.text(),
            None => return false,
        };
        // The editor is closed either way; return to the staging screen first
        // so a possible Confirm popup opens on top of it.
        self.nav.pop();
        if text.is_empty() {
            if let Some(s) = self.staging.as_mut() {
                s.status_msg = Some("Commit message is empty.".into());
            }
            return false;
        }
        self.apply_commit_editor_text(&text);
        let has_staged = self
            .staging
            .as_ref()
            .map(|s| s.has_staged())
            .unwrap_or(false);
        self.request_commit(text, has_staged)
    }

    pub(crate) fn discard_commit_message_editor(&mut self) {
        self.commit_msg_editor = None;
        self.nav.pop();
    }

    pub(crate) fn paste_staging_text(&mut self, text: &str) {
        if let Some(staging) = self.staging.as_mut()
            && staging.focus == StagingFocus::CommitMessage
        {
            if text.contains('\n') || text.contains('\r') {
                // Multi-line pastes get the full editor.
                self.enter_commit_message_editor();
                self.paste_commit_message_editor(text);
            } else {
                staging.msg_insert_text(text);
            }
        }
    }

    pub(crate) fn paste_commit_message_editor(&mut self, text: &str) {
        if let Some(editor) = self.commit_msg_editor.as_mut() {
            editor.insert_mode = true;
            editor.insert_text(text);
        }
    }
}
