//! Mouse text selection: panel bounds, extracted text and scroll behaviour.

use crate::app::App;
use crate::app::models::{Mode, TextSelection, VisibleRow};
use ratatui::layout::Rect;

impl App {
    pub(crate) fn text_selection_panel_bounds(
        &self,
        column: u16,
        row: u16,
    ) -> Option<(u16, u16, u16, u16)> {
        let mut regions = Vec::new();
        match self.mode() {
            Mode::Normal => {
                if let Some((list, _)) = self.commit_list_regions() {
                    regions.push(list);
                }
                if let Some((info, diff)) = self.commit_diff_regions() {
                    regions.extend([info, diff]);
                }
                if let Some(details) = self.commit_details_region() {
                    regions.push(details);
                }
            }
            Mode::Staging => {
                if let Some((files, diff, message)) = self.staging_regions() {
                    regions.extend([files, diff, message]);
                }
            }
            _ => {}
        }
        let region = regions
            .into_iter()
            .find(|region| Self::rect_contains(*region, column, row))
            .or_else(Self::screen_area)?;
        let left = region.x.saturating_add(1);
        let top = region.y.saturating_add(1);
        let right = region.right().saturating_sub(2).max(left);
        let bottom = region.bottom().saturating_sub(2).max(top);
        Some((left, top, right, bottom))
    }

    pub(crate) fn selection_contains(&self, column: u16, row: u16) -> bool {
        self.text_selection.as_ref().is_some_and(|selection| {
            let start = (selection.start_row, selection.start_column);
            let end = (selection.end_row, selection.end_column);
            let (start, end) = if start <= end {
                (start, end)
            } else {
                (end, start)
            };
            (row, column) >= start && (row, column) <= end
        })
    }

    pub(crate) fn clear_text_selection(&mut self) {
        self.text_selection = None;
        self.text_selection_drag_start = None;
        self.last_left_click = None;
    }

    pub(crate) fn preserve_text_selection_on_scroll(
        &mut self,
        pane: Rect,
        _scroll_down: bool,
        _amount: usize,
    ) {
        let pane_start_column = pane.x.saturating_add(1);
        let pane_start_row = pane.y.saturating_add(1);
        let pane_end_column = pane.right().saturating_sub(2).max(pane_start_column);
        let pane_end_row = pane.bottom().saturating_sub(2).max(pane_start_row);
        let selection_belongs_to_pane = self.text_selection.as_ref().is_some_and(|selection| {
            (selection.panel_start_column, selection.panel_start_row)
                == (pane_start_column, pane_start_row)
                && (selection.panel_end_column, selection.panel_end_row)
                    == (pane_end_column, pane_end_row)
        });
        if selection_belongs_to_pane {
            self.clear_text_selection();
        }
    }

