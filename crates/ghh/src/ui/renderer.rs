use crate::app::{App, ConfirmAction, Mode};
use crate::ui::popups;
use ratatui::{Frame, layout::Rect, style::Color};

fn render_text_selection(frame: &mut Frame, app: &mut App) {
    let Some(selection) = app.text_selection.as_mut() else {
        return;
    };
    let area = frame.area();
    let buffer = frame.buffer_mut();
    let panel_start_column = selection.panel_start_column.max(area.x);
    let panel_start_row = selection.panel_start_row.max(area.y);
    let panel_end_column = selection
        .panel_end_column
        .min(area.right().saturating_sub(1));
    let panel_end_row = selection.panel_end_row.min(area.bottom().saturating_sub(1));
    if selection.select_word {
        let row = selection.start_row.max(panel_start_row).min(panel_end_row);
        let column = selection
            .start_column
            .max(panel_start_column)
            .min(panel_end_column);
        if let Some((start, end)) = crate::ui::helpers::word_bounds_at(
            buffer,
            panel_start_column,
            panel_end_column,
            column,
            row,
        ) {
            selection.start_column = start;
            selection.end_column = end;
            selection.start_row = row;
            selection.end_row = row;
        }
        selection.select_word = false;
    }
    if selection.expand_next_word {
        let row = selection.end_row;
        if let Some(end) = crate::ui::helpers::next_word_end(
            buffer,
            panel_end_column,
            row,
            selection.end_column,
        ) {
            selection.end_column = end;
            selection.end_row = row;
        }
        selection.expand_next_word = false;
    }
    if selection.extend_to_row_end {
        selection.end_column = panel_end_column;
        selection.end_row = selection.start_row;
        selection.extend_to_row_end = false;
    }
    if selection.extend_to_panel_end {
        selection.end_column = panel_end_column;
        selection.end_row = panel_end_row;
        selection.extend_to_panel_end = false;
    }
    if selection.select_all_panel {
        selection.start_column = panel_start_column;
        selection.start_row = panel_start_row;
        selection.end_column = panel_end_column;
        selection.end_row = panel_end_row;
        selection.select_all_panel = false;
    }
    let (start_row, start_column, end_row, end_column) =
        if (selection.start_row, selection.start_column)
            <= (selection.end_row, selection.end_column)
        {
            (
                selection.start_row,
                selection.start_column,
                selection.end_row,
                selection.end_column,
            )
        } else {
            (
                selection.end_row,
                selection.end_column,
                selection.start_row,
                selection.start_column,
            )
        };
    let start_row = start_row.max(panel_start_row).min(panel_end_row);
    let end_row = end_row.max(panel_start_row).min(panel_end_row);
    let mut lines = Vec::new();
    for row in start_row..=end_row {
        let left = if row == start_row {
            start_column.max(panel_start_column)
        } else {
            panel_start_column
        };
        let right = if row == end_row {
            end_column.min(panel_end_column)
        } else {
            panel_end_column
        };
        if left > right {
            continue;
        }
        let mut line = String::new();
        for column in left..=right {
            if let Some(cell) = buffer.cell_mut((column, row)) {
                line.push_str(cell.symbol());
                cell.set_bg(Color::DarkGray).set_fg(Color::White);
            }
        }
        lines.push(line.trim_end().to_string());
    }
    selection.text = selection
        .all_panel_text
        .clone()
        .unwrap_or_else(|| lines.join("\n"));
}

fn render_modal_overlay(frame: &mut Frame, popup: Option<Rect>) {
    let area = frame.area();
    let buffer = frame.buffer_mut();
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if popup
                .map(|rect| rect.contains((x, y).into()))
                .unwrap_or(false)
            {
                continue;
            }
            if let Some(cell) = buffer.cell_mut((x, y)) {
                // Indexed, not Rgb: Terminal.app (pre-macOS 26) drops truecolor.
                cell.set_fg(Color::Indexed(245)).set_bg(Color::Indexed(233));
            }
        }
    }
}

