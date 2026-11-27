//! Staging screen: stage/unstage files, review the diff, then Enter opens
//! the message editor and its Save creates the commit (`Mode::Staging` + `Mode::EditCommitMessage`).

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
}

/// Shown when Enter asks for a commit and no file is staged.
const NOTHING_STAGED: &str = "Nothing staged: Space stages the file, a stages all.";

pub struct StagingState {
    pub files: Vec<git::FileStatus>,
    pub list_state: ListState,
    pub diff_lines: Vec<String>,
    pub diff_scroll: usize,
    /// Message of a commit that was not confirmed; the editor starts with it.
    pub commit_msg: String,
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
            Ok(files) => self.set_files(files),
            Err(e) => {
                self.status_msg = Some(format!("Reload error: {e}"));
            }
        }
        self.refresh_diff(repo_path);
    }

    /// Replace the list. The selection goes with the file, not with the
    /// row: staging a file moves it to another group. A file that is gone
    /// leaves the selection on its row.
    fn set_files(&mut self, files: Vec<git::FileStatus>) {
        let prev = self.list_state.selected();
        let was = self.selected_file().cloned();
        let same_file = was.and_then(|was| {
            let at = |same_state: bool| {
                files.iter().position(|file| {
                    file.path == was.path && (file.stage_state == was.stage_state) == same_state
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

    /// A background reading (`repo_watch.rs`) found the working tree
    /// changed outside the app: take the new list and load the diff again.
    /// Nothing moves when nothing changed; the diff keeps its scroll while
    /// the same file stays selected.
    pub(crate) fn apply_outside_change(
        &mut self,
        repo_path: &std::path::Path,
        files: Vec<git::FileStatus>,
        content_changed: bool,
    ) {
        let files_changed = files != self.files;
        if !files_changed && !content_changed {
            return;
        }
        let was = self.selected_file().cloned();
        let scroll = self.diff_scroll;
        if files_changed {
            self.set_files(files);
        }
        self.refresh_diff(repo_path);
        if self.selected_file() == was.as_ref() {
            // Drawing stops the scroll at the end of a diff that got shorter.
            self.diff_scroll = scroll;
        }
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

    /// Select `file` (same path and state); the last row when there is
    /// none, which is where a file toggled on the last row leaves a gap.
    fn select_file(&mut self, repo_path: &std::path::Path, file: Option<&git::FileStatus>) {
        let Some(last) = self.files.len().checked_sub(1) else {
            return;
        };
        let row = file
            .and_then(|file| {
                self.files
                    .iter()
                    .position(|f| f.path == file.path && f.stage_state == file.stage_state)
            })
            .unwrap_or(last);
        if self.list_state.selected() != Some(row) {
            self.list_state.select(Some(row));
            self.refresh_diff(repo_path);
        }
    }

    /// Run a stage/unstage-style git op, surface its error, then reload.
    fn run_git_op(&mut self, repo_path: &std::path::Path, result: Result<()>) {
        self.status_msg = result.err().map(|e| format!("Error: {e}"));
        self.reload(repo_path);
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
        // These keys change or re-read the status themselves: a background
        // reading started before them must not land after them and put the
        // old list back under the cursor.
        if matches!(key.code, KeyCode::Char(' ' | 'a' | 'u' | 'r')) {
            self.repo_watch.discard_pending();
        }
        let Some(staging) = self.staging.as_mut() else {
            self.nav.reset(Mode::Normal);
            return false;
        };

        match staging.focus {
            StagingFocus::FileList => match key.code {
                KeyCode::Char('q') => return true,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
                KeyCode::Enter => {
                    if staging.has_staged() {
                        self.enter_commit_message_editor();
                    } else {
                        staging.status_msg = Some(NOTHING_STAGED.into());
                    }
                }
                KeyCode::Esc | KeyCode::Backspace => {
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
                        // The file moves to another group; the selection
                        // goes to the one that was under it, so Space
                        // again takes the next file.
                        let row = staging.list_state.selected().unwrap_or(0);
                        let below = staging.files.get(row + 1).cloned();
                        let result = if file.stage_state == git::StageState::Staged {
                            git::unstage_file(&repo_path, &file)
                        } else {
                            git::stage_file(&repo_path, &file)
                        };
                        let done = result.is_ok();
                        staging.run_git_op(&repo_path, result);
                        if done {
                            staging.select_file(&repo_path, below.as_ref());
                        }
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
                KeyCode::Tab | KeyCode::BackTab => staging.focus = StagingFocus::Diff,
                _ => {}
            },
            StagingFocus::Diff => match key.code {
                KeyCode::Char('q') => return true,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
                KeyCode::Enter => {
                    if staging.has_staged() {
                        self.enter_commit_message_editor();
                    } else {
                        staging.status_msg = Some(NOTHING_STAGED.into());
                    }
                }
                KeyCode::Esc | KeyCode::Backspace | KeyCode::Tab | KeyCode::BackTab => {
                    staging.focus = StagingFocus::FileList
                }
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
            // Kept for the next Enter, should the commit not be confirmed.
            s.commit_msg = text.to_string();
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

    /// Space toggles the file and moves to the one that was under it:
    /// three Spaces stage three files; on the last row it stays there.
    #[test]
    fn space_toggles_and_moves_to_the_next_file() {
        let dir = std::env::temp_dir().join(format!("ghh-staging-space-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git2::Repository::init(&dir).unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(dir.join(name), "x\n").unwrap();
        }
        let mut app = App::new(dir.clone(), true);
        app.initial_history_loader_rx = None;
        app.staging = Some(StagingState::new(&dir).unwrap());
        app.nav.push(Mode::Staging);
        let space = |app: &mut App| {
            app.handle_staging(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        };
        let selected = |app: &App| {
            let file = app.staging.as_ref().unwrap().selected_file().unwrap();
            (file.path.clone(), file.stage_state.clone())
        };
        let staged = |app: &App| {
            let files = &app.staging.as_ref().unwrap().files;
            files
                .iter()
                .filter(|f| f.stage_state == git::StageState::Staged)
                .count()
        };

        space(&mut app);
        assert_eq!(selected(&app), ("b.txt".into(), git::StageState::Untracked));
        space(&mut app);
        assert_eq!(selected(&app), ("c.txt".into(), git::StageState::Untracked));
        assert_eq!(staged(&app), 2);
        // Last row: c.txt is staged and moves up; the cursor stays last.
        space(&mut app);
        assert_eq!(staged(&app), 3);
        assert_eq!(selected(&app), ("c.txt".into(), git::StageState::Staged));

        // Unstaging from the top: a.txt drops under the staged ones, the
        // cursor goes to b.txt, which was under it.
        app.staging.as_mut().unwrap().list_state.select(Some(0));
        space(&mut app);
        assert_eq!(selected(&app), ("b.txt".into(), git::StageState::Staged));
        assert_eq!(staged(&app), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A reload after stage all / unstage all keeps the selection on the
    /// same file, wherever its group put it.
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

    /// Enter opens the message editor from the file list and the diff when
    /// something is staged; with nothing staged it says so. Tab moves
    /// between the two panes only.
    #[test]
    fn enter_opens_the_message_editor_when_something_is_staged() {
        let dir = std::env::temp_dir().join(format!("ghh-staging-editor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git2::Repository::init(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "x\n").unwrap();
        let mut app = App::new(dir.clone(), true);
        app.initial_history_loader_rx = None;
        app.staging = Some(StagingState::new(&dir).unwrap());
        app.nav.push(Mode::Staging);
        let press = |app: &mut App, code| {
            app.handle_staging(KeyEvent::new(code, KeyModifiers::NONE));
        };
        let focus = |app: &App| app.staging.as_ref().unwrap().focus;

        press(&mut app, KeyCode::Enter);
        assert!(*app.mode() == Mode::Staging, "nothing staged: no editor");
        assert_eq!(
            app.staging.as_ref().unwrap().status_msg.as_deref(),
            Some(NOTHING_STAGED)
        );

        press(&mut app, KeyCode::Char(' '));
        assert!(app.staging.as_ref().unwrap().has_staged());
        press(&mut app, KeyCode::Enter);
        assert!(*app.mode() == Mode::EditCommitMessage, "from the file list");
        app.discard_commit_message_editor();

        press(&mut app, KeyCode::Tab);
        assert!(focus(&app) == StagingFocus::Diff);
        press(&mut app, KeyCode::Enter);
        assert!(*app.mode() == Mode::EditCommitMessage, "from the diff");
        app.discard_commit_message_editor();
        press(&mut app, KeyCode::Tab);
        assert!(focus(&app) == StagingFocus::FileList, "two panes to cycle");

        // The pane and its keys are gone: `c` and letters do nothing.
        press(&mut app, KeyCode::Char('c'));
        press(&mut app, KeyCode::Char('x'));
        assert!(focus(&app) == StagingFocus::FileList);
        assert_eq!(app.staging.as_ref().unwrap().commit_msg, "");
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
        // Backspace is Esc: in the diff back to the file list, in the file
        // list out of the screen.
        press(&mut app, KeyCode::Backspace);
        assert!(app.staging.as_ref().unwrap().focus == StagingFocus::FileList);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('G'));
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal
            .draw(|frame| crate::ui::render(frame, &mut app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let screen: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(screen.contains("+line 299") && screen.contains("+line 280"));
        assert!(scroll(&app) < 300, "stopped at the end: {}", scroll(&app));

        press(&mut app, KeyCode::BackTab);
        press(&mut app, KeyCode::Backspace);
        assert!(app.staging.is_none() && *app.mode() == Mode::Normal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An outside change keeps the cursor on the same file, not on the same
    /// row, and the diff scroll while that file stays selected.
    #[test]
    fn outside_change_keeps_the_cursor_on_its_file() {
        let dir = std::env::temp_dir().join(format!("ghh-staging-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git2::Repository::init(&dir).unwrap();
        for name in ["b.txt", "c.txt"] {
            std::fs::write(dir.join(name), "x\n").unwrap();
        }
        let mut staging = StagingState::new(&dir).unwrap();
        staging.select_by(&dir, 1);
        staging.diff_scroll = 4;
        let path = |s: &StagingState| s.selected_file().map(|f| f.path.clone());
        assert_eq!(path(&staging).as_deref(), Some("c.txt"));

        // Nothing changed: nothing moves, the diff is not loaded again.
        let same = staging.files.clone();
        staging.diff_lines = vec!["kept".into()];
        staging.apply_outside_change(&dir, same, false);
        assert_eq!((staging.diff_lines.len(), staging.diff_scroll), (1, 4));

        // A file appears above the cursor: the cursor goes down with c.txt.
        std::fs::write(dir.join("a.txt"), "x\n").unwrap();
        let files = git::load_status(&dir).unwrap();
        staging.apply_outside_change(&dir, files, true);
        assert_eq!(staging.files.len(), 3);
        assert_eq!(path(&staging).as_deref(), Some("c.txt"));
        assert_eq!(staging.diff_scroll, 4, "same file: scroll kept");
        assert_ne!(
            staging.diff_lines,
            vec!["kept".to_string()],
            "diff reloaded"
        );

        // The selected file goes away: the cursor stays on a valid row and
        // the scroll starts over for the other file.
        std::fs::remove_file(dir.join("c.txt")).unwrap();
        let files = git::load_status(&dir).unwrap();
        staging.apply_outside_change(&dir, files, true);
        assert_eq!(path(&staging).as_deref(), Some("b.txt"));
        assert_eq!(staging.diff_scroll, 0);

        // Everything gone: empty list, no selection.
        staging.apply_outside_change(&dir, Vec::new(), true);
        assert!(staging.selected_file().is_none() && staging.diff_lines.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