    pub(crate) fn text_selection_panel_text(&self, column: u16, row: u16) -> Option<String> {
        match self.mode() {
            Mode::Normal => {
                if let Some((info, diff)) = self.commit_diff_regions() {
                    if Self::rect_contains(diff, column, row) {
                        return self.selected_commit().and_then(|commit| {
                            self.commit_diff_cache
                                .get(&commit.hash)
                                .map(|lines| lines.join("\n"))
                        });
                    }
                    if Self::rect_contains(info, column, row) {
                        let commit = self.selected_commit()?;
                        let stats = self.stats_cache.get(&commit.hash);
                        let files = self
                            .commit_diff_file_cache
                            .get(&commit.hash)
                            .map(Vec::as_slice)
                            .unwrap_or_default();
                        let author = if commit.author_email.is_empty() {
                            commit.author_name.clone()
                        } else {
                            format!("{} <{}>", commit.author_name, commit.author_email)
                        };
                        let message = commit.full_message.as_deref().unwrap_or(&commit.message);
                        return Some(format!(
                            "Hash: {}\nAuthor: {}\nDate: {}\n{}\n\nMessage:\n{}\n\nFiles:\n{}",
                            commit.hash,
                            author,
                            commit.date_display_text(),
                            Self::format_change_summary(stats),
                            message,
                            Self::format_file_entries(files),
                        ));
                    }
                }
                if self
                    .commit_details_region()
                    .is_some_and(|details| Self::rect_contains(details, column, row))
                {
                    let commit = self.selected_commit()?;
                    let author = if commit.author_email.is_empty() {
                        commit.author_name.clone()
                    } else {
                        format!("{} <{}>", commit.author_name, commit.author_email)
                    };
                    return Some(format!(
                        "Hash: {}\nAuthor: {}\nDate: {}\nMessage: {}\n{}",
                        commit.hash,
                        author,
                        commit.date_display_text(),
                        commit.message,
                        Self::format_change_summary(self.stats_cache.get(&commit.hash)),
                    ));
                }
                self.commit_list_regions().and_then(|(list, _)| {
                    Self::rect_contains(list, column, row).then(|| {
                        self.visible
                            .iter()
                            .map(|row| match row {
                                VisibleRow::Commit { idx, .. }
                                | VisibleRow::DrillHeader { idx, .. } => self
                                    .commits
                                    .get(*idx)
                                    .map(|commit| {
                                        format!(
                                            "{} {} {} {}",
                                            commit.short_hash(),
                                            commit.date_display_text(),
                                            commit.author_name,
                                            commit.message
                                        )
                                    })
                                    .unwrap_or_default(),
                                VisibleRow::CollapsedBranch { count, .. } => {
                                    format!("... {count} commits hidden")
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                })
            }
            Mode::Staging => {
                let (files, diff, message) = self.staging_regions()?;
                let staging = self.staging.as_ref()?;
                if Self::rect_contains(diff, column, row) {
                    Some(staging.diff_lines.join("\n"))
                } else if Self::rect_contains(files, column, row) {
                    Some(
                        staging
                            .files
                            .iter()
                            .map(|file| format!("[{}] {}", file.status_char, file.path))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                } else if Self::rect_contains(message, column, row) {
                    Some(staging.commit_msg.clone())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub(crate) fn text_selection_panel_line_index(&self, column: u16, row: u16) -> usize {
        match self.mode() {
            Mode::Normal => {
                if let Some((info, diff)) = self.commit_diff_regions() {
                    if Self::rect_contains(diff, column, row) {
                        return self.commit_diff_scroll
                            + usize::from(row.saturating_sub(diff.y + 1));
                    }
                    if Self::rect_contains(info, column, row) {
                        return self.commit_diff_info_scroll
                            + usize::from(row.saturating_sub(info.y + 1));
                    }
                }
                if let Some((list, _)) = self.commit_list_regions()
                    && Self::rect_contains(list, column, row)
                {
                    return self.list_state.offset() + usize::from(row.saturating_sub(list.y + 1));
                }
                self.commit_details_region()
                    .filter(|details| Self::rect_contains(*details, column, row))
                    .map_or(0, |details| usize::from(row.saturating_sub(details.y + 1)))
            }
            Mode::Staging => {
                let Some((files, diff, message)) = self.staging_regions() else {
                    return 0;
                };
                let Some(staging) = self.staging.as_ref() else {
                    return 0;
                };
                if Self::rect_contains(diff, column, row) {
                    staging.diff_scroll + usize::from(row.saturating_sub(diff.y + 1))
                } else if Self::rect_contains(files, column, row) {
                    staging.list_state.offset() + usize::from(row.saturating_sub(files.y + 1))
                } else if Self::rect_contains(message, column, row) {
                    usize::from(row.saturating_sub(message.y + 1))
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    pub(crate) fn text_selection_panel_text_from(
        &self,
        selection: &TextSelection,
    ) -> Option<String> {
        let full_text =
            self.text_selection_panel_text(selection.start_column, selection.start_row)?;
        let start_line =
            self.text_selection_panel_line_index(selection.start_column, selection.start_row);
        let start_column = usize::from(
            selection
                .start_column
                .saturating_sub(selection.panel_start_column),
        );
        let mut lines: Vec<String> = full_text
            .lines()
            .skip(start_line)
            .map(String::from)
            .collect();
        if let Some(first_line) = lines.first_mut() {
            *first_line = first_line.chars().skip(start_column).collect();
        }
        Some(lines.join("\n"))
    }

    pub(crate) fn copy_selected_text(&mut self) {
        let Some(text) = self
            .text_selection
            .as_ref()
            .map(|selection| selection.text.trim_end().to_string())
        else {
            return;
        };
        if text.is_empty() {
            self.status_message = Some("No text selected.".into());
        } else if self.write_clipboard_text(&text) {
            self.status_message = Some(format!(
                "Copied {} characters to clipboard.",
                text.chars().count()
            ));
        } else {
            self.status_message = Some("Clipboard copy unavailable.".into());
        }
    }
}
