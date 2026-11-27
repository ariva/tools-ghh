//! Shared key handling for every vim-like [`MultilineEditor`] popup.
//!
//! The squash-message, staging-commit-message and commit-field editors all
//! use identical vim-style bindings. This module implements them once; each
//! mode only decides what to do with the few keys that leave the editor
//! (submit / escape / quit / button focus), reported as [`EditorKeyOutcome`].
//!
//! Editors open in insert mode. There Enter adds a line and Ctrl+Shift+Enter
//! (or Ctrl+S, for terminals that cannot tell Ctrl+Shift+Enter from Enter)
//! executes the selected button; Esc goes to the view-only command mode,
//! where Enter executes the button.

use crate::app::editor::MultilineEditor;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a key press means beyond pure text editing. `Handled` covers every
/// key the editor consumed itself (cursor moves, typing, `dd`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorKeyOutcome {
    /// The editor consumed the key; nothing else to do.
    Handled,
    /// Ctrl+C — the app should quit.
    Quit,
    /// Esc in command mode — the caller decides between discarding and
    /// asking for confirmation when there are unsaved changes.
    Escape,
    /// Execute the selected button: Ctrl+Shift+Enter or Ctrl+S in insert mode,
    /// Enter or Ctrl+S in command mode.
    Submit,
    /// Tab/BackTab in either mode — the caller moves the Save/Cancel focus.
    SwitchButton,
}

/// Apply one key press to `editor`, using the shared vim-like binding set.
pub fn handle_editor_key(editor: &mut MultilineEditor, key: KeyEvent) -> EditorKeyOutcome {
    if editor.insert_mode {
        handle_insert_mode(editor, key)
    } else {
        handle_command_mode(editor, key)
    }
}

fn handle_insert_mode(editor: &mut MultilineEditor, key: KeyEvent) -> EditorKeyOutcome {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => editor.exit_insert_mode(),
        // Seen as Ctrl+Shift+Enter only where the terminal reports it (kitty
        // keyboard protocol, turned on in main.rs); elsewhere it arrives as
        // a plain Enter and adds a line, and Ctrl+S is the way to submit.
        KeyCode::Enter
            if key
                .modifiers
                .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT) =>
        {
            return EditorKeyOutcome::Submit;
        }
        KeyCode::Char('s') if ctrl => return EditorKeyOutcome::Submit,
        KeyCode::Tab | KeyCode::BackTab => return EditorKeyOutcome::SwitchButton,
        KeyCode::Char(c)
            if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT =>
        {
            editor.insert_char(c);
        }
        KeyCode::Backspace => editor.backspace(),
        KeyCode::Delete => editor.delete_at(),
        KeyCode::Enter => editor.split_line(),
        KeyCode::Left => editor.move_left(),
        KeyCode::Right => editor.move_right(),
        KeyCode::Up => editor.move_up(),
        KeyCode::Down => editor.move_down(),
        KeyCode::Home if ctrl => editor.move_doc_start(),
        KeyCode::End if ctrl => editor.move_doc_end(),
        KeyCode::Home => editor.move_line_start(),
        KeyCode::End => editor.move_line_end(),
        _ => {}
    }
    EditorKeyOutcome::Handled
}

