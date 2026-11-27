//! Interactive squash: pick a base commit plus commits to absorb into it
//! (`Mode::Squash`), then edit the combined message (`Mode::SquashMessage`).

use crate::app::App;
use crate::app::editor::MultilineEditor;
use crate::app::editor::{EditorKeyOutcome, handle_editor_key};
use crate::app::models::{ConfirmAction, Mode, SquashState, VisibleRow};
use crate::events::EventTime;
use crate::events::{ActionKind, AppEvent};
use crate::git;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashSet;

/// Squash-specific state: wraps the generic editor and carries squash metadata.
pub struct SquashMessageState {
    pub editor: MultilineEditor,
    pub base_hash: String,
    pub squash_hashes: Vec<String>,
    pub original_message: String,
}

impl SquashMessageState {
    pub fn new(base_hash: String, squash_hashes: Vec<String>, initial_lines: Vec<String>) -> Self {
        let original_message = initial_lines
            .iter()
            .filter(|line| !line.starts_with('#'))
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();
        Self {
            // Opens in view mode, unlike the other message editors: the
            // combined message is usually right as it is, so Enter saves it.
            editor: MultilineEditor::from_lines(initial_lines),
            base_hash,
            squash_hashes,
            original_message,
        }
    }

    /// Return the combined message, stripping `#`-comment lines.
    pub fn combined_message(&self) -> String {
        self.editor
            .lines
            .iter()
            .filter(|l| !l.starts_with('#'))
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    }
}

// ── App impl: squash mode ─────────────────────────────────────────────────────

impl App {
    pub(crate) fn enter_squash(&mut self) {
        let base_idx = match self.list_state.selected().and_then(|i| self.visible.get(i)) {
            Some(VisibleRow::Commit { idx, .. }) => *idx,
            _ => return,
        };
        self.squash = Some(SquashState {
            base_idx,
            squash_set: HashSet::new(),
        });
        self.nav.push(Mode::Squash);
        self.status_message = Some(format!(
            "Base: {} — Space: toggle squash  Enter: confirm  Esc: cancel",
            &self.commits[base_idx].hash[..7]
        ));
    }

    fn squash_status(&self, base_idx: usize, selected: usize) -> String {
        format!(
            "Base: {}  │  {} commit{} selected for squash",
            &self.commits[base_idx].hash[..7],
            selected,
            if selected == 1 { "" } else { "s" }
        )
    }

    pub(crate) fn handle_squash(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
            KeyCode::Esc => {
                let base_idx = self.squash.as_ref().map(|sq| sq.base_idx);
                self.squash = None;
                self.nav.pop();
                self.status_message = None;
                if let Some(idx) = base_idx {
                    self.select_commit_idx(idx);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                self.schedule_selected_stats_load();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                self.schedule_selected_stats_load();
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.move_top();
                self.schedule_selected_stats_load();
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.move_bottom();
                self.schedule_selected_stats_load();
            }
            KeyCode::Char(' ') => self.toggle_squash_mark(),
            KeyCode::Enter => self.open_squash_message_editor(),
            _ => {}
        }
        false
    }

    /// Mark/unmark the selected commit for squashing into the base.
    fn toggle_squash_mark(&mut self) {
        let Some(commit_idx) = self
            .list_state
            .selected()
            .and_then(|i| self.visible.get(i))
            .and_then(|row| match row {
                VisibleRow::Commit { idx, .. } => Some(*idx),
                _ => None,
            })
        else {
            return;
        };
        let Some(sq) = self.squash.as_mut() else {
            return;
        };
        if commit_idx == sq.base_idx {
            self.status_message = Some("Cannot mark the base commit for squash.".into());
            return;
        }
        if !sq.squash_set.remove(&commit_idx) {
            sq.squash_set.insert(commit_idx);
        }
        let (base_idx, selected) = (sq.base_idx, sq.squash_set.len());
        self.status_message = Some(self.squash_status(base_idx, selected));
    }

    /// Continue from the picker to the combined-message editor.
    fn open_squash_message_editor(&mut self) {
        let Some(sq) = &self.squash else { return };
        if sq.squash_set.is_empty() {
            self.status_message = Some("Select commits to squash with Space first.".into());
            return;
        }
        let base_idx = sq.base_idx;
        let base_hash = self.commits[base_idx].hash.clone();
        let mut squash_idxs: Vec<usize> = sq.squash_set.iter().copied().collect();
        squash_idxs.sort_unstable();
        let squash_hashes: Vec<String> = squash_idxs
            .iter()
            .map(|&i| self.commits[i].hash.clone())
            .collect();
        let initial_lines = self.build_squash_message_lines(base_idx, &squash_hashes);
        self.squash_msg = Some(SquashMessageState::new(
            base_hash,
            squash_hashes,
            initial_lines,
        ));
        self.editor_save_selected = true;
        self.nav.push(Mode::SquashMessage);
    }

