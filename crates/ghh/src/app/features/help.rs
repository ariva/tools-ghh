//! Help popup content per screen.

use crate::app::App;
use crate::app::models::Mode;

impl App {
    pub fn can_open_help(&self) -> bool {
        match self.mode() {
            Mode::Search
            | Mode::Edit
            | Mode::EditCommitMessage
            | Mode::EditCommitField
            | Mode::SquashMessage => false,
            Mode::Staging => self
                .staging
                .as_ref()
                .map(|s| s.focus != crate::app::features::staging::StagingFocus::CommitMessage)
                .unwrap_or(true),
            _ => true,
        }
    }

    pub fn help_title(&self) -> &'static str {
        match self.mode() {
            Mode::Normal => "Commit List Help",
            Mode::Search => "Search Help",
            Mode::Staging => "Staging Help",
            Mode::Stash => "Stash Help",
            Mode::Squash => "Squash Help",
            Mode::Move => "Move Commit Help",
            Mode::Edit => "Edit Help",
            Mode::Confirm(..) => "Confirm Help",
            Mode::Debugger => "Debugger Help",
            _ => "Help",
        }
    }

    pub fn help_lines(&self) -> Vec<String> {
        match self.mode() {
            Mode::Normal => {
                // Lines starting with `─` are section dividers; the popup
                // draws them in the border colour (ui/popups/help.rs).
                let mut lines: Vec<String> = vec![
                    "── Move around ──".into(),
                    "j/k, ↑/↓   navigate commits".into(),
                    "PgUp/PgDn  jump by page (also P / p)".into(),
                    "g / G      jump top / bottom".into(),
                    "C-M-d/u    jump halfway toward bottom / top".into(),
                    "{ / }      previous / next branch- or tag-labeled commit".into(),
                    "[ / ]      first parent / nearest child".into(),
                    "── Merges and branches ──".into(),
                    "m          jump to the merge (PR) that brought commit to mainline".into(),
                    "b          back to the commit m jumped from".into(),
                    "M, S-Enter drill into containing merge (PR), commit stays selected".into(),
                    "Enter      drill into merge/collapsed branch commits".into(),
                    "Backspace  back to outer commit list".into(),
                    "Space, ←→  collapse/expand merge branch".into(),
                    "── Search ──".into(),
                    "/          search commits".into(),
                    "n / N      next / previous search match".into(),
                    "f          filter list to search matches (again: remove)".into(),
                    "F          clear search (matches + filter)".into(),
                    "── View ──".into(),
                    "v          toggle diff pane".into(),
                ];
                if self.show_commit_diff {
                    lines.push("Tab        switch focus list/diff".into());
                    lines.push("PgUp/PgDn  page list or scroll diff (also P / p)".into());
                    lines.push("Esc        hide diff".into());
                }
                lines.extend(
                    [
                        "l / L      cycle focused (gitk) / graph / simple view (L reverse)",
                        "o          commit order: topological / strictly by date",
                        ".          open pane context menu",
                        "── Commit, rewrite, stash ──",
                        "e          edit selected commit",
                        "E          move commit up/down (rewrites history)",
                        "           only commits above the newest merge; clean working tree;",
                        "           not in a drill view or filtered list",
                        "r          open squash picker",
                        "s          open staging (also Enter on row 0: uncommitted changes)",
                        "           on row 0: Tab to Details, then j/k p/P g/G scroll its files",
                        "S          browse stashes (view / apply / pop / drop)",
                        "── Other ──",
                        "h / ?      open help",
                        "Ctrl+L     repaint screen (fix rendering artifacts)",
                        "mouse      drag pane borders to resize (saved on quit)",
                        "q          quit",
                    ]
                    .map(String::from),
                );
                lines
            }
            Mode::Staging => vec![
                "Tab        cycle file list / diff / message".into(),
                "j/k, ↑/↓   navigate file list or scroll diff".into(),
                "Space      stage / unstage selected file".into(),
                "a / u      stage all / unstage all".into(),
                "c          focus commit message".into(),
                "Enter      commit with current message".into(),
                "Alt+Enter  open vim-like message editor".into(),
                "Tab        select Save / Cancel".into(),
                "Ctrl+V     paste message text".into(),
                "r          reload status".into(),
                "Esc        back or leave focused field".into(),
                "h / ?      open help".into(),
                "q          quit".into(),
            ],
            Mode::Search => {
                let mut lines: Vec<String> = vec![
                    "Tab        autocomplete or next field".into(),
                    "↑/↓        previous / next field".into(),
                    "Enter      search".into(),
                    "Esc        close search".into(),
                    "M-Bksp     clear field (also Ctrl+U)".into(),
                    "Dates      To pre-fills from From on focus".into(),
                    "File opts  action filter + full-history checkbox when a file is set".into(),
                    "".into(),
                ];
                // Field matching rules come from hint_for — the same text the
                // popup's bottom line shows, so help can't drift from it.
                use crate::app::SearchField as F;
                for field in [
                    F::Quick,
                    F::Hash,
                    F::Message,
                    F::Author,
                    F::DateFrom,
                    F::File,
                    F::Branch,
                    F::Tag,
                ] {
                    let rules = crate::ui::popups::search::hint_for(field)
                        .trim_start_matches("Ctrl+U: clear field. ");
                    let label = match field {
                        F::Quick => "Quick",
                        F::DateFrom => "Date",
                        F::File => "File",
                        other => other.label(),
                    };
                    lines.push(format!("{label:<10} {rules}"));
                }
                lines
            }
            Mode::Squash => vec![
                "j/k, ↑/↓   navigate commits".into(),
                "Space      mark/unmark commit for squash".into(),
                "Enter      continue to squash message".into(),
                "Esc        cancel squash mode".into(),
                "h / ?      open help".into(),
                "q          quit".into(),
            ],
            Mode::Stash => vec![
                "j/k, ↑/↓   select stash, or scroll changes when focused".into(),
                "g / G      first / last (stash or line)".into(),
                "Tab        switch focus: stash list / changes".into(),
                "PgUp/PgDn  page through the changes".into(),
                "a          apply to working tree (stash is kept)".into(),
                "p          pop: apply, then remove from stash list".into(),
                "d          drop: delete without applying".into(),
                "r          reload stash list".into(),
                "Esc, Bksp  back to commit list".into(),
                "h / ?      open help".into(),
                "q          quit".into(),
            ],
            Mode::Move => vec![
                "j/k, ↑/↓   move the commit down / up (preview only)".into(),
                "Enter      apply: asks to confirm, then rewrites history".into(),
                "Esc        cancel, nothing changed".into(),
                "Conflict   rebase stays in progress: fix + stage files,".into(),
                "           then E to continue or abort".into(),
                "h / ?      open help".into(),
                "q          quit".into(),
            ],
            Mode::Edit => vec![
                "Tab, ↑/↓   move between fields".into(),
                "Message field is read-only preview".into(),
                "Enter      open message editor or save other field changes".into(),
                "Alt+Enter  also opens message editor".into(),
                "Tab        select Save / Cancel".into(),
                "Ctrl+V     paste field text".into(),
                "Esc        cancel edit".into(),
                "q          quit app".into(),
            ],
            Mode::Confirm(..) => vec![
                "←→, Tab     switch Yes / No".into(),
                "Enter      confirm selected choice".into(),
                "y / n      quick confirm / cancel".into(),
                "Esc        cancel".into(),
                "h / ?      open help".into(),
            ],
            Mode::Debugger => vec![
                "j/k, ↑/↓   scroll log".into(),
                "g / G      oldest / latest".into(),
                "c          clear log".into(),
                "D, Esc, Backspace  close debugger".into(),
                "h / ?      open help".into(),
                "q          quit".into(),
            ],
            _ => vec!["Enter, Esc  close help".into()],
        }
    }
}
