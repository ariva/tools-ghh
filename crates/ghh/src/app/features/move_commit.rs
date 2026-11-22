//! Move mode (`Mode::Move`): pick a commit with `E`, slide it with ↑/↓ as a
//! live preview, Enter confirms and reorders history via `git rebase -i`.
//! A conflict leaves the rebase in progress; a popup offers continue/abort.

use crate::app::App;
use crate::app::models::{ConfirmAction, Mode, MoveState, VisibleRow};
use crate::git::{self, RebaseOutcome};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    /// Row content while a move is previewed (identity otherwise).
    pub(crate) fn display_idx(&self, idx: usize) -> usize {
        self.move_state.as_ref().map_or(idx, |m| m.display_idx(idx))
    }

    pub(crate) fn enter_move(&mut self) {
        if git::rebase_in_progress(&self.repo_path) {
            self.open_move_conflict("an earlier conflict".into());
            return;
        }
        if !self.drill_stack.is_empty() || self.search.filter_active {
            self.status_message =
                Some("Leave the drill view / search filter before moving.".into());
            return;
        }
        let Some(VisibleRow::Commit { idx, .. }) =
            self.list_state.selected().and_then(|i| self.visible.get(i))
        else {
            return;
        };
        let idx = *idx;
        let chain = match git::movable_chain(&self.repo_path, &self.commits[idx].hash) {
            Ok(chain) => chain,
            Err(e) => {
                self.status_message = Some(format!("Cannot move: {e}"));
                return;
            }
        };
        // Every chain commit must be a visible row, or the preview would skip rows.
        let chain: Option<Vec<usize>> = chain
            .iter()
            .map(|hash| self.commit_index.get(hash).copied())
            .collect();
        let Some(chain) = chain.filter(|c| c.len() > 1) else {
            self.status_message = Some("Cannot move: no other commit to move it past.".into());
            return;
        };
        let Some(from) = chain.iter().position(|&i| i == idx) else {
            return;
        };
        self.move_state = Some(MoveState {
            chain,
            from,
            pos: from,
        });
        self.nav.push(Mode::Move);
        self.status_message = Some(format!(
            "Moving {} — ↑/↓: move  Enter: apply  Esc: cancel",
            self.commits[idx].short_hash()
        ));
    }

    pub(crate) fn handle_move(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
            KeyCode::Esc => {
                if let Some(m) = self.move_state.take() {
                    self.select_commit_idx(m.chain[m.from]);
                }
                self.nav.pop();
                self.status_message = None;
            }
            KeyCode::Up | KeyCode::Char('k') => self.shift_move(false),
            KeyCode::Down | KeyCode::Char('j') => self.shift_move(true),
            KeyCode::Enter => self.confirm_move(),
            _ => {}
        }
        false
    }

    fn shift_move(&mut self, down: bool) {
        let Some(m) = self.move_state.as_mut() else {
            return;
        };
        let at_edge = if down {
            m.pos + 1 >= m.chain.len()
        } else {
            m.pos == 0
        };
        if at_edge {
            self.status_message = Some(if down {
                "Cannot move lower: merge commit or start of history below.".into()
            } else {
                "Already at the top (HEAD).".into()
            });
            return;
        }
        m.pos = if down { m.pos + 1 } else { m.pos - 1 };
        let (row, summary) = (m.chain[m.pos], self.move_summary());
        self.select_commit_idx(row);
        self.status_message = Some(format!("Moving {summary} — Enter: apply  Esc: cancel"));
    }

    /// "a1b2c3d 2 positions up"
    fn move_summary(&self) -> String {
        let Some(m) = &self.move_state else {
            return String::new();
        };
        let n = m.pos.abs_diff(m.from);
        format!(
            "{} {} position{} {}",
            self.commits[m.chain[m.from]].short_hash(),
            n,
            if n == 1 { "" } else { "s" },
            if m.pos < m.from { "up" } else { "down" }
        )
    }

    fn confirm_move(&mut self) {
        let Some(m) = &self.move_state else { return };
        if m.pos == m.from {
            self.status_message = Some("Not moved yet — use ↑/↓ first, Esc cancels.".into());
            return;
        }
        // Everything from the lowest affected row up to HEAD is re-picked,
        // oldest first, in the previewed order.
        let lowest = m.from.max(m.pos);
        let todo: Vec<String> = m.chain[..=lowest]
            .iter()
            .rev()
            .map(|&row| self.commits[m.display_idx(row)].hash.clone())
            .collect();
        let action = ConfirmAction::Move {
            lowest_hash: self.commits[m.chain[lowest]].hash.clone(),
            todo,
            summary: self.move_summary(),
        };
        self.nav.push(Mode::Confirm(action, false));
    }

    pub(crate) fn execute_move(&mut self, lowest_hash: String, todo: Vec<String>) {
        let row = self.list_state.selected();
        self.move_state = None;
        self.nav.reset(Mode::Normal);
        let result = git::move_commit(&self.repo_path, &lowest_hash, &todo);
        self.after_rebase_step(result, "Commit moved.");
        if row.is_some() && !self.visible.is_empty() {
            self.list_state
                .select(row.map(|r| r.min(self.visible.len() - 1)));
        }
    }

    /// Yes/No from the conflict popup.
    pub(crate) fn resolve_move_conflict(&mut self, continue_rebase: bool) {
        if continue_rebase {
            let result = git::rebase_continue(&self.repo_path);
            self.after_rebase_step(result, "Move completed.");
        } else {
            let message = match git::rebase_abort(&self.repo_path) {
                Ok(()) => "Move aborted — original history restored.".to_string(),
                Err(e) => format!("Abort failed: {e}"),
            };
            self.reload_after_rebase();
            self.status_message = Some(message);
        }
    }

    fn after_rebase_step(&mut self, result: git::Result<RebaseOutcome>, done: &str) {
        match result {
            Ok(RebaseOutcome::Done) => {
                self.reload_after_rebase();
                self.status_message = Some(done.into());
            }
            Ok(RebaseOutcome::Conflict(stopped_at)) => {
                self.reload_after_rebase();
                self.open_move_conflict(stopped_at);
            }
            Err(e) => self.status_message = Some(format!("Move error: {e}")),
        }
    }

    fn open_move_conflict(&mut self, stopped_at: String) {
        self.status_message = Some(
            "Rebase in progress — fix conflicts, stage them, then E: continue / abort.".into(),
        );
        self.nav.push(Mode::Confirm(
            ConfirmAction::MoveConflict { stopped_at },
            true,
        ));
    }

    fn reload_after_rebase(&mut self) {
        match git::load_commits(&self.repo_path, self.commit_sort) {
            Ok(commits) => self.replace_commits(commits),
            Err(e) => self.status_message = Some(format!("Reload failed: {e}")),
        }
    }
}
