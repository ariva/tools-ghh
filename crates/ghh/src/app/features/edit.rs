//! Commit-edit popup: edit message, author name/email and date of a commit.
//!
//! Two modes are involved: `Mode::Edit` (the field form) and
//! `Mode::EditCommitField` (the multiline message editor opened from it).

use crate::app::App;
use crate::app::editor::MultilineEditor;
use crate::app::editor::{EditorKeyOutcome, handle_editor_key};
use crate::app::models::{ConfirmAction, EditField, Mode, VisibleRow};
use crate::events::EventTime;
use crate::events::{ActionKind, AppEvent};
use crate::git;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Form state for the commit-edit popup (single-line fields + message preview).
pub struct EditState {
    pub field: EditField,
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    pub date: String,
    /// Commit's tz offset; the date field is entered/shown in this zone.
    pub offset_minutes: i32,
    pub cursor: usize,
}

impl EditState {
    pub fn from_commit(commit: &git::CommitInfo) -> Self {
        let message = commit
            .full_message
            .clone()
            .unwrap_or_else(|| commit.message.clone());
        let cursor = message.chars().count();
        Self {
            field: EditField::Message,
            message,
            author_name: commit.author_name.clone(),
            author_email: commit.author_email.clone(),
            date: commit.date_edit_text().into_owned(),
            offset_minutes: commit.offset_minutes,
            cursor,
        }
    }

    fn current_value(&self) -> &str {
        match self.field {
            EditField::Message => &self.message,
            EditField::AuthorName => &self.author_name,
            EditField::AuthorEmail => &self.author_email,
            EditField::Date => &self.date,
        }
    }

    fn current_value_mut(&mut self) -> &mut String {
        match self.field {
            EditField::Message => &mut self.message,
            EditField::AuthorName => &mut self.author_name,
            EditField::AuthorEmail => &mut self.author_email,
            EditField::Date => &mut self.date,
        }
    }

    pub fn set_cursor_end(&mut self) {
        self.cursor = self.current_value().chars().count();
    }

    pub fn insert_char(&mut self, c: char) {
        let cursor = self.cursor;
        let val = self.current_value_mut();
        let byte_pos = val
            .char_indices()
            .nth(cursor)
            .map(|(i, _)| i)
            .unwrap_or(val.len());
        val.insert(byte_pos, c);
        self.cursor += 1;
    }

    pub fn insert_text(&mut self, text: &str) {
        for ch in text.chars() {
            match ch {
                '\r' => {}
                '\n' => self.insert_char(' '),
                c => self.insert_char(c),
            }
        }
    }

    pub fn delete_before(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let cursor = self.cursor;
        let val = self.current_value_mut();
        let byte_pos = val
            .char_indices()
            .nth(cursor - 1)
            .map(|(i, _)| i)
            .unwrap_or(0);
        val.remove(byte_pos);
        self.cursor -= 1;
    }

    pub fn delete_at(&mut self) {
        let cursor = self.cursor;
        let val = self.current_value_mut();
        let char_count = val.chars().count();
        if cursor >= char_count {
            return;
        }
        let byte_pos = val
            .char_indices()
            .nth(cursor)
            .map(|(i, _)| i)
            .unwrap_or(val.len());
        val.remove(byte_pos);
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn move_right(&mut self) {
        let len = self.current_value().chars().count();
        if self.cursor < len {
            self.cursor += 1;
        }
    }
}

// ── App impl: edit mode ───────────────────────────────────────────────────────

impl App {
    fn edit_has_changes(&self) -> bool {
        let (Some(edit), Some(commit)) = (self.edit.as_ref(), self.selected_commit()) else {
            return false;
        };
        edit.message
            != commit
                .full_message
                .clone()
                .unwrap_or_else(|| commit.message.clone())
            || edit.author_name != commit.author_name
            || edit.author_email != commit.author_email
            || edit.date != commit.date_edit_text().as_ref()
    }

    pub(crate) fn enter_edit(&mut self) {
        let Some(visible_idx) = self.list_state.selected() else {
            return;
        };
        let Some(VisibleRow::Commit { idx, .. }) = self.visible.get(visible_idx) else {
            return;
        };
        let commit_idx = *idx;
        // Full message and author email are loaded lazily; fetch them now so
        // the form starts from complete data.
        if self.commits[commit_idx].full_message.is_none() {
            let hash = self.commits[commit_idx].hash.clone();
            match git::get_commit_full_message(&self.repo_path, &hash) {
                Ok(message) => self.commits[commit_idx].full_message = Some(message),
                Err(e) => {
                    self.status_message = Some(format!("Error loading commit message: {e}"));
                    return;
                }
            }
        }
        if self.commits[commit_idx].author_email.is_empty() {
            let hash = self.commits[commit_idx].hash.clone();
            match git::get_commit_author_email(&self.repo_path, &hash) {
                Ok(author_email) => self.commits[commit_idx].author_email = author_email,
                Err(e) => {
                    self.status_message = Some(format!("Error loading commit author email: {e}"));
                    return;
                }
            }
        }
        self.edit = Some(EditState::from_commit(&self.commits[commit_idx]));
        self.nav.push(Mode::Edit);
        self.status_message = None;
    }

