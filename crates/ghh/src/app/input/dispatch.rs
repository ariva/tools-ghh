//! Global key dispatch plus handlers for the commit list, confirm popup and
//! debugger. Editor-style popups have their own handlers in their modules.

use crate::app::App;
use crate::app::models::{CommitListFocus, ConfirmAction, Mode};
use crate::events::{AppEvent, EventTime, format_key_event};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    /// Enhanced key handler: emits KeyPress + ScreenChange events around the dispatch.
    /// Call this from the event loop instead of `handle_key` directly.
    pub fn process_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        let screen_before = self.current_screen();
        // Don't log key presses while the debugger itself is open — it would
        // flood the log with navigation noise.
        if !matches!(self.mode(), Mode::Debugger) {
            self.event_bus.emit(AppEvent::KeyPress {
                display: format_key_event(key),
                screen: screen_before.clone(),
                at: EventTime::now(),
            });
        }
        let should_quit = self.handle_key(key);
        let screen_after = self.current_screen();
        if screen_before != screen_after {
            self.event_bus.emit(AppEvent::ScreenChange {
                from: screen_before,
                to: screen_after,
                at: EventTime::now(),
            });
        }
        should_quit
    }

    /// Returns `true` if the app should quit.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if self.context_menu.is_some() {
            match key.code {
                KeyCode::Esc | KeyCode::Tab | KeyCode::BackTab => {
                    self.context_menu = None;
                }
                KeyCode::Up => {
                    if let Some(menu) = self.context_menu.as_mut() {
                        menu.selected = menu
                            .selected
                            .checked_sub(1)
                            .unwrap_or(menu.actions.len() - 1);
                    }
                }
                KeyCode::Down => {
                    if let Some(menu) = self.context_menu.as_mut() {
                        menu.selected = (menu.selected + 1) % menu.actions.len();
                    }
                }
                KeyCode::Char(shortcut @ '0'..='9') => {
                    let action_index = self.context_menu.as_ref().and_then(|menu| {
                        menu.action_index_for_shortcut(shortcut.to_digit(10)? as usize)
                    });
                    if let Some(action_index) = action_index {
                        if let Some(menu) = self.context_menu.as_mut() {
                            menu.selected = action_index;
                        }
                        self.execute_context_menu_action();
                    }
                }
                KeyCode::Enter => {
                    self.execute_context_menu_action();
                }
                _ => {}
            }
            return false;
        }
        if self.show_help {
            match key.code {
                KeyCode::Char('h')
                | KeyCode::Char('q')
                | KeyCode::Enter
                | KeyCode::Esc
                | KeyCode::Tab
                | KeyCode::BackTab => {
                    self.show_help = false;
                }
                _ => {}
            }
            return false;
        }
        if matches!(key.code, KeyCode::Char('h') | KeyCode::Char('?')) && self.can_open_help() {
            self.show_help = true;
            return false;
        }
        if key.code == KeyCode::Esc && self.text_selection.is_some() {
            self.clear_text_selection();
            return false;
        }
        if key.code == KeyCode::Char('l') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.force_redraw = true;
            return false;
        }
        if key.code == KeyCode::Char('v')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && self.can_paste_here()
        {
            self.paste_from_clipboard();
            return false;
        }
        if key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && self
                .text_selection
                .as_ref()
                .is_some_and(|selection| !selection.text.trim().is_empty())
        {
            self.copy_selected_text();
            return false;
        }
        // Shift+D (uppercase D) toggles the debugger overlay, except while a
        // text field is being typed into (e.g. "README.md" in search).
        let typing = matches!(
            self.mode(),
            Mode::Search
                | Mode::Edit
                | Mode::SquashMessage
                | Mode::EditCommitMessage
                | Mode::EditCommitField
        );
        if key.code == KeyCode::Char('D') && !typing {
            self.toggle_debugger();
            return false;
        }
        match self.mode() {
            Mode::Normal => self.handle_normal(key),
            Mode::Search => self.handle_search(key),
            Mode::Edit => self.handle_edit(key),
            Mode::Staging => self.handle_staging(key),
            Mode::Stash => self.handle_stash(key),
            Mode::Squash => self.handle_squash(key),
            Mode::Move => self.handle_move(key),
            Mode::SquashMessage => self.handle_squash_message(key),
            Mode::EditCommitMessage => self.handle_commit_message_editor(key),
            Mode::EditCommitField => self.handle_commit_field_editor(key),
            Mode::Debugger => self.handle_debugger(key),
            Mode::Confirm(..) => self.handle_confirm(key),
        }
    }

    pub(crate) fn handle_normal(&mut self, key: KeyEvent) -> bool {
        let page_size = self.commit_list_page_size();
        let (diff_info_page_size, diff_page_size) = self.commit_diff_page_steps();
        // The Details pane takes focus only for the "Uncommitted changes"
        // row; once another row is selected the list has it again.
        if self.commit_list_focus == CommitListFocus::Details && !self.working_tree_selected() {
            self.commit_list_focus = CommitListFocus::List;
        }
        let panes_to_cycle = self.show_commit_diff || self.working_tree_selected();
        let details_page = self
            .commit_details_region()
            .map_or(1, crate::app::layout::pane_page_step) as isize;
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
            KeyCode::Esc if self.show_commit_diff => self.toggle_commit_diff(),
            KeyCode::Esc if self.commit_list_focus == CommitListFocus::Details => {
                self.commit_list_focus = CommitListFocus::List;
            }
            KeyCode::BackTab if panes_to_cycle => self.cycle_commit_diff_focus(false),
            KeyCode::Tab if panes_to_cycle && key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.cycle_commit_diff_focus(false);
            }
            KeyCode::Tab if panes_to_cycle => self.cycle_commit_diff_focus(true),
            KeyCode::Down | KeyCode::Char('j') => match self.commit_list_focus {
                CommitListFocus::List => {
                    self.move_down();
                    self.mark_perf_action("commit_list.action.move_down");
                    self.commit_diff_scroll = 0;
                    self.commit_diff_info_scroll = 0;
                    self.schedule_selected_stats_load();
                }
                CommitListFocus::DiffInfo => {
                    self.mark_perf_action("commit_list.action.diff_info_scroll_down");
                    let previous = self.commit_diff_info_scroll;
                    self.commit_diff_info_scroll = self.commit_diff_info_scroll.saturating_add(1);
                    if let Some((info, _)) = self.commit_diff_regions() {
                        self.preserve_text_selection_on_scroll(
                            info,
                            true,
                            self.commit_diff_info_scroll.abs_diff(previous),
                        );
                    }
                }
                CommitListFocus::Diff => {
                    self.mark_perf_action("commit_list.action.diff_scroll_down");
                    let previous = self.commit_diff_scroll;
                    self.commit_diff_scroll = self.commit_diff_scroll.saturating_add(1);
                    if let Some((_, diff)) = self.commit_diff_regions() {
                        self.preserve_text_selection_on_scroll(
                            diff,
                            true,
                            self.commit_diff_scroll.abs_diff(previous),
                        );
                    }
                }
                CommitListFocus::Details => self.scroll_working_tree_details(1),
            },
            KeyCode::Up | KeyCode::Char('k') => match self.commit_list_focus {
                CommitListFocus::List => {
                    self.move_up();
                    self.mark_perf_action("commit_list.action.move_up");
                    self.commit_diff_scroll = 0;
                    self.commit_diff_info_scroll = 0;
                    self.schedule_selected_stats_load();
                }
                CommitListFocus::DiffInfo => {
                    self.mark_perf_action("commit_list.action.diff_info_scroll_up");
                    let previous = self.commit_diff_info_scroll;
                    self.commit_diff_info_scroll = self.commit_diff_info_scroll.saturating_sub(1);
                    if let Some((info, _)) = self.commit_diff_regions() {
                        self.preserve_text_selection_on_scroll(
                            info,
                            false,
                            previous.abs_diff(self.commit_diff_info_scroll),
                        );
                    }
                }
                CommitListFocus::Diff => {
                    self.mark_perf_action("commit_list.action.diff_scroll_up");
                    let previous = self.commit_diff_scroll;
                    self.commit_diff_scroll = self.commit_diff_scroll.saturating_sub(1);
                    if let Some((_, diff)) = self.commit_diff_regions() {
                        self.preserve_text_selection_on_scroll(
                            diff,
                            false,
                            previous.abs_diff(self.commit_diff_scroll),
                        );
                    }
                }
                CommitListFocus::Details => self.scroll_working_tree_details(-1),
            },
            KeyCode::Char('g') | KeyCode::Home => match self.commit_list_focus {
                CommitListFocus::List => {
                    self.move_top();
                    self.mark_perf_action("commit_list.action.move_top");
                    self.commit_diff_scroll = 0;
                    self.commit_diff_info_scroll = 0;
                    self.schedule_selected_stats_load();
                }
                CommitListFocus::DiffInfo => {
                    self.mark_perf_action("commit_list.action.diff_info_scroll_top");
                    self.commit_diff_info_scroll = 0;
                }
                CommitListFocus::Diff => {
                    self.mark_perf_action("commit_list.action.diff_scroll_top");
                    self.commit_diff_scroll = 0
                }
                CommitListFocus::Details => self.working_tree.details_scroll = 0,
            },
            KeyCode::Char('G') | KeyCode::End => {
                if self.commit_list_focus == CommitListFocus::Details {
                    self.scroll_working_tree_details(isize::MAX);
                }
                if self.commit_list_focus == CommitListFocus::List {
                    if self.full_history_required("jumping to bottom") {
                        return false;
                    }
                    self.move_bottom();
                    self.mark_perf_action("commit_list.action.move_bottom");
                    self.commit_diff_scroll = 0;
                    self.commit_diff_info_scroll = 0;
                    self.schedule_selected_stats_load();
                }
            }
            KeyCode::Char('E') if self.commit_list_focus == CommitListFocus::List => {
                if self.full_history_required("moving commits") {
                    return false;
                }
                self.mark_perf_action("commit_list.action.enter_move");
                self.enter_move();
            }
            KeyCode::Char('m') if self.commit_list_focus == CommitListFocus::List => {
                self.jump_to_merge_point();
                self.mark_perf_action("commit_list.action.jump_merge_point");
            }
            KeyCode::Char('b') if self.commit_list_focus == CommitListFocus::List => {
                self.jump_back_from_merge_point();
                self.mark_perf_action("commit_list.action.jump_back_from_merge_point");
            }
            KeyCode::Char(c @ ('{' | '}')) if self.commit_list_focus == CommitListFocus::List => {
                self.jump_labeled(c == '}');
                self.mark_perf_action("commit_list.action.jump_labeled");
            }
            KeyCode::Char(c @ ('[' | ']')) if self.commit_list_focus == CommitListFocus::List => {
                self.jump_parent_or_child(c == '[');
                self.mark_perf_action("commit_list.action.jump_parent_child");
            }
            // Halfway jumps (gitk has nothing like it; borrowed from guitar):
            // binary-search seeking through six-figure histories in ~20 keys.
            KeyCode::Char('d' | 'u')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.modifiers.contains(KeyModifiers::ALT)
                    && self.commit_list_focus == CommitListFocus::List =>
            {
                self.move_halfway(key.code == KeyCode::Char('d'));
                self.mark_perf_action("commit_list.action.move_halfway");
                self.commit_diff_scroll = 0;
                self.commit_diff_info_scroll = 0;
                self.schedule_selected_stats_load();
            }
            // p / P page like PgDn / PgUp, for keyboards without those keys.
            KeyCode::PageDown | KeyCode::Char('p') => match self.commit_list_focus {
                CommitListFocus::List => {
                    self.move_page_down(page_size);
                    self.mark_perf_action("commit_list.action.page_down");
                    self.commit_diff_scroll = 0;
                    self.commit_diff_info_scroll = 0;
                    self.schedule_selected_stats_load();
                }
                CommitListFocus::DiffInfo if self.show_commit_diff => {
                    self.mark_perf_action("commit_list.action.diff_info_page_down");
                    let previous = self.commit_diff_info_scroll;
                    self.commit_diff_info_scroll = self
                        .commit_diff_info_scroll
                        .saturating_add(diff_info_page_size);
                    if let Some((info, _)) = self.commit_diff_regions() {
                        self.preserve_text_selection_on_scroll(
                            info,
                            true,
                            self.commit_diff_info_scroll.abs_diff(previous),
                        );
                    }
                }
                CommitListFocus::Diff if self.show_commit_diff => {
                    self.mark_perf_action("commit_list.action.diff_page_down");
                    let previous = self.commit_diff_scroll;
                    self.commit_diff_scroll =
                        self.commit_diff_scroll.saturating_add(diff_page_size);
                    if let Some((_, diff)) = self.commit_diff_regions() {
                        self.preserve_text_selection_on_scroll(
                            diff,
                            true,
                            self.commit_diff_scroll.abs_diff(previous),
                        );
                    }
                }
                CommitListFocus::DiffInfo => {}
                CommitListFocus::Diff => {}
                CommitListFocus::Details => self.scroll_working_tree_details(details_page),
            },
            KeyCode::PageUp | KeyCode::Char('P') => match self.commit_list_focus {
                CommitListFocus::List => {
                    self.move_page_up(page_size);
                    self.mark_perf_action("commit_list.action.page_up");
                    self.commit_diff_scroll = 0;
                    self.commit_diff_info_scroll = 0;
                    self.schedule_selected_stats_load();
                }
                CommitListFocus::DiffInfo if self.show_commit_diff => {
                    self.mark_perf_action("commit_list.action.diff_info_page_up");
                    let previous = self.commit_diff_info_scroll;
                    self.commit_diff_info_scroll = self
                        .commit_diff_info_scroll
                        .saturating_sub(diff_info_page_size);
                    if let Some((info, _)) = self.commit_diff_regions() {
                        self.preserve_text_selection_on_scroll(
                            info,
                            false,
                            previous.abs_diff(self.commit_diff_info_scroll),
                        );
                    }
                }
                CommitListFocus::Diff if self.show_commit_diff => {
                    self.mark_perf_action("commit_list.action.diff_page_up");
                    let previous = self.commit_diff_scroll;
                    self.commit_diff_scroll =
                        self.commit_diff_scroll.saturating_sub(diff_page_size);
                    if let Some((_, diff)) = self.commit_diff_regions() {
                        self.preserve_text_selection_on_scroll(
                            diff,
                            false,
                            previous.abs_diff(self.commit_diff_scroll),
                        );
                    }
                }
                CommitListFocus::DiffInfo => {}
                CommitListFocus::Diff => {}
                CommitListFocus::Details => self.scroll_working_tree_details(-details_page),
            },
            KeyCode::Char('e') => {
                if self.full_history_required("editing commits") {
                    return false;
                }
                self.mark_perf_action("commit_list.action.enter_edit");
                self.enter_edit()
            }
            KeyCode::Char('v') => {
                self.mark_perf_action("commit_list.action.toggle_diff");
                self.toggle_commit_diff()
            }
            KeyCode::Char('.') => {
                self.mark_perf_action("commit_list.action.open_context_menu");
                self.open_context_menu_for_active_pane()
            }
            KeyCode::Char('l') => {
                self.mark_perf_action("commit_list.action.cycle_commit_list_view");
                self.cycle_commit_list_view()
            }
            KeyCode::Char('L') => {
                self.mark_perf_action("commit_list.action.cycle_commit_list_view_back");
                self.cycle_commit_list_view_back()
            }
            KeyCode::Char('M') => {
                self.mark_perf_action("commit_list.action.drill_into_merge_of_selected");
                self.drill_into_merge_of_selected()
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.mark_perf_action("commit_list.action.drill_into_merge_of_selected");
                self.drill_into_merge_of_selected()
            }
            // On the "Uncommitted changes" row Enter opens staging, like `s`.
            KeyCode::Enter if self.working_tree_selected() => {
                self.commit_list_focus = CommitListFocus::List;
                if self.full_history_required("opening staging") {
                    return false;
                }
                self.mark_perf_action("commit_list.action.enter_staging");
                self.enter_staging()
            }
            KeyCode::Enter => {
                self.mark_perf_action("commit_list.action.drill_into_selected");
                self.drill_into_selected()
            }
            KeyCode::Backspace => {
                self.mark_perf_action("commit_list.action.drill_back");
                self.drill_back()
            }
            KeyCode::Char('o') => {
                self.mark_perf_action("commit_list.action.toggle_commit_sort");
                self.toggle_commit_sort()
            }
            KeyCode::Char('/') => self.open_search(),
            // Shift reverses the match-hop direction: N = previous.
            KeyCode::Char('n') => self.move_search_match(true),
            KeyCode::Char('N') => self.move_search_match(false),
            KeyCode::Char('f') => self.toggle_search_filter(),
            KeyCode::Char('F') => self.clear_search_results(),
            KeyCode::Char('s') => {
                if self.full_history_required("opening staging") {
                    return false;
                }
                self.mark_perf_action("commit_list.action.enter_staging");
                self.enter_staging()
            }
            KeyCode::Char('S') => {
                self.mark_perf_action("commit_list.action.enter_stash");
                self.enter_stash()
            }
            KeyCode::Char('r') => {
                if self.full_history_required("starting squash") {
                    return false;
                }
                self.mark_perf_action("commit_list.action.enter_squash");
                self.enter_squash()
            }
            KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right => {
                if self.full_history_required("collapsing branches") {
                    return false;
                }
                self.mark_perf_action("commit_list.action.toggle_collapse");
                self.toggle_collapse()
            }
            _ => {}
        }
        false
    }

    /// Keys inside the confirmation popup. Cancelling simply pops the popup
    /// off the navigation stack, which lands back on whichever screen opened
    /// it — no per-action restore table needed.
    pub(crate) fn handle_confirm(&mut self, key: KeyEvent) -> bool {
        if matches!(
            key.code,
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab
        ) {
            if let Mode::Confirm(_, yes) = self.nav.mode_mut() {
                *yes = !*yes;
            }
            return false;
        }

        // Conflict popup: Esc must not abort the rebase — it only hides the
        // popup so the user can go fix things (m reopens it).
        if key.code == KeyCode::Esc
            && matches!(
                self.mode(),
                Mode::Confirm(ConfirmAction::MoveConflict { .. }, _)
            )
        {
            self.nav.pop();
            return false;
        }

        let confirmed = match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => true,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => false,
            KeyCode::Enter => match self.mode() {
                Mode::Confirm(_, yes) => *yes,
                _ => false,
            },
            _ => return false,
        };

        // Close the popup; the opener's mode becomes active again.
        let Some(Mode::Confirm(action, _)) = self.nav.pop() else {
            return false;
        };

        if confirmed {
            match action {
                ConfirmAction::RemoveOrSegment {
                    field_index,
                    segment_index,
                    ..
                } => self.remove_or_segment_confirmed(field_index, segment_index),
                ConfirmAction::SaveEdit => self.execute_save_edit(),
                ConfirmAction::SaveEditFromMessageEditor => {
                    self.save_commit_field_editor_confirmed();
                }
                ConfirmAction::DiscardEditMessageEditor => self.discard_commit_field_editor(),
                ConfirmAction::DiscardStagingMessageEditor => self.discard_commit_message_editor(),
                ConfirmAction::DiscardSquashMessageEditor => self.discard_squash_message_editor(),
                ConfirmAction::Commit { msg } => return self.execute_commit(msg),
                ConfirmAction::Squash {
                    base_hash,
                    squash_hashes,
                    message,
                } => self.execute_squash_with_message(base_hash, squash_hashes, message),
                ConfirmAction::Move {
                    lowest_hash, todo, ..
                } => self.execute_move(lowest_hash, todo),
                ConfirmAction::MoveConflict { .. } => self.resolve_move_conflict(true),
                ConfirmAction::StashPop { entry } => self.execute_stash_apply(entry, true),
                ConfirmAction::StashDrop { entry } => self.execute_stash_drop(entry),
            }
        } else if matches!(action, ConfirmAction::MoveConflict { .. }) {
            self.resolve_move_conflict(false);
        } else if matches!(action, ConfirmAction::Squash { .. }) {
            // Back in the squash picker: put the cursor on the base commit.
            if let Some(sq) = &self.squash {
                let base_idx = sq.base_idx;
                self.select_commit_idx(base_idx);
            }
        }
        false
    }

    pub(crate) fn handle_debugger(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('D') => self.toggle_debugger(),
            KeyCode::Down | KeyCode::Char('j') => self.event_bus.scroll_down(),
            KeyCode::Up | KeyCode::Char('k') => self.event_bus.scroll_up(),
            KeyCode::Char('g') | KeyCode::Home => self.event_bus.scroll_to_start(),
            KeyCode::Char('G') | KeyCode::End => self.event_bus.jump_to_end(),
            KeyCode::Char('c') => self.event_bus.clear(),
            _ => {}
        }
        false
    }
}
