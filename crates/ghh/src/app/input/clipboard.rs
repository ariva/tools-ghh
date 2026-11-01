//! Clipboard integration and paste routing. Uses `arboard` first and falls
//! back to the common CLI tools (pbcopy/pbpaste, wl-clipboard, xclip, xsel).

use crate::app::App;
use crate::app::models::Mode;
use arboard::Clipboard;
use std::io::Write;
use std::process::{Command, Stdio};

impl App {
    pub fn handle_paste(&mut self, text: String) {
        match self.mode() {
            Mode::Search => self.paste_search_text(&text),
            Mode::Edit => self.paste_edit_text(&text),
            Mode::Staging => self.paste_staging_text(&text),
            Mode::EditCommitMessage => self.paste_commit_message_editor(&text),
            Mode::EditCommitField => self.paste_commit_field_editor(&text),
            Mode::SquashMessage => self.paste_squash_message_editor(&text),
            _ => {}
        }
    }

    pub fn can_paste_here(&self) -> bool {
        match self.mode() {
            Mode::Search => true,
            Mode::Edit => self
                .edit
                .as_ref()
                .map(|edit| edit.field != crate::app::EditField::Message)
                .unwrap_or(false),
            Mode::Staging => self
                .staging
                .as_ref()
                .map(|staging| staging.focus == crate::app::features::staging::StagingFocus::CommitMessage)
                .unwrap_or(false),
            Mode::EditCommitMessage | Mode::EditCommitField => self.commit_msg_editor.is_some(),
            Mode::SquashMessage => self.squash_msg.is_some(),
            _ => false,
        }
    }

    pub fn paste_from_clipboard(&mut self) {
        match Self::read_clipboard_text() {
            Some(text) if !text.is_empty() => self.handle_paste(text),
            _ => self.status_message = Some("Clipboard paste unavailable.".into()),
        }
    }

    pub(crate) fn read_clipboard_text() -> Option<String> {
        if let Ok(mut clipboard) = Clipboard::new()
            && let Ok(text) = clipboard.get_text()
            && !text.is_empty()
        {
            return Some(text);
        }

        let commands: [(&str, &[&str]); 4] = [
            ("pbpaste", &[]),
            ("wl-paste", &["-n"]),
            ("xclip", &["-selection", "clipboard", "-o"]),
            ("xsel", &["--clipboard", "--output"]),
        ];

        for (program, args) in commands {
            let output = match Command::new(program).args(args).output() {
                Ok(output) => output,
                Err(_) => continue,
            };
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout).into_owned();
                return Some(text);
            }
        }
        None
    }

    pub(crate) fn write_clipboard_text(&mut self, text: &str) -> bool {
        if self.clipboard.is_none() {
            self.clipboard = Clipboard::new().ok();
        }
        if let Some(clipboard) = self.clipboard.as_mut()
            && clipboard.set_text(text).is_ok()
        {
            return true;
        }

        let commands: [(&str, &[&str]); 4] = [
            ("pbcopy", &[]),
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ];
        for (program, args) in commands {
            let Ok(mut child) = Command::new(program)
                .args(args)
                .stdin(Stdio::piped())
                .spawn()
            else {
                continue;
            };
            if child
                .stdin
                .as_mut()
                .is_some_and(|stdin| stdin.write_all(text.as_bytes()).is_ok())
                && child.wait().is_ok_and(|status| status.success())
            {
                return true;
            }
        }
        false
    }
}