pub fn render(frame: &mut Frame, app: &mut App) {
    // Debugger overlay takes the whole screen.
    if matches!(app.mode(), Mode::Debugger) {
        super::screens::render_debugger(frame, app);
        return;
    }

    if matches!(app.mode(), Mode::Search) {
        super::screens::render_normal(frame, app, frame.area(), false);
        render_text_selection(frame, app);
        if app.context_menu.is_none() {
            render_modal_overlay(frame, app.modal_popup_rect());
        }
        // This branch returns early, so it must draw the help overlay itself
        // (the [ Help ] button sets show_help while the popup is open).
        if app.show_help {
            popups::render_help_popup(frame, app.help_title(), &app.help_lines(), frame.area());
        } else {
            popups::render_search_popup(frame, app, frame.area());
        }
        if let Some(menu_area) = app.context_menu_rect()
            && let Some(menu) = app.context_menu.as_ref()
        {
            popups::render_context_menu(frame, menu_area, menu);
        }
        return;
    }

    let area = frame.area();

    let under_staging = matches!(
        &app.mode(),
        Mode::Staging
            | Mode::Confirm(ConfirmAction::Commit { .. }, _)
            | Mode::Confirm(ConfirmAction::DiscardStagingMessageEditor, _)
            | Mode::EditCommitMessage
    );
    let under_squash_confirm =
        matches!(&app.mode(), Mode::Confirm(ConfirmAction::Squash { .. }, _));
    let squash_msg_active = matches!(app.mode(), Mode::SquashMessage)
        || matches!(&app.mode(), Mode::Confirm(ConfirmAction::Squash { .. }, _));

    let under_stash = matches!(
        &app.mode(),
        Mode::Stash
            | Mode::Confirm(ConfirmAction::StashPop { .. } | ConfirmAction::StashDrop { .. }, _)
    );

    if under_stash {
        super::screens::render_stash(frame, app);
    } else if under_staging {
        super::screens::render_staging(frame, app);
    } else {
        super::screens::render_normal(
            frame,
            app,
            area,
            under_squash_confirm || matches!(app.mode(), Mode::Squash) || squash_msg_active,
        );
    }

    render_text_selection(frame, app);

    if app.popup_visible() && app.context_menu.is_none() {
        render_modal_overlay(frame, app.modal_popup_rect());
    }

    let render_mode_popup = !app.show_help;

    if render_mode_popup
        && matches!(app.mode(), Mode::SquashMessage)
        && let Some(sm) = &app.squash_msg
    {
        popups::render_multiline_editor_popup(
            frame,
            &sm.editor,
            "Edit Squash Message",
            area,
            true,
            false,
            app.multiline_editor_changed(),
        );
    }

    if render_mode_popup
        && matches!(app.mode(), Mode::EditCommitMessage)
        && let Some(editor) = &app.commit_msg_editor
    {
        popups::render_multiline_editor_popup(
            frame,
            editor,
            "Edit Commit Message",
            area,
            app.editor_save_selected,
            true,
            app.multiline_editor_changed(),
        );
    }

    if render_mode_popup && let Mode::Confirm(action, yes_selected) = &app.mode() {
        super::popups::render_confirm_popup(frame, action, *yes_selected, area);
    }

    if render_mode_popup
        && matches!(app.mode(), Mode::Edit)
        && let Some(edit) = &app.edit
    {
        popups::render_edit_popup(frame, edit, area);
    }

    if render_mode_popup
        && matches!(app.mode(), Mode::EditCommitField)
        && let Some(editor) = &app.commit_msg_editor
    {
        popups::render_multiline_editor_popup(
            frame,
            editor,
            "Edit Message",
            area,
            app.editor_save_selected,
            true,
            app.multiline_editor_changed(),
        );
    }

    if app.show_help && app.context_menu.is_none() {
        popups::render_help_popup(frame, app.help_title(), &app.help_lines(), area);
    }

    if let Some(menu_area) = app.context_menu_rect()
        && let Some(menu) = app.context_menu.as_ref()
    {
        popups::render_context_menu(frame, menu_area, menu);
    }
}
