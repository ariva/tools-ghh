//! Mouse handling: clicks, double-click word selection, drag selection and
//! mode-aware scrolling.

use crate::app::App;
use crate::app::models::{CommitListFocus, ContextMenuAction, Mode, ResizeTarget, TextSelection};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::time::{Duration, Instant};

const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(400);

impl App {
    /// Router only: each popup/mode gets its own handler below. Order
    /// matters — overlays first, then menus, then selection, then modes.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> bool {
        let click = matches!(mouse.kind, MouseEventKind::Down(_));
        let left_click = mouse.kind == MouseEventKind::Down(MouseButton::Left);
        let right_click = mouse.kind == MouseEventKind::Down(MouseButton::Right);

        if self.show_help {
            return self.mouse_in_help(mouse);
        }
        if left_click && matches!(self.mode(), Mode::EditCommitMessage | Mode::EditCommitField) {
            return self.mouse_in_editor_buttons(mouse);
        }
        if click && matches!(self.mode(), Mode::Confirm(..)) {
            return self.mouse_in_confirm(mouse);
        }
        if left_click && matches!(self.mode(), Mode::Edit) {
            return self.mouse_in_edit(mouse);
        }
        if left_click && matches!(self.mode(), Mode::Search) {
            return self.mouse_in_search(mouse);
        }
        if left_click && matches!(self.mode(), Mode::SquashMessage) {
            return self.mouse_in_squash_message(mouse);
        }
        if self.context_menu.is_some() {
            return self.mouse_on_context_menu(mouse);
        }
        if right_click && let Some(handled) = self.mouse_right_click_menus(mouse) {
            return handled;
        }
        if self.popup_visible() {
            return false;
        }
        if let Some(handled) = self.mouse_resize(mouse) {
            return handled;
        }
        if let Some(handled) = self.mouse_text_selection(mouse) {
            return handled;
        }
        self.mouse_scroll_and_click(mouse)
    }

    /// Help overlay: [X]/[ OK ]/outside close, the wheel scrolls the text;
    /// everything else is inert.
    fn mouse_in_help(&mut self, mouse: MouseEvent) -> bool {
        let click = matches!(mouse.kind, MouseEventKind::Down(_));
        if self.show_help {
            match mouse.kind {
                MouseEventKind::ScrollUp => self.wheel_help(true),
                MouseEventKind::ScrollDown => self.wheel_help(false),
                _ => {}
            }
            if click {
                match self.help_popup_rect() {
                    Some(popup) if Self::rect_contains(popup, mouse.column, mouse.row) => {
                        // Inside: only the close controls act.
                        let on_close = Self::rect_contains(
                            crate::app::layout::help_close_rect(popup),
                            mouse.column,
                            mouse.row,
                        ) || Self::rect_contains(
                            crate::app::layout::help_ok_rect(popup),
                            mouse.column,
                            mouse.row,
                        );
                        if on_close {
                            self.show_help = false;
                        }
                    }
                    _ => self.show_help = false, // outside (or no rect): close
                }
            }
            return false;
        }
        false
    }

    /// Multiline editor popups: Save/Cancel buttons, outside = escape.
    fn mouse_in_editor_buttons(&mut self, mouse: MouseEvent) -> bool {
        let left_click = mouse.kind == MouseEventKind::Down(MouseButton::Left);
        if left_click
            && matches!(self.mode(), Mode::EditCommitMessage | Mode::EditCommitField)
            && let Some((popup, save, cancel)) = self.multiline_editor_button_rects()
        {
            if Self::rect_contains(save, mouse.column, mouse.row) {
                self.editor_save_selected = true;
                return self.activate_multiline_editor_action();
            }
            if Self::rect_contains(cancel, mouse.column, mouse.row) {
                self.editor_save_selected = false;
                self.discard_multiline_editor();
                return false;
            }
            if !Self::rect_contains(popup, mouse.column, mouse.row) {
                self.escape_multiline_editor();
                return false;
            }
            return false;
        }
        false
    }

    /// Confirm dialog: Yes/No buttons, outside = Esc.
    fn mouse_in_confirm(&mut self, mouse: MouseEvent) -> bool {
        let click = matches!(mouse.kind, MouseEventKind::Down(_));
        let left_click = mouse.kind == MouseEventKind::Down(MouseButton::Left);
        if click && matches!(self.mode(), Mode::Confirm(..)) {
            if let Some((popup, yes, no)) = self.confirm_popup_button_rects() {
                if left_click && Self::rect_contains(yes, mouse.column, mouse.row) {
                    if let Mode::Confirm(_, selected) = self.nav.mode_mut() {
                        *selected = true;
                    }
                    return self.handle_confirm(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                }
                if left_click && Self::rect_contains(no, mouse.column, mouse.row) {
                    if let Mode::Confirm(_, selected) = self.nav.mode_mut() {
                        *selected = false;
                    }
                    return self.handle_confirm(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                }
                if !Self::rect_contains(popup, mouse.column, mouse.row) {
                    return self.handle_confirm(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                }
            }
            return false;
        }
        false
    }

    /// Edit popup: click outside cancels.
    fn mouse_in_edit(&mut self, mouse: MouseEvent) -> bool {
        let left_click = mouse.kind == MouseEventKind::Down(MouseButton::Left);
        if left_click && matches!(self.mode(), Mode::Edit) {
            if self
                .edit_popup_rect()
                .map(|popup| Self::rect_contains(popup, mouse.column, mouse.row))
                != Some(true)
            {
                self.cancel_edit();
            }
            return false;
        }
        false
    }

    /// Search popup: outside cancels; inside resolves pixels (char pos,
    /// double-click) and delegates semantics to apply_search_popup_hit.
    fn mouse_in_search(&mut self, mouse: MouseEvent) -> bool {
        let left_click = mouse.kind == MouseEventKind::Down(MouseButton::Left);
        if left_click && matches!(self.mode(), Mode::Search) {
            if self
                .search_popup_rect()
                .map(|popup| Self::rect_contains(popup, mouse.column, mouse.row))
                != Some(true)
            {
                self.cancel_search();
                return false;
            }
            // Activate on press only; the matching release must not re-fire.
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                && let Some(hit) = self.search_popup_hit(mouse.column, mouse.row)
            {
                use crate::app::layout::SearchPopupHit;
                // Pixel work stays here: double-click detection and mapping
                // the click column to a char position through the SAME layout
                // the renderer used. Semantics live in apply_search_popup_hit
                // (deterministic, tested without a terminal).
                let popup = self.search_popup_rect();
                let inner_width = popup.map(|p| p.width.saturating_sub(4)).unwrap_or(0);
                let mut clicked_char_pos = None;
                let mut double_click = false;
                if let SearchPopupHit::Field(index) = hit {
                    let field = crate::app::SearchField::ALL[index];
                    let now = Instant::now();
                    // Double-click on the already-focused field selects its
                    // whole value (Backspace then clears it).
                    double_click = self.search.field == field
                        && self.last_left_click.is_some_and(|(last, column, row)| {
                            now.duration_since(last) <= DOUBLE_CLICK_INTERVAL
                                && row == mouse.row
                                && column.abs_diff(mouse.column) <= 1
                        });
                    self.last_left_click = Some((now, mouse.column, mouse.row));
                    clicked_char_pos = popup.map(|popup| {
                        let was_active = self.search.field == field;
                        let value_len = self.search.value(field).chars().count();
                        let multi =
                            crate::app::features::search::SearchState::multi_value_separator(field)
                                .is_some();
                        let layout = crate::app::layout::layout_field_line(
                            popup.width.saturating_sub(4),
                            value_len,
                            was_active.then(|| self.search.cursor.min(value_len)),
                            multi && was_active,
                            true,
                            16,
                        );
                        crate::app::layout::char_pos_at(
                            layout,
                            value_len,
                            mouse.column.saturating_sub(popup.x + 2),
                        )
                    });
                }
                self.apply_search_popup_hit(hit, clicked_char_pos, double_click, inner_width);
            }
            return false;
        }
        false
    }

    /// Squash message editor: click outside = Esc.
    fn mouse_in_squash_message(&mut self, mouse: MouseEvent) -> bool {
        let left_click = mouse.kind == MouseEventKind::Down(MouseButton::Left);
        if left_click && matches!(self.mode(), Mode::SquashMessage) {
            if self
                .multiline_editor_popup_rect()
                .map(|popup| Self::rect_contains(popup, mouse.column, mouse.row))
                != Some(true)
            {
                return self.handle_squash_message(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            }
            return false;
        }
        false
    }

    /// Open context menu: item click executes, elsewhere closes.
    fn mouse_on_context_menu(&mut self, mouse: MouseEvent) -> bool {
        let click = matches!(mouse.kind, MouseEventKind::Down(_));
        if self.context_menu.is_some() {
            let menu_rect = self.context_menu_rect();
            let menu_left_click = matches!(
                mouse.kind,
                MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
            );
            if click || menu_left_click {
                let inside = menu_rect
                    .map(|rect| Self::rect_contains(rect, mouse.column, mouse.row))
                    .unwrap_or(false);
                let item_clicked = menu_rect.is_some_and(|rect| {
                    mouse.row > rect.y
                        && mouse.row < rect.bottom().saturating_sub(1)
                        && mouse.column > rect.x
                        && mouse.column < rect.right().saturating_sub(1)
                });
                if menu_left_click && inside && item_clicked {
                    if let Some(rect) = menu_rect
                        && let Some(menu) = self.context_menu.as_mut()
                    {
                        let item_row = mouse.row.saturating_sub(rect.y.saturating_add(1)) as usize;
                        if let Some(action_index) = menu.action_index_at_content_row(item_row) {
                            menu.selected = action_index;
                            self.execute_context_menu_action();
                        }
                    }
                } else {
                    self.context_menu = None;
                }
            }
            return false;
        }
        false
    }

    /// Right-click menu launchers (commit row, commit info, selection,
    /// paste targets).
    fn mouse_right_click_menus(&mut self, mouse: MouseEvent) -> Option<bool> {
        let right_click = mouse.kind == MouseEventKind::Down(MouseButton::Right);
        if right_click
            && matches!(self.mode(), Mode::Normal)
            && let Some(commit_idx) = self.commit_list_commit_at(mouse.column, mouse.row)
        {
            self.select_commit_idx(commit_idx);
            self.commit_diff_scroll = 0;
            self.commit_diff_info_scroll = 0;
            self.schedule_selected_stats_load();
            // Same menu as the keyboard path (`.`): ONE source for the
            // commit-row actions, so the two menus can never drift apart.
            let actions = self.commit_list_context_actions();
            self.open_context_menu(mouse.column, mouse.row, actions);
            return Some(false);
        }

        if right_click && self.is_commit_context_position(mouse.column, mouse.row) {
            self.open_context_menu(mouse.column, mouse.row, Self::commit_copy_context_actions());
            return Some(false);
        }

        if right_click
            && self
                .text_selection
                .as_ref()
                .is_some_and(|selection| !selection.text.trim().is_empty())
        {
            self.open_context_menu(mouse.column, mouse.row, Vec::new());
            return Some(false);
        }

        if right_click && self.can_paste_here() {
            self.open_context_menu(
                mouse.column,
                mouse.row,
                vec![ContextMenuAction::PasteClipboard],
            );
            return Some(false);
        }
        None
    }

    /// Pane resizing: press on a border starts a drag, drag moves the split,
    /// release ends it. None = not a resize event, fall through.
    fn mouse_resize(&mut self, mouse: MouseEvent) -> Option<bool> {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let target = self.resize_target_at(mouse.column, mouse.row)?;
                self.resize_drag = Some(target);
                Some(false)
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let target = self.resize_drag?;
                self.apply_resize(target, mouse.column, mouse.row);
                Some(false)
            }
            MouseEventKind::Up(MouseButton::Left) if self.resize_drag.is_some() => {
                self.resize_drag = None;
                tracing::info!(
                    "panes resized: list {}%, info {}%, files {}%, details {} rows",
                    self.splits.list_pct,
                    self.splits.diff_info_pct,
                    self.splits.files_pct,
                    self.splits.details_height
                );
                Some(false)
            }
            _ => None,
        }
    }

    /// The border under the cursor, if any (±1 cell on the shared edge).
    fn resize_target_at(&self, column: u16, row: u16) -> Option<ResizeTarget> {
        match self.mode() {
            Mode::Normal => {
                if self.show_commit_diff {
                    let (_, diff_rect) = self.commit_list_regions()?;
                    if row >= diff_rect.y
                        && row < diff_rect.bottom()
                        && (column == diff_rect.x || column + 1 == diff_rect.x)
                    {
                        return Some(ResizeTarget::ListDiff);
                    }
                    // The info pane's bottom edge runs over the Files column
                    // and the patch pane alike.
                    if let Some((_, patch_rect)) = self.commit_diff_regions()
                        && column >= diff_rect.x
                        && column < diff_rect.right()
                        && (row == patch_rect.y || row + 1 == patch_rect.y)
                    {
                        return Some(ResizeTarget::InfoPatch);
                    }
                    if let Some(files_rect) = self.commit_files_region()
                        && row > files_rect.y
                        && row < files_rect.bottom()
                        && (column == files_rect.right() || column + 1 == files_rect.right())
                    {
                        return Some(ResizeTarget::FilesPatch);
                    }
                }
                let details = self.commit_details_region()?;
                (row == details.y).then_some(ResizeTarget::DetailsTop)
            }
            Mode::Staging => {
                let (_, diff_rect, _) = self.staging_regions()?;
                (row >= diff_rect.y
                    && row < diff_rect.bottom()
                    && (column == diff_rect.x || column + 1 == diff_rect.x))
                    .then_some(ResizeTarget::ListDiff)
            }
            _ => None,
        }
    }

    /// Move the dragged split to the cursor. Stored as percentages of the
    /// live band, so the layout restores sensibly at any terminal size.
    fn apply_resize(&mut self, target: ResizeTarget, column: u16, row: u16) {
        let Some(area) = Self::screen_area() else {
            return;
        };
        match target {
            ResizeTarget::ListDiff => {
                if area.width > 0 {
                    self.splits.list_pct = (u32::from(column) * 100 / u32::from(area.width)) as u16;
                }
            }
            ResizeTarget::InfoPatch => {
                // The diff column spans the same rows as the list pane.
                if let Some((list_rect, _)) = self.commit_list_regions()
                    && list_rect.height > 0
                {
                    self.splits.diff_info_pct = (u32::from(row.saturating_sub(list_rect.y)) * 100
                        / u32::from(list_rect.height))
                        as u16;
                }
            }
            ResizeTarget::FilesPatch => {
                if let Some((_, diff_rect)) = self.commit_list_regions()
                    && diff_rect.width > 0
                {
                    self.splits.files_pct = (u32::from(column.saturating_sub(diff_rect.x)) * 100
                        / u32::from(diff_rect.width))
                        as u16;
                }
            }
            ResizeTarget::DetailsTop => {
                // Details footer runs down to the status bar (last row).
                self.splits.details_height = area.height.saturating_sub(1).saturating_sub(row);
            }
        }
        self.splits = self.splits.clamped();
    }

    /// Text selection: double-click word/line/panel ladder, drag selection.
    /// None = not handled here, fall through to mode dispatch.
    fn mouse_text_selection(&mut self, mouse: MouseEvent) -> Option<bool> {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left)
                if matches!(self.mode(), Mode::Normal | Mode::Staging) =>
            {
                let now = Instant::now();
                let double_click = self.last_left_click.is_some_and(|(last, column, row)| {
                    now.duration_since(last) <= DOUBLE_CLICK_INTERVAL
                        && row == mouse.row
                        && column.abs_diff(mouse.column) <= 1
                });
                self.last_left_click = Some((now, mouse.column, mouse.row));
                let selection_contains_click = self.selection_contains(mouse.column, mouse.row);
                if double_click && selection_contains_click {
                    let select_all_text = self.text_selection.as_ref().and_then(|selection| {
                        match selection.word_selection_level {
                            3 => self.text_selection_panel_text_from(selection),
                            4 => self.text_selection_panel_text(mouse.column, mouse.row),
                            _ => None,
                        }
                    });
                    if let Some(selection) = self.text_selection.as_mut() {
                        match selection.word_selection_level {
                            1 => {
                                selection.expand_next_word = true;
                                selection.word_selection_level = 2;
                            }
                            2 => {
                                selection.extend_to_row_end = true;
                                selection.word_selection_level = 3;
                            }
                            3 => {
                                selection.extend_to_panel_end = true;
                                selection.all_panel_text = select_all_text;
                                selection.word_selection_level = 4;
                            }
                            4 => {
                                selection.select_all_panel = true;
                                selection.all_panel_text = select_all_text;
                                selection.word_selection_level = 5;
                            }
                            _ => {}
                        }
                    }
                    self.text_selection_drag_start = None;
                    return Some(false);
                }

                if double_click {
                    let (panel_start_column, panel_start_row, panel_end_column, panel_end_row) =
                        self.text_selection_panel_bounds(mouse.column, mouse.row)
                            .unwrap_or((mouse.column, mouse.row, mouse.column, mouse.row));
                    self.text_selection = Some(TextSelection {
                        start_column: mouse.column,
                        start_row: mouse.row,
                        end_column: mouse.column,
                        end_row: mouse.row,
                        text: String::new(),
                        select_word: true,
                        expand_next_word: false,
                        extend_to_row_end: false,
                        extend_to_panel_end: false,
                        select_all_panel: false,
                        word_selection_level: 1,
                        panel_start_column,
                        panel_start_row,
                        panel_end_column,
                        panel_end_row,
                        all_panel_text: None,
                    });
                    self.text_selection_drag_start = None;
                    return Some(false);
                }

                if selection_contains_click {
                    self.text_selection_drag_start = None;
                    return Some(false);
                }

                self.text_selection = None;
                self.text_selection_drag_start = Some((mouse.column, mouse.row));
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some((start_column, start_row)) = self.text_selection_drag_start
                    && (start_column != mouse.column || start_row != mouse.row)
                {
                    let (panel_start_column, panel_start_row, panel_end_column, panel_end_row) =
                        self.text_selection_panel_bounds(start_column, start_row)
                            .unwrap_or((start_column, start_row, start_column, start_row));
                    let selection = self.text_selection.get_or_insert_with(|| TextSelection {
                        start_column,
                        start_row,
                        end_column: mouse.column,
                        end_row: mouse.row,
                        text: String::new(),
                        select_word: false,
                        expand_next_word: false,
                        extend_to_row_end: false,
                        extend_to_panel_end: false,
                        select_all_panel: false,
                        word_selection_level: 0,
                        panel_start_column,
                        panel_start_row,
                        panel_end_column,
                        panel_end_row,
                        all_panel_text: None,
                    });
                    selection.end_column = mouse.column;
                    selection.end_row = mouse.row;
                    return Some(false);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.text_selection_drag_start.take().is_some()
                    && let Some(selection) = self.text_selection.as_mut()
                {
                    selection.end_column = mouse.column;
                    selection.end_row = mouse.row;
                    return Some(false);
                }
            }
            _ => {}
        }
        None
    }

    /// Mode-aware scroll + click dispatch (list, staging, editors, diff).
    fn mouse_scroll_and_click(&mut self, mouse: MouseEvent) -> bool {
        let up = mouse.kind == MouseEventKind::ScrollUp;
        let down = mouse.kind == MouseEventKind::ScrollDown;
        // Clone to drop the borrow on the navigator before mutating other fields.
        match self.mode().clone() {
            Mode::Debugger => {
                if up {
                    self.event_bus.scroll_up();
                } else if down {
                    self.event_bus.scroll_down();
                }
            }
            Mode::Staging | Mode::EditCommitMessage => self.mouse_in_staging(mouse),
            Mode::Stash => self.mouse_in_stash(mouse),
            Mode::SquashMessage => {
                if (up || down)
                    && let Some(s) = self.squash_msg.as_mut()
                {
                    if up {
                        s.editor.move_up();
                    } else {
                        s.editor.move_down();
                    }
                }
            }
            Mode::EditCommitField => {
                if (up || down)
                    && let Some(e) = self.commit_msg_editor.as_mut()
                {
                    if up {
                        e.move_up();
                    } else {
                        e.move_down();
                    }
                }
            }
            _ => self.mouse_in_list(mouse),
        }
        false
    }

    /// Stash screen: click focuses a pane / selects a stash; wheel scrolls
    /// the pane under the cursor.
    fn mouse_in_stash(&mut self, mouse: MouseEvent) {
        let delta: isize = match mouse.kind {
            MouseEventKind::ScrollUp => -1,
            MouseEventKind::ScrollDown => 1,
            MouseEventKind::Down(_) => 0,
            _ => return,
        };
        let Some((list_rect, diff_rect, _)) = self.stash_regions() else {
            return;
        };
        let drawn = self.drawn_scroll.stash_diff;
        let Some(stash) = self.stash.as_mut() else {
            return;
        };
        let row = stash.list_state.selected().unwrap_or(0);
        if Self::rect_contains(diff_rect, mouse.column, mouse.row) {
            stash.diff_focused = delta == 0 || stash.diff_focused;
            if delta != 0 {
                stash.diff_scroll = crate::app::layout::wheel_target(
                    stash.diff_scroll,
                    drawn,
                    delta < 0,
                    diff_rect,
                );
                stash.scroll_diff(0); // stops at the end of the patch
            }
        } else if Self::rect_contains(list_rect, mouse.column, mouse.row) {
            let target = if delta == 0 {
                stash.diff_focused = false;
                // Inside the border: first list row is list_rect.y + 1.
                match mouse.row.checked_sub(list_rect.y + 1) {
                    Some(r) if mouse.row + 1 < list_rect.bottom() => {
                        stash.list_state.offset() + usize::from(r)
                    }
                    _ => return,
                }
            } else {
                row.saturating_add_signed(delta)
            };
            if target != row && target < stash.entries.len() {
                stash.select(&self.repo_path, target);
            }
        }
    }

    /// Staging screen: pane focus clicks + per-pane scrolling.
    fn mouse_in_staging(&mut self, mouse: MouseEvent) {
        let up = mouse.kind == MouseEventKind::ScrollUp;
        let down = mouse.kind == MouseEventKind::ScrollDown;
        let click = matches!(mouse.kind, MouseEventKind::Down(_));
        let staging_regions = self.staging_regions();
        let drawn = self.drawn_scroll.staging_diff;
        if let Some(s) = self.staging.as_mut()
            && let Some((files_rect, diff_rect, _)) = staging_regions
        {
            if click {
                if Self::rect_contains(files_rect, mouse.column, mouse.row) {
                    s.focus = crate::app::features::staging::StagingFocus::FileList;
                } else if Self::rect_contains(diff_rect, mouse.column, mouse.row) {
                    s.focus = crate::app::features::staging::StagingFocus::Diff;
                }
            }
            if up || down {
                if Self::rect_contains(diff_rect, mouse.column, mouse.row) {
                    s.diff_scroll =
                        crate::app::layout::wheel_target(s.diff_scroll, drawn, up, diff_rect);
                } else if Self::rect_contains(files_rect, mouse.column, mouse.row) {
                    if up {
                        let prev = match s.list_state.selected() {
                            Some(0) | None => 0,
                            Some(i) => i - 1,
                        };
                        s.list_state.select(Some(prev));
                        s.refresh_diff(&self.repo_path);
                    } else {
                        let len = s.files.len();
                        if len > 0 {
                            let next = match s.list_state.selected() {
                                Some(i) if i + 1 < len => i + 1,
                                Some(i) => i,
                                None => 0,
                            };
                            s.list_state.select(Some(next));
                            s.refresh_diff(&self.repo_path);
                        }
                    }
                }
            }
        }
    }

    /// Commit list + diff panels: focus clicks, file jumps, row select,
    /// pane-aware scrolling.
    fn mouse_in_list(&mut self, mouse: MouseEvent) {
        let up = mouse.kind == MouseEventKind::ScrollUp;
        let down = mouse.kind == MouseEventKind::ScrollDown;
        let click = matches!(mouse.kind, MouseEventKind::Down(_));
        let left_click = mouse.kind == MouseEventKind::Down(MouseButton::Left);
        if let Some((list_rect, diff_rect)) = self.commit_list_regions() {
            let diff_sections = self.commit_diff_regions();
            // Drill breadcrumb on the list's title row: hashes jump back
            // to that level, "…" pagers scroll the crumb window.
            if left_click && let Some(hit) = self.breadcrumb_click(mouse.column, mouse.row) {
                use crate::app::layout::BreadcrumbHit;
                match hit {
                    BreadcrumbHit::Root => self.drill_back_to(0),
                    BreadcrumbHit::Segment(level) => self.drill_back_to(level + 1),
                    BreadcrumbHit::PagePrev => {
                        let layout = self.breadcrumb(list_rect.width);
                        self.breadcrumb_start = Some(layout.prev_start);
                    }
                    BreadcrumbHit::PageNext => {
                        let layout = self.breadcrumb(list_rect.width);
                        self.breadcrumb_start = Some(layout.next_start);
                    }
                }
                return;
            }
            let files_rect = self.commit_files_region();
            let over_files =
                files_rect.is_some_and(|rect| Self::rect_contains(rect, mouse.column, mouse.row));
            if let Some(rect) = files_rect
                && over_files
            {
                if click {
                    self.commit_list_focus = CommitListFocus::Files;
                    // Inside the border: first file row is rect.y + 1.
                    if let Some(r) = mouse.row.checked_sub(rect.y + 1)
                        && mouse.row + 1 < rect.bottom()
                    {
                        self.select_files_row(self.diff_files_state.offset() + usize::from(r));
                    }
                } else if up || down {
                    // Like the commit list: the wheel moves the pick.
                    self.move_files_row(if up { -1 } else { 1 });
                }
                return;
            }
            if click && self.show_commit_diff {
                if Self::rect_contains(list_rect, mouse.column, mouse.row) {
                    self.commit_list_focus = CommitListFocus::List;
                } else if diff_sections
                    .map(|(info_rect, _)| Self::rect_contains(info_rect, mouse.column, mouse.row))
                    == Some(true)
                {
                    self.commit_list_focus = CommitListFocus::DiffInfo;
                } else if Self::rect_contains(diff_rect, mouse.column, mouse.row) {
                    self.commit_list_focus = CommitListFocus::Diff;
                }
            }
            if click && let Some(visible_idx) = self.commit_list_row_at(mouse.column, mouse.row) {
                self.list_state.select(Some(visible_idx));
                self.commit_diff_scroll = 0;
                self.commit_diff_info_scroll = 0;
                self.schedule_selected_stats_load();
                return;
            }
            if (up || down || click)
                && self.working_tree_selected()
                && let Some(details) = self.commit_details_region()
                && Self::rect_contains(details, mouse.column, mouse.row)
            {
                if click {
                    self.commit_list_focus = CommitListFocus::Details;
                } else {
                    self.working_tree.details_scroll = crate::app::layout::wheel_target(
                        self.working_tree.details_scroll,
                        self.drawn_scroll.details,
                        up,
                        details,
                    );
                    self.scroll_working_tree_details(0); // stops at the last file
                }
                return;
            }
            if up || down {
                if let Some((info_rect, _)) = diff_sections
                    && Self::rect_contains(info_rect, mouse.column, mouse.row)
                {
                    let previous = self.commit_diff_info_scroll;
                    self.commit_diff_info_scroll = crate::app::layout::wheel_target(
                        self.commit_diff_info_scroll,
                        self.drawn_scroll.info,
                        up,
                        info_rect,
                    );
                    self.preserve_text_selection_on_scroll(
                        info_rect,
                        !up,
                        previous.abs_diff(self.commit_diff_info_scroll),
                    );
                    return;
                } else if self.show_commit_diff
                    && diff_sections
                        .map(|(_, patch_rect)| {
                            Self::rect_contains(patch_rect, mouse.column, mouse.row)
                        })
                        .unwrap_or(false)
                {
                    let previous = self.commit_diff_scroll;
                    if let Some((_, diff_rect)) = diff_sections {
                        self.commit_diff_scroll = crate::app::layout::wheel_target(
                            self.commit_diff_scroll,
                            self.drawn_scroll.diff,
                            up,
                            diff_rect,
                        );
                        self.preserve_text_selection_on_scroll(
                            diff_rect,
                            !up,
                            previous.abs_diff(self.commit_diff_scroll),
                        );
                    }
                    return;
                }
                if Self::rect_contains(list_rect, mouse.column, mouse.row) {
                    if up {
                        self.move_up();
                        self.commit_diff_scroll = 0;
                        self.commit_diff_info_scroll = 0;
                    } else {
                        self.move_down();
                        self.commit_diff_scroll = 0;
                        self.commit_diff_info_scroll = 0;
                    }
                    self.schedule_selected_stats_load();
                }
            }
        } else if up || down {
            if up {
                self.move_up();
            } else {
                self.move_down();
            }
            self.schedule_selected_stats_load();
        }
    }
}