    /// Base message first, then each squashed commit's message under a
    /// `# --- <hash> ---` comment separator.
    pub(crate) fn build_squash_message_lines(
        &self,
        base_idx: usize,
        squash_hashes: &[String],
    ) -> Vec<String> {
        let mut lines = vec![self.commits[base_idx].message.clone()];
        for hash in squash_hashes {
            if let Some(c) = self.commit_by_hash(hash) {
                lines.push(format!("# --- {} ---", &c.hash[..7]));
                lines.push(c.message.clone());
            }
        }
        lines
    }

    pub(crate) fn handle_squash_message(&mut self, key: KeyEvent) -> bool {
        let Some(state) = self.squash_msg.as_mut() else {
            self.nav.reset(Mode::Normal);
            return false;
        };
        match handle_editor_key(&mut state.editor, key) {
            EditorKeyOutcome::Handled => {}
            EditorKeyOutcome::Quit => return true,
            EditorKeyOutcome::Escape => self.cancel_multiline_editor(),
            EditorKeyOutcome::Submit => {
                self.activate_multiline_editor_action();
            }
            EditorKeyOutcome::SwitchButton => {
                self.editor_save_selected = !self.editor_save_selected;
            }
        }
        false
    }

    pub(crate) fn squash_message_has_changes(&self) -> bool {
        self.squash_msg
            .as_ref()
            .map(|state| state.combined_message() != state.original_message)
            .unwrap_or(false)
    }

    pub(crate) fn discard_squash_message_editor(&mut self) {
        self.squash_msg = None;
        self.nav.pop();
        if let Some(sq) = &self.squash {
            let base_idx = sq.base_idx;
            self.select_commit_idx(base_idx);
        }
    }

    pub(crate) fn paste_squash_message_editor(&mut self, text: &str) {
        if let Some(state) = self.squash_msg.as_mut() {
            state.editor.insert_mode = true;
            state.editor.insert_text(text);
        }
    }

    pub(crate) fn squash_msg_confirm(&mut self) {
        let Some(s) = self.squash_msg.as_ref() else {
            return;
        };
        let msg = s.combined_message();
        if msg.is_empty() {
            self.status_message = Some("Message cannot be empty.".into());
            return;
        }
        let base_hash = s.base_hash.clone();
        let squash_hashes = s.squash_hashes.clone();
        self.squash_msg = None;
        // Close the editor first so the Confirm popup opens over the picker.
        self.nav.pop();
        if self.yes {
            self.execute_squash_with_message(base_hash, squash_hashes, msg);
        } else {
            self.nav.push(Mode::Confirm(
                ConfirmAction::Squash {
                    base_hash,
                    squash_hashes,
                    message: msg,
                },
                true,
            ));
        }
    }