fn handle_command_mode(editor: &mut MultilineEditor, key: KeyEvent) -> EditorKeyOutcome {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    // Any key other than the second `d` of a `dd` cancels the pending delete;
    // clear the flag up front and re-set it in the `d` arm below.
    let pending_d = editor.pending_d;
    editor.pending_d = false;

    match key.code {
        KeyCode::Char('c') if ctrl => return EditorKeyOutcome::Quit,
        KeyCode::Char('s') if ctrl => return EditorKeyOutcome::Submit,
        KeyCode::Enter => return EditorKeyOutcome::Submit,
        KeyCode::Esc => return EditorKeyOutcome::Escape,
        KeyCode::Tab | KeyCode::BackTab => return EditorKeyOutcome::SwitchButton,
        KeyCode::Char('j') | KeyCode::Down => editor.move_down(),
        KeyCode::Char('k') | KeyCode::Up => editor.move_up(),
        KeyCode::Char('h') | KeyCode::Left => editor.move_left(),
        KeyCode::Char('l') | KeyCode::Right => {
            // In command mode the cursor stops on the last character, not after it.
            let max = editor.line_len().saturating_sub(1);
            if editor.cursor_col < max {
                editor.cursor_col += 1;
            }
        }
        KeyCode::Char('i') => editor.insert_mode = true,
        KeyCode::Char('a') => {
            editor.cursor_col = (editor.cursor_col + 1).min(editor.line_len());
            editor.insert_mode = true;
        }
        KeyCode::Char('A') => {
            editor.cursor_col = editor.line_len();
            editor.insert_mode = true;
        }
        KeyCode::Char('o') => editor.open_line_below(),
        KeyCode::Char('O') => editor.open_line_above(),
        KeyCode::Char('d') => {
            if pending_d {
                editor.delete_line();
            } else {
                editor.pending_d = true;
            }
        }
        KeyCode::Char('x') | KeyCode::Delete => editor.delete_at(),
        KeyCode::Char('0') | KeyCode::Home => {
            if ctrl {
                editor.move_doc_start();
            } else {
                editor.move_line_start();
            }
        }
        KeyCode::Char('$') | KeyCode::End => {
            if ctrl {
                editor.move_doc_end();
            } else {
                editor.cursor_col = editor.line_len().saturating_sub(1);
            }
        }
        _ => {}
    }
    EditorKeyOutcome::Handled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn typing_in_insert_mode() {
        let mut editor = MultilineEditor::new("");
        editor.insert_mode = true;
        for c in "hi".chars() {
            assert_eq!(
                handle_editor_key(&mut editor, key(KeyCode::Char(c))),
                EditorKeyOutcome::Handled
            );
        }
        assert_eq!(editor.text(), "hi");
    }

    #[test]
    fn esc_leaves_insert_mode_then_reports_escape() {
        let mut editor = MultilineEditor::new("abc");
        editor.insert_mode = true;
        assert_eq!(
            handle_editor_key(&mut editor, key(KeyCode::Esc)),
            EditorKeyOutcome::Handled
        );
        assert!(!editor.insert_mode);
        assert_eq!(
            handle_editor_key(&mut editor, key(KeyCode::Esc)),
            EditorKeyOutcome::Escape
        );
    }

    #[test]
    fn dd_deletes_line_single_d_does_not() {
        let mut editor = MultilineEditor::new("one\ntwo");
        handle_editor_key(&mut editor, key(KeyCode::Char('d')));
        assert_eq!(editor.lines.len(), 2);
        handle_editor_key(&mut editor, key(KeyCode::Char('d')));
        assert_eq!(editor.lines, vec!["two".to_string()]);
    }

    #[test]
    fn pending_d_is_cancelled_by_other_keys() {
        let mut editor = MultilineEditor::new("one\ntwo");
        handle_editor_key(&mut editor, key(KeyCode::Char('d')));
        handle_editor_key(&mut editor, key(KeyCode::Char('j')));
        handle_editor_key(&mut editor, key(KeyCode::Char('d')));
        assert_eq!(editor.lines.len(), 2, "j must cancel the pending d");
    }

    #[test]
    fn command_mode_exit_keys() {
        let mut editor = MultilineEditor::new("x");
        assert_eq!(
            handle_editor_key(&mut editor, key(KeyCode::Enter)),
            EditorKeyOutcome::Submit
        );
        assert_eq!(
            handle_editor_key(&mut editor, ctrl('s')),
            EditorKeyOutcome::Submit
        );
        assert_eq!(
            handle_editor_key(&mut editor, ctrl('c')),
            EditorKeyOutcome::Quit
        );
        assert_eq!(
            handle_editor_key(&mut editor, key(KeyCode::Tab)),
            EditorKeyOutcome::SwitchButton
        );
    }

    #[test]
    fn insert_mode_enter_adds_a_line_and_ctrl_shift_enter_submits() {
        let mut editor = MultilineEditor::new("ab");
        editor.insert_mode = true;
        assert_eq!(
            handle_editor_key(&mut editor, key(KeyCode::Enter)),
            EditorKeyOutcome::Handled
        );
        assert_eq!(editor.lines.len(), 2, "Enter adds a line");
        // Shift alone is not the execute key: it adds a line like Enter.
        assert_eq!(
            handle_editor_key(
                &mut editor,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)
            ),
            EditorKeyOutcome::Handled
        );
        assert_eq!(editor.lines.len(), 3);
        assert_eq!(
            handle_editor_key(
                &mut editor,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL | KeyModifiers::SHIFT)
            ),
            EditorKeyOutcome::Submit
        );
        assert_eq!(
            handle_editor_key(&mut editor, ctrl('s')),
            EditorKeyOutcome::Submit
        );
        assert_eq!(editor.lines.len(), 3, "submitting adds nothing");
        assert!(editor.insert_mode);
    }

    #[test]
    fn insert_mode_tab_switches_the_button() {
        let mut editor = MultilineEditor::new("ab");
        editor.insert_mode = true;
        for code in [KeyCode::Tab, KeyCode::BackTab] {
            assert_eq!(
                handle_editor_key(&mut editor, key(code)),
                EditorKeyOutcome::SwitchButton
            );
        }
        assert_eq!(editor.text(), "ab", "Tab types nothing");
    }

    #[test]
    fn open_line_below_enters_insert_mode() {
        let mut editor = MultilineEditor::new("top");
        handle_editor_key(&mut editor, key(KeyCode::Char('o')));
        assert!(editor.insert_mode);
        assert_eq!(editor.lines, vec!["top".to_string(), String::new()]);
        assert_eq!(editor.cursor_line, 1);
    }
}