    pub(crate) fn cancel_edit(&mut self) {
        self.edit = None;
        self.nav.pop();
    }

    pub(crate) fn save_edit(&mut self) {
        let valid = self
            .list_state
            .selected()
            .and_then(|i| self.visible.get(i))
            .map(|r| matches!(r, VisibleRow::Commit { .. }))
            .unwrap_or(false)
            && self.edit.is_some();
        if !valid {
            self.cancel_edit();
            return;
        }
        if !self.edit_has_changes() {
            self.status_message = Some("No changes to save.".into());
            return;
        }
        if self.yes {
            self.execute_save_edit();
        } else {
            self.nav.push(Mode::Confirm(ConfirmAction::SaveEdit, true));
        }
    }

    /// Rewrite the commit, reload history, and return to the commit list.
    pub(crate) fn execute_save_edit(&mut self) {
        let commit_idx = self
            .list_state
            .selected()
            .and_then(|i| self.visible.get(i))
            .and_then(|row| match row {
                VisibleRow::Commit { idx, .. } => Some(*idx),
                _ => None,
            });
        let (Some(commit_idx), Some(edit)) = (commit_idx, self.edit.take()) else {
            self.nav.reset(Mode::Normal);
            return;
        };

        let commit_hash = self.commits[commit_idx].hash.clone();
        let orig_date = self.commits[commit_idx].date_edit_text().into_owned();
        let new_date = (edit.date != orig_date).then(|| edit.date.clone());

        match git::modify_commit(
            &self.repo_path,
            &commit_hash,
            Some(&edit.message),
            Some(&edit.author_name),
            Some(&edit.author_email),
            new_date.as_deref(),
        ) {
            Ok(()) => match git::load_commits(&self.repo_path, self.commit_sort) {
                Ok(commits) => {
                    self.replace_commits(commits);
                    // An edit keeps the order of the list: same commit
                    // index. Not the same row: the reload expands collapsed
                    // merges and leaves a drill view.
                    self.select_commit_idx(commit_idx.min(self.commits.len().saturating_sub(1)));
                    self.status_message = Some("Commit modified successfully.".into());
                    self.event_bus.emit(AppEvent::Action {
                        kind: ActionKind::CommitModified,
                        screen: self.current_screen(),
                        at: EventTime::now(),
                    });
                }
                Err(e) => {
                    self.status_message = Some(format!("Modified but reload failed: {e}"));
                }
            },
            Err(e) => {
                self.status_message = Some(format!("Error: {e}"));
            }
        }
        self.nav.reset(Mode::Normal);
    }

    pub(crate) fn handle_edit(&mut self, key: KeyEvent) -> bool {
        let Some(edit) = self.edit.as_mut() else {
            self.nav.reset(Mode::Normal);
            return false;
        };
        let on_message = edit.field == EditField::Message;

        match key.code {
            KeyCode::Esc => self.cancel_edit(),
            // The message field is a read-only preview; Enter opens the
            // multiline editor. Other fields save on Enter.
            KeyCode::Enter => {
                if on_message {
                    self.enter_commit_field_editor();
                } else {
                    self.save_edit();
                }
            }
            KeyCode::Up | KeyCode::BackTab => {
                edit.field = edit.field.prev();
                edit.set_cursor_end();
            }
            KeyCode::Down | KeyCode::Tab => {
                edit.field = edit.field.next();
                edit.set_cursor_end();
            }
            // Single-line editing applies to every field except the preview.
            KeyCode::Char(c)
                if !on_message
                    && (key.modifiers == KeyModifiers::NONE
                        || key.modifiers == KeyModifiers::SHIFT) =>
            {
                edit.insert_char(c);
            }
            KeyCode::Backspace if !on_message => edit.delete_before(),
            KeyCode::Delete if !on_message => edit.delete_at(),
            KeyCode::Left if !on_message => edit.move_left(),
            KeyCode::Right if !on_message => edit.move_right(),
            KeyCode::Home if !on_message => edit.cursor = 0,
            KeyCode::End if !on_message => edit.set_cursor_end(),
            _ => {}
        }
        false
    }

    pub(crate) fn paste_edit_text(&mut self, text: &str) {
        if let Some(edit) = self.edit.as_mut()
            && edit.field != EditField::Message
        {
            edit.insert_text(text);
        }
    }

    pub(crate) fn enter_commit_field_editor(&mut self) {
        let msg = self
            .edit
            .as_ref()
            .map(|e| e.message.clone())
            .unwrap_or_default();
        let mut editor = MultilineEditor::new(&msg);
        editor.insert_mode = true;
        self.commit_msg_editor = Some(editor);
        self.editor_save_selected = true;
        self.nav.push(Mode::EditCommitField);
    }

