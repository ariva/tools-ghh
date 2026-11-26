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
                // Staging a file moves it to another group of the list;
                // the selection goes with the file, not with the row.
                let was = self.selected_file().cloned();
                let same_file = was.and_then(|was| {
                    let at = |same_state: bool| {
                        files.iter().position(|file| {
                            file.path == was.path
                                && (file.stage_state == was.stage_state) == same_state
                        })
                    };
                    at(true).or_else(|| at(false))
                });
                self.files = files;
                let new_sel = if self.files.is_empty() {
                    None
                } else {
                    Some(same_file.unwrap_or(prev.unwrap_or(0).min(self.files.len() - 1)))
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

    /// Move the file selection by `delta` rows, stopping at the first and
    /// last file, and refresh the diff pane.
    pub fn select_by(&mut self, repo_path: &std::path::Path, delta: isize) {
        let Some(last) = self.files.len().checked_sub(1) else {
            return;
        };
        let from = self.list_state.selected();
        let to = from.unwrap_or(0).saturating_add_signed(delta).min(last);
        if from != Some(to) {
            self.list_state.select(Some(to));
            self.refresh_diff(repo_path);
        }
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
        // One page = the rows a pane shows; 10 when the size is unknown.
        let (files_page, diff_page) =
            self.staging_regions().map_or((10, 10), |(files, diff, _)| {
                (
                    crate::app::layout::pane_page_step(files) as isize,
                    crate::app::layout::pane_page_step(diff),
                )
            });
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
                KeyCode::Down | KeyCode::Char('j') => staging.select_by(&repo_path, 1),
                KeyCode::Up | KeyCode::Char('k') => staging.select_by(&repo_path, -1),
                // p / P page like PgDn / PgUp, as in the commit list.
                KeyCode::PageDown | KeyCode::Char('p') => staging.select_by(&repo_path, files_page),
                KeyCode::PageUp | KeyCode::Char('P') => staging.select_by(&repo_path, -files_page),
                KeyCode::Home | KeyCode::Char('g') => staging.select_by(&repo_path, isize::MIN),
                KeyCode::End | KeyCode::Char('G') => staging.select_by(&repo_path, isize::MAX),
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
                // Drawing stops the scroll at the end of the diff.
                KeyCode::Down | KeyCode::Char('j') => {
                    staging.diff_scroll = staging.diff_scroll.saturating_add(1);
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    staging.diff_scroll = staging.diff_scroll.saturating_sub(1);
                }
                KeyCode::PageDown | KeyCode::Char('p') => {
                    staging.diff_scroll = staging.diff_scroll.saturating_add(diff_page);
                }
                KeyCode::PageUp | KeyCode::Char('P') => {
                    staging.diff_scroll = staging.diff_scroll.saturating_sub(diff_page);
                }
                KeyCode::Home | KeyCode::Char('g') => staging.diff_scroll = 0,
                KeyCode::End | KeyCode::Char('G') => staging.diff_scroll = usize::MAX,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Staging a file moves it up into the staged group; the selection
    /// stays on that file, so Space again unstages it.
    #[test]
    fn selection_follows_a_file_into_its_new_group() {
        let dir = std::env::temp_dir().join(format!("ghh-staging-follow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git2::Repository::init(&dir).unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(dir.join(name), "x\n").unwrap();
        }
        let mut staging = StagingState::new(&dir).unwrap();
        staging.list_state.select(Some(2));
        let file = staging.selected_file().cloned().unwrap();
        assert_eq!(file.path, "c.txt");

        staging.run_git_op(&dir, git::stage_file(&dir, &file));
        let order: Vec<&str> = staging.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(order, ["c.txt", "a.txt", "b.txt"], "staged group first");
        let selected = staging.selected_file().cloned().unwrap();
        assert_eq!(selected.path, "c.txt");
        assert_eq!(selected.stage_state, git::StageState::Staged);

        staging.run_git_op(&dir, git::unstage_file(&dir, &selected));
        assert_eq!(staging.selected_file().unwrap().path, "c.txt");
        assert_eq!(staging.list_state.selected(), Some(2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PgDn/PgUp (and p/P, g/G) move the file selection by pages and stop
    /// at the ends; in the diff pane they scroll, j/k by one line.
    #[test]
    fn page_keys_work_in_the_file_list_and_the_diff() {
        use ratatui::{Terminal, backend::TestBackend};
        let dir = std::env::temp_dir().join(format!("ghh-staging-pages-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git2::Repository::init(&dir).unwrap();
        for i in 0..40 {
            let body: String = (0..300).map(|line| format!("line {line}\n")).collect();
            std::fs::write(dir.join(format!("f{i:02}.txt")), body).unwrap();
        }
        let mut app = App::new(dir.clone(), true);
        app.initial_history_loader_rx = None;
        app.staging = Some(StagingState::new(&dir).unwrap());
        app.nav.push(Mode::Staging);
        let press = |app: &mut App, code| {
            app.handle_staging(KeyEvent::new(code, KeyModifiers::NONE));
        };
        let row = |app: &App| app.staging.as_ref().unwrap().list_state.selected().unwrap();

        press(&mut app, KeyCode::PageDown);
        let page = row(&app);
        assert!(page > 1, "PgDn moves more than one row: {page}");
        press(&mut app, KeyCode::Char('p'));
        assert_eq!(row(&app), (page * 2).min(39));
        press(&mut app, KeyCode::PageUp);
        press(&mut app, KeyCode::Char('P'));
        press(&mut app, KeyCode::PageUp);
        assert_eq!(row(&app), 0, "stops at the first file");
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(row(&app), 39);
        press(&mut app, KeyCode::PageDown);
        assert_eq!(row(&app), 39, "stops at the last file");
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(row(&app), 0);

        // Diff pane: one line per j, a page per PgDn, and the end stop.
        press(&mut app, KeyCode::Tab);
        let scroll = |app: &App| app.staging.as_ref().unwrap().diff_scroll;
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(scroll(&app), 1);
        press(&mut app, KeyCode::PageDown);
        assert!(scroll(&app) > 2);
        press(&mut app, KeyCode::Char('G'));
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal
            .draw(|frame| crate::ui::render(frame, &mut app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let screen: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(screen.contains("+line 299") && screen.contains("+line 280"));
        assert!(scroll(&app) < 300, "stopped at the end: {}", scroll(&app));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
