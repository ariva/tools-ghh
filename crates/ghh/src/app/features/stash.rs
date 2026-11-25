//! Stash screen (`Mode::Stash`): stash list on the left, the selected stash's
//! patch on the right, details at the bottom; apply / pop / drop actions.

use crate::app::App;
use crate::app::models::{ConfirmAction, Mode};
use crate::git::{self, StashEntry};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;
use std::path::Path;

pub struct StashState {
    pub entries: Vec<StashEntry>,
    pub list_state: ListState,
    pub diff_lines: Vec<String>,
    pub diff_scroll: usize,
    /// `git stash show --stat` lines, summary line first.
    pub stat_lines: Vec<String>,
    pub diff_focused: bool,
}

impl StashState {
    pub fn new(repo_path: &Path) -> git::Result<Self> {
        let mut state = Self {
            entries: Vec::new(),
            list_state: ListState::default(),
            diff_lines: Vec::new(),
            diff_scroll: 0,
            stat_lines: Vec::new(),
            diff_focused: false,
        };
        state.reload(repo_path)?;
        Ok(state)
    }

    /// Re-read the stash list, keeping the cursor near where it was.
    pub fn reload(&mut self, repo_path: &Path) -> git::Result<()> {
        self.entries = git::list_stashes(repo_path)?;
        let selected = self.list_state.selected().unwrap_or(0);
        self.select(repo_path, selected);
        Ok(())
    }

    pub fn selected(&self) -> Option<&StashEntry> {
        self.list_state.selected().and_then(|i| self.entries.get(i))
    }

    /// Select row `index` (clamped) and load its patch + stat.
    pub fn select(&mut self, repo_path: &Path, index: usize) {
        self.diff_scroll = 0;
        self.diff_lines.clear();
        self.stat_lines.clear();
        if self.entries.is_empty() {
            self.list_state.select(None);
            return;
        }
        self.list_state
            .select(Some(index.min(self.entries.len() - 1)));
        let hash = self.selected().map(|e| e.hash.clone()).unwrap_or_default();
        self.diff_lines = match git::stash_patch(repo_path, &hash) {
            Ok(patch) => patch.lines().map(String::from).collect(),
            Err(e) => vec![format!("Error loading stash: {e}")],
        };
        if let Ok(stat) = git::stash_stat(repo_path, &hash) {
            self.stat_lines = stat.lines().map(|l| l.trim().to_string()).collect();
            // Summary ("N files changed, …") is last in git's output; show it first.
            self.stat_lines.rotate_right(1);
        }
    }

    pub fn scroll_diff(&mut self, delta: isize) {
        let max = self.diff_lines.len().saturating_sub(1);
        self.diff_scroll = self.diff_scroll.saturating_add_signed(delta).min(max);
    }
}

impl App {
    pub(crate) fn enter_stash(&mut self) {
        match StashState::new(&self.repo_path) {
            Ok(state) => {
                self.status_message = state.entries.is_empty().then(|| "No stashes.".to_string());
                self.stash = Some(state);
                self.nav.push(Mode::Stash);
            }
            Err(e) => self.status_message = Some(format!("Error loading stashes: {e}")),
        }
    }

    pub(crate) fn handle_stash(&mut self, key: KeyEvent) -> bool {
        let repo_path = self.repo_path.clone();
        let page = self
            .stash_regions()
            .map_or(10, |(_, diff, _)| crate::app::layout::pane_page_step(diff))
            as isize;
        let Some(stash) = self.stash.as_mut() else {
            self.nav.reset(Mode::Normal);
            return false;
        };
        let row = stash.list_state.selected().unwrap_or(0);
        // j/k and g/G drive whichever pane has focus; PgUp/PgDn always the diff.
        let step = |stash: &mut StashState, delta: isize| {
            if stash.diff_focused {
                stash.scroll_diff(delta);
            } else {
                stash.select(&repo_path, row.saturating_add_signed(delta));
            }
        };
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
            KeyCode::Esc | KeyCode::Backspace => {
                self.stash = None;
                self.nav.pop();
                self.status_message = None;
                self.refresh_working_tree();
            }
            KeyCode::Tab | KeyCode::BackTab => stash.diff_focused = !stash.diff_focused,
            KeyCode::Down | KeyCode::Char('j') => step(stash, 1),
            KeyCode::Up | KeyCode::Char('k') => step(stash, -1),
            KeyCode::Char('g') | KeyCode::Home => step(stash, isize::MIN),
            KeyCode::Char('G') | KeyCode::End => step(stash, isize::MAX),
            KeyCode::PageDown => stash.scroll_diff(page),
            KeyCode::PageUp => stash.scroll_diff(-page),
            KeyCode::Char('r') => {
                let result = stash.reload(&repo_path);
                self.status_message = Some(match result {
                    Ok(()) => "Stash list reloaded.".into(),
                    Err(e) => format!("Reload error: {e}"),
                });
            }
            KeyCode::Char('a') => {
                if let Some(entry) = stash.selected().cloned() {
                    self.execute_stash_apply(entry, false);
                }
            }
            KeyCode::Char('p') => {
                if let Some(entry) = stash.selected().cloned() {
                    self.nav
                        .push(Mode::Confirm(ConfirmAction::StashPop { entry }, true));
                }
            }
            KeyCode::Char('d') => {
                if let Some(entry) = stash.selected().cloned() {
                    self.nav
                        .push(Mode::Confirm(ConfirmAction::StashDrop { entry }, false));
                }
            }
            _ => {}
        }
        false
    }

    pub(crate) fn execute_stash_apply(&mut self, entry: StashEntry, pop: bool) {
        let verb = if pop { "Popped" } else { "Applied" };
        let result = git::stash_apply(&self.repo_path, &entry, pop);
        self.finish_stash_action(
            result,
            format!("{verb} {} to the working tree.", entry.name),
        );
    }

    pub(crate) fn execute_stash_drop(&mut self, entry: StashEntry) {
        let result = git::stash_drop(&self.repo_path, &entry);
        self.finish_stash_action(
            result,
            format!("Dropped {} ({}).", entry.name, &entry.hash[..7]),
        );
    }

    fn finish_stash_action(&mut self, result: git::Result<()>, done: String) {
        // Reload either way: a failed pop keeps the stash, an external change shifts it.
        if let Some(stash) = self.stash.as_mut() {
            let _ = stash.reload(&self.repo_path);
        }
        self.status_message = Some(match result {
            Ok(()) => done,
            Err(e) => format!("Stash error: {e}"),
        });
    }
}