    pub(crate) fn commit_field_editor_has_changes(&self) -> bool {
        let editor_text = self
            .commit_msg_editor
            .as_ref()
            .map(|editor| editor.text())
            .unwrap_or_default();
        let current = self
            .edit
            .as_ref()
            .map(|e| e.message.clone())
            .unwrap_or_default();
        editor_text.trim() != current.trim()
    }

    /// Message editor inside the commit-edit popup. Shared vim bindings live
    /// in [`editor_keys`]; only the exit decisions are handled here.
    pub(crate) fn handle_commit_field_editor(&mut self, key: KeyEvent) -> bool {
        let Some(editor) = self.commit_msg_editor.as_mut() else {
            self.nav.pop();
            return false;
        };
        match handle_editor_key(editor, key) {
            EditorKeyOutcome::Handled => {}
            EditorKeyOutcome::Quit => return true,
            EditorKeyOutcome::Escape => {
                if self.commit_field_editor_has_changes() {
                    self.nav.push(Mode::Confirm(
                        ConfirmAction::DiscardEditMessageEditor,
                        false,
                    ));
                } else {
                    self.discard_commit_field_editor();
                }
            }
            EditorKeyOutcome::Submit => {
                self.activate_multiline_editor_action();
            }
            EditorKeyOutcome::SwitchButton => {
                self.editor_save_selected = !self.editor_save_selected;
            }
        }
        false
    }

    pub(crate) fn confirm_commit_field_editor(&mut self) {
        let text = self
            .commit_msg_editor
            .as_ref()
            .map(|editor| editor.text())
            .unwrap_or_default();
        let current = self
            .edit
            .as_ref()
            .map(|e| e.message.clone())
            .unwrap_or_default();
        if text == current {
            // Nothing changed — just close the editor.
            self.commit_msg_editor = None;
            self.nav.pop();
            return;
        }
        if self.yes {
            self.save_commit_field_editor_confirmed();
        } else {
            self.nav.push(Mode::Confirm(
                ConfirmAction::SaveEditFromMessageEditor,
                true,
            ));
        }
    }

    pub(crate) fn save_commit_field_editor_confirmed(&mut self) {
        if let Some(editor) = self.commit_msg_editor.take() {
            let text = editor.text();
            if let Some(e) = self.edit.as_mut() {
                e.message = text;
                e.set_cursor_end();
            }
        }
        self.execute_save_edit();
    }

    pub(crate) fn discard_commit_field_editor(&mut self) {
        self.commit_msg_editor = None;
        self.nav.pop();
    }

    pub(crate) fn paste_commit_field_editor(&mut self, text: &str) {
        if let Some(editor) = self.commit_msg_editor.as_mut() {
            editor.insert_mode = true;
            editor.insert_text(text);
        }
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

    fn commit(dir: &Path, file: &str, message: &str) {
        std::fs::write(dir.join(file), format!("{message}\n")).unwrap();
        git(dir, &["add", file]);
        git(dir, &["commit", "-qm", message]);
    }

    /// The edited commit stays selected when the reload expands a merge
    /// that was collapsed above it: its row moves, the commit does not.
    #[test]
    fn selection_stays_on_the_edited_commit() {
        let dir = std::env::temp_dir().join(format!("ghh-edit-select-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.name", "t"]);
        git(&dir, &["config", "user.email", "t@t"]);
        git(&dir, &["config", "commit.gpgsign", "false"]);
        commit(&dir, "a.txt", "root");
        git(&dir, &["checkout", "-q", "-b", "topic"]);
        commit(&dir, "t.txt", "topic 1");
        commit(&dir, "t.txt", "topic 2");
        git(&dir, &["checkout", "-q", "main"]);
        commit(&dir, "b.txt", "main 2");
        git(
            &dir,
            &["merge", "-q", "--no-ff", "-m", "merge topic", "topic"],
        );

        let mut app = App::new(dir.clone(), true);
        app.initial_history_loader_rx = None;
        app.history_loading = false;
        let commits = git::load_commits(&dir, app.commit_sort).unwrap();
        app.replace_commits(commits);
        app.list_state.select(Some(0));
        assert!(app.commits[0].is_merge);
        app.toggle_collapse();

        let root_idx = app.commits.len() - 1;
        app.select_commit_idx(root_idx);
        let row_while_collapsed = app.list_state.selected().unwrap();
        app.enter_edit();
        app.edit.as_mut().unwrap().message = "root, edited".into();
        app.execute_save_edit();

        assert_eq!(
            app.selected_commit().map(|c| c.message.as_str()),
            Some("root, edited"),
            "{:?}",
            app.status_message
        );
        assert_ne!(
            app.list_state.selected(),
            Some(row_while_collapsed),
            "the reload expanded the merge: a kept row number is another commit"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