    pub(crate) fn execute_squash_with_message(
        &mut self,
        base_hash: String,
        squash_hashes: Vec<String>,
        message: String,
    ) {
        let n = squash_hashes.len();
        match git::squash_commits(&self.repo_path, &base_hash, &squash_hashes, &message) {
            Ok(()) => match git::load_commits(&self.repo_path, self.commit_sort) {
                Ok(commits) => {
                    self.replace_commits(commits);
                    // The squashed commit sits where the newest commit of
                    // the range was: n commits above the oldest one.
                    let squashed_idx = self
                        .squash
                        .as_ref()
                        .map(|sq| {
                            let oldest = sq
                                .squash_set
                                .iter()
                                .copied()
                                .chain(std::iter::once(sq.base_idx))
                                .max()
                                .unwrap_or(sq.base_idx);
                            oldest.saturating_sub(n)
                        })
                        .unwrap_or(0);
                    // A commit index, not a row: the "Uncommitted changes"
                    // row and the reset collapse state shift the rows.
                    self.select_commit_idx(squashed_idx.min(self.commits.len().saturating_sub(1)));
                    self.status_message = Some(format!(
                        "Squashed {} commit{} successfully.",
                        n,
                        if n == 1 { "" } else { "s" }
                    ));
                    self.event_bus.emit(AppEvent::Action {
                        kind: ActionKind::CommitSquashed { count: n },
                        screen: self.current_screen(),
                        at: EventTime::now(),
                    });
                }
                Err(e) => {
                    self.status_message = Some(format!("Squashed but reload failed: {e}"));
                }
            },
            Err(e) => {
                self.status_message = Some(format!("Squash error: {e}"));
            }
        }
        self.squash = None;
        self.nav.reset(Mode::Normal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    }

    /// Squash started on line 1 ends on line 1, also with the "Uncommitted
    /// changes" row above it: the target is a commit, not a row number.
    #[test]
    fn selection_returns_to_the_squashed_commit() {
        let dir = std::env::temp_dir().join(format!("ghh-squash-select-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.name", "t"]);
        git(&dir, &["config", "user.email", "t@t"]);
        git(&dir, &["config", "commit.gpgsign", "false"]);
        for i in 1..=4 {
            std::fs::write(dir.join("a.txt"), format!("{i}\n")).unwrap();
            git(&dir, &["add", "a.txt"]);
            git(&dir, &["commit", "-qm", &format!("commit {i}")]);
        }
        // Untracked file: row 0 is shown, rows and commit indices differ.
        std::fs::write(dir.join("untracked.txt"), "x\n").unwrap();

        let mut app = App::new(dir.clone(), true);
        app.initial_history_loader_rx = None;
        app.history_loading = false;
        let commits = git::load_commits(&dir, app.commit_sort).unwrap();
        app.replace_commits(commits);
        app.set_working_tree(git::load_status(&dir).unwrap());
        assert!(app.working_tree_row_shown());

        // Line 1 (newest commit) is the base, line 2 is folded into it.
        app.select_commit_idx(0);
        assert_eq!(app.list_state.selected(), Some(1));
        app.enter_squash();
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        app.handle_squash(key(KeyCode::Down));
        app.handle_squash(key(KeyCode::Char(' ')));
        let base = app.commits[0].hash.clone();
        let folded = vec![app.commits[1].hash.clone()];
        app.execute_squash_with_message(base, folded, "squashed".into());

        assert_eq!(app.commits.len(), 3, "{:?}", app.status_message);
        assert_eq!(app.list_state.selected(), Some(1), "back on line 1");
        assert_eq!(
            app.selected_commit().map(|c| c.message.as_str()),
            Some("squashed")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The squash editor behind its popup, with nothing to squash for real.
    fn app_in_squash_editor(name: &str) -> App {
        let dir = std::env::temp_dir().join(format!("ghh-squash-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir, false);
        app.squash_msg = Some(SquashMessageState::new(
            "base".into(),
            vec!["folded".into()],
            vec!["message".into()],
        ));
        app.editor_save_selected = true;
        app.nav.push(Mode::Squash);
        app.nav.push(Mode::SquashMessage);
        app
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_shift_enter() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL | KeyModifiers::SHIFT)
    }

    /// Opens in view mode; `i` starts editing, where Enter adds a line and
    /// Ctrl+Shift+Enter executes Save, which asks before it rewrites history.
    #[test]
    fn squash_editor_enter_adds_a_line_and_ctrl_shift_enter_saves() {
        let mut app = app_in_squash_editor("save");
        fn editor(app: &App) -> &MultilineEditor {
            &app.squash_msg.as_ref().unwrap().editor
        }
        assert!(!editor(&app).insert_mode, "opens in view mode");
        app.handle_squash_message(key(KeyCode::Char('i')));
        assert!(editor(&app).insert_mode);
        app.handle_squash_message(key(KeyCode::Enter));
        assert_eq!(editor(&app).lines.len(), 2);
        assert!(matches!(app.mode(), Mode::SquashMessage));

        app.handle_squash_message(ctrl_shift_enter());
        assert!(
            matches!(
                app.mode(),
                Mode::Confirm(ConfirmAction::Squash { .. }, true)
            ),
            "Save goes to the confirm popup"
        );
    }

    /// Tab moves to Cancel; executing it closes an unchanged editor and
    /// asks first when the text was changed.
    #[test]
    fn squash_editor_tab_selects_cancel() {
        let mut app = app_in_squash_editor("cancel");
        app.handle_squash_message(key(KeyCode::Tab));
        assert!(!app.editor_save_selected);
        app.handle_squash_message(ctrl_shift_enter());
        assert!(app.squash_msg.is_none(), "unchanged: closed at once");
        assert!(matches!(app.mode(), Mode::Squash));

        let mut app = app_in_squash_editor("cancel-changed");
        app.handle_squash_message(key(KeyCode::Char('i')));
        app.handle_squash_message(key(KeyCode::Char('x')));
        app.handle_squash_message(key(KeyCode::Tab));
        app.handle_squash_message(ctrl_shift_enter());
        assert!(
            matches!(
                app.mode(),
                Mode::Confirm(ConfirmAction::DiscardSquashMessageEditor, false)
            ),
            "changed text: asks, default No"
        );
        assert!(app.squash_msg.is_some());
    }

    /// As opened, in view mode: Enter executes Save without editing.
    #[test]
    fn squash_editor_view_mode_enter_executes() {
        let mut app = app_in_squash_editor("view");
        app.handle_squash_message(key(KeyCode::Enter));
        assert!(matches!(
            app.mode(),
            Mode::Confirm(ConfirmAction::Squash { .. }, true)
        ));
    }
}
