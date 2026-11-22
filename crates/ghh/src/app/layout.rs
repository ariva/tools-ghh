//! All screen-geometry math: pane regions, popup rectangles and hit tests.
//!
//! The region functions come in two layers: pure functions that take the
//! screen `area` explicitly (unit-testable without a terminal), and thin
//! `App` wrappers that feed in the live terminal size. The UI and the mouse
//! code both go through these, so they always agree on where each pane is.

use crate::app::App;
use crate::app::models::{Mode, VisibleRow};
use crate::settings::Splits;
use crossterm::terminal;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

// ── pure geometry (no App, no terminal) ──────────────────────────────────────

/// Vertical skeleton shared by the main screens:
/// title bar / content / details footer / status bar.
pub(crate) fn main_chunks(area: Rect, footer_height: u16) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(footer_height),
            Constraint::Length(1),
        ])
        .split(area)
}

/// Commit list + (optional) diff column. With the diff hidden the list takes
/// the full width and the diff rect is zero-width.
pub(crate) fn commit_list_regions_in(area: Rect, show_diff: bool, splits: Splits) -> (Rect, Rect) {
    let chunks = main_chunks(area, splits.details_height);
    let (list_pct, diff_pct) = if show_diff {
        (splits.list_pct, 100 - splits.list_pct)
    } else {
        (100, 0)
    };
    let mid = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(list_pct),
            Constraint::Percentage(diff_pct),
        ])
        .split(chunks[1]);
    (mid[0], mid[1])
}

/// Split the diff column into the file/info pane (top) and patch pane (bottom).
pub(crate) fn commit_diff_regions_in(
    area: Rect,
    show_diff: bool,
    splits: Splits,
) -> Option<(Rect, Rect)> {
    if !show_diff {
        return None;
    }
    let (_, diff) = commit_list_regions_in(area, show_diff, splits);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(splits.diff_info_pct),
            Constraint::Percentage(100 - splits.diff_info_pct),
        ])
        .split(diff);
    Some((sections[0], sections[1]))
}

/// The details footer under the commit list.
pub(crate) fn commit_details_region_in(area: Rect, splits: Splits) -> Rect {
    main_chunks(area, splits.details_height)[2]
}

/// Staging screen: file list, diff, and the commit-message input row.
pub(crate) fn staging_regions_in(area: Rect, splits: Splits) -> (Rect, Rect, Rect) {
    let chunks = main_chunks(area, 3);
    let mid = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(splits.list_pct),
            Constraint::Percentage(100 - splits.list_pct),
        ])
        .split(chunks[1]);
    (mid[0], mid[1], chunks[2])
}

/// Stash screen: stash list, patch, and the details pane (same proportions
/// as the commit list with its diff open).
pub(crate) fn stash_regions_in(area: Rect, splits: Splits) -> (Rect, Rect, Rect) {
    let chunks = main_chunks(area, splits.details_height);
    let (list, diff) = commit_list_regions_in(area, true, splits);
    (list, diff, chunks[2])
}

/// Lines a PageUp/PageDown jumps inside a bordered pane.
pub(crate) fn pane_page_step(rect: Rect) -> usize {
    usize::from(rect.height.saturating_sub(2))
        .saturating_sub(1)
        .max(1)
}

pub(crate) fn rect_contains(rect: Rect, column: u16, row: u16) -> bool {
    column >= rect.x
        && column < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

// ── App wrappers (live terminal size) ────────────────────────────────────────

impl App {
    pub(crate) fn screen_area() -> Option<Rect> {
        let (width, height) = terminal::size().ok()?;
        Some(Rect::new(0, 0, width, height))
    }

    /// Kept as an associated function so call sites can use `Self::rect_contains`.
    pub(crate) fn rect_contains(rect: Rect, column: u16, row: u16) -> bool {
        rect_contains(rect, column, row)
    }

    pub(crate) fn commit_list_regions(&self) -> Option<(Rect, Rect)> {
        Some(commit_list_regions_in(
            Self::screen_area()?,
            self.show_commit_diff,
            self.splits,
        ))
    }

    pub(crate) fn commit_diff_regions(&self) -> Option<(Rect, Rect)> {
        commit_diff_regions_in(Self::screen_area()?, self.show_commit_diff, self.splits)
    }

    pub(crate) fn commit_details_region(&self) -> Option<Rect> {
        Some(commit_details_region_in(Self::screen_area()?, self.splits))
    }

    pub(crate) fn stash_regions(&self) -> Option<(Rect, Rect, Rect)> {
        Some(stash_regions_in(Self::screen_area()?, self.splits))
    }

    pub(crate) fn staging_regions(&self) -> Option<(Rect, Rect, Rect)> {
        Some(staging_regions_in(Self::screen_area()?, self.splits))
    }

    pub(crate) fn commit_list_commit_at(&self, column: u16, row: u16) -> Option<usize> {
        let visible_idx = self.commit_list_row_at(column, row)?;
        match self.visible.get(visible_idx) {
            Some(VisibleRow::Commit { idx, .. } | VisibleRow::DrillHeader { idx, .. }) => {
                Some(*idx)
            }
            Some(VisibleRow::CollapsedBranch { .. }) | None => None,
        }
    }

    pub(crate) fn commit_diff_page_steps(&self) -> (usize, usize) {
        self.commit_diff_regions()
            .map(|(info_rect, diff_rect)| (pane_page_step(info_rect), pane_page_step(diff_rect)))
            .unwrap_or((1, 1))
    }

    /// Map a click inside the diff-info pane to the index of the file entry
    /// under the cursor, if any.
    pub(crate) fn selected_diff_file_at(
        &self,
        info_rect: Rect,
        column: u16,
        row: u16,
    ) -> Option<usize> {
        if !rect_contains(info_rect, column, row) || row <= info_rect.y {
            return None;
        }
        let commit = self.selected_commit()?;
        let files = self.commit_diff_file_cache.get(&commit.hash)?;
        let message = commit.full_message.as_deref().unwrap_or(&commit.message);
        let message_lines = message.lines().count().max(1);
        // Header block (hash/author/date/stats…) occupies 9 rows before the
        // message; files start right after the message lines.
        let first_file_line = 9 + message_lines;
        let clicked_line = self.commit_diff_info_scroll + usize::from(row - info_rect.y - 1);
        clicked_line
            .checked_sub(first_file_line)
            .filter(|index| *index < files.len())
    }

    pub fn context_menu_rect(&self) -> Option<Rect> {
        let state = self.context_menu.as_ref()?;
        let area = Self::screen_area()?;
        let width = state
            .actions
            .iter()
            .map(|action| action.label(state.clipboard_available).len() as u16 + 8)
            .max()
            .unwrap_or(20)
            .max(24)
            .min(area.width);
        let height = (state.content_line_count() as u16 + 2).min(area.height);
        let max_x = area.width.saturating_sub(width);
        let max_y = area.height.saturating_sub(height);
        Some(Rect::new(
            state.x.min(max_x),
            state.y.min(max_y),
            width,
            height,
        ))
    }

    pub fn multiline_editor_button_rects(&self) -> Option<(Rect, Rect, Rect)> {
        self.commit_msg_editor.as_ref()?;
        if !matches!(self.mode(), Mode::EditCommitMessage | Mode::EditCommitField) {
            return None;
        }
        let popup = self.multiline_editor_popup_rect()?;
        let inner = Rect {
            x: popup.x + 1,
            y: popup.y + 1,
            width: popup.width.saturating_sub(2),
            height: popup.height.saturating_sub(2),
        };
        let button_y = inner.y + inner.height.saturating_sub(1);
        let save = Rect::new(inner.x, button_y, 12, 1);
        let cancel = Rect::new(inner.x + 14, button_y, 12, 1);
        Some((popup, save, cancel))
    }

    pub fn multiline_editor_popup_rect(&self) -> Option<Rect> {
        let lines = match self.mode() {
            Mode::EditCommitMessage | Mode::EditCommitField => {
                self.commit_msg_editor.as_ref()?.lines.len()
            }
            Mode::SquashMessage => self.squash_msg.as_ref()?.editor.lines.len(),
            _ => return None,
        };
        let actions = matches!(self.mode(), Mode::EditCommitMessage | Mode::EditCommitField);
        Some(multiline_editor_popup_rect_in(
            lines,
            actions,
            Self::screen_area()?,
        ))
    }

    pub fn confirm_popup_button_rects(&self) -> Option<(Rect, Rect, Rect)> {
        let action = match self.mode() {
            Mode::Confirm(action, _) => action,
            _ => return None,
        };
        let popup = confirm_popup_rect_in(action.is_dangerous(), Self::screen_area()?);
        let button_y = popup.y
            + action.prompt_lines().len() as u16
            + if action.is_dangerous() { 4 } else { 3 };
        let yes = Rect::new(popup.x + 3, button_y, 7, 1);
        let no = Rect::new(popup.x + 13, button_y, 6, 1);
        Some((popup, yes, no))
    }

    pub fn edit_popup_rect(&self) -> Option<Rect> {
        if !matches!(self.mode(), Mode::Edit) {
            return None;
        }
        Some(edit_popup_rect_in(Self::screen_area()?))
    }

    pub fn search_popup_rect(&self) -> Option<Rect> {
        if !matches!(self.mode(), Mode::Search) {
            return None;
        }
        Some(search_popup_rect_in(Self::screen_area()?))
    }

    /// What a click inside the search popup lands on. Geometry mirrors
    /// `render_search_popup`; `search_popup_hit_in` holds the pure math so
    /// tests can pin it without a live terminal.
    pub fn search_popup_hit(&self, column: u16, row: u16) -> Option<SearchPopupHit> {
        let popup = self.search_popup_rect()?;
        // Suggestions line: clicking a candidate fills the active input. The
        // cache holds exactly what the renderer just drew.
        let inner_x = popup.x + 2;
        let inner_y = popup.y + 1;
        if row > inner_y + VISUAL_ROWS
            && row < inner_y + VISUAL_ROWS + 1 + SUGGESTION_LINES
            && column >= inner_x
            && let Some((_, _, _, suggestions)) = &self.search.suggestion_cache
            && !suggestions.is_empty()
        {
            let widths: Vec<u16> = suggestions
                .iter()
                .map(|(c, _)| c.chars().count() as u16)
                .collect();
            let inner_width = popup.width.saturating_sub(4);
            let (placed, more) =
                layout_suggestions(&widths, self.search.suggestion_page_start, inner_width);
            let line = row - (inner_y + VISUAL_ROWS + 1);
            let col = column - inner_x;
            if self.search.suggestion_page_start > 0
                && line == 0
                && (SUGGESTION_INDENT..SUGGESTION_INDENT + 3).contains(&col)
            {
                return Some(SearchPopupHit::SuggestionsPrev);
            }
            if let Some((more_line, more_x)) = more
                && line == more_line
                && col >= more_x
                && col < more_x + 3
            {
                return Some(SearchPopupHit::SuggestionsMore);
            }
            for p in placed {
                if p.line == line && col >= p.x && col < p.x + p.width {
                    return Some(SearchPopupHit::Suggestion(p.index));
                }
            }
        }
        let value_lens: [usize; 9] = std::array::from_fn(|i| self.search.values[i].chars().count());
        // [ OR ] hides while the tail segment is empty (an " OR " was just
        // appended) — two joiners in a row make no sense.
        let or_button_visible =
            crate::app::features::search::SearchState::multi_value_separator(self.search.field)
                .is_some()
                && !self.search.active_prefix().trim().is_empty();
        search_popup_hit_in(
            popup,
            self.search.reset_backup.is_some(),
            &value_lens,
            self.search.field.index(),
            self.search.cursor,
            or_button_visible,
            column,
            row,
        )
    }

    pub fn help_popup_rect(&self) -> Option<Rect> {
        if !self.show_help {
            return None;
        }
        Some(help_popup_rect_in(
            self.help_lines().len(),
            Self::screen_area()?,
        ))
    }

    pub fn popup_visible(&self) -> bool {
        self.show_help
            || self.context_menu.is_some()
            || matches!(
                self.mode(),
                Mode::Search
                    | Mode::Edit
                    | Mode::EditCommitMessage
                    | Mode::EditCommitField
                    | Mode::SquashMessage
                    | Mode::Confirm(..)
            )
    }

    pub fn modal_popup_rect(&self) -> Option<Rect> {
        if let Some(popup) = self.context_menu_rect() {
            return Some(popup);
        }
        if let Some(popup) = self.help_popup_rect() {
            return Some(popup);
        }
        match self.mode() {
            Mode::Search => self.search_popup_rect(),
            Mode::Edit => self.edit_popup_rect(),
            Mode::EditCommitMessage | Mode::EditCommitField | Mode::SquashMessage => {
                self.multiline_editor_popup_rect()
            }
            Mode::Confirm(..) => self.confirm_popup_button_rects().map(|(popup, _, _)| popup),
            _ => None,
        }
    }

    /// Suffix appended after the breadcrumb (filter indicator, non-clickable).
    pub(crate) fn list_title_suffix(&self) -> String {
        if self.search.filter_active && !self.search.matches.is_empty() {
            format!(
                "\u{25B8} filtered {} (f: remove) ",
                self.search.matches.len()
            )
        } else {
            String::new()
        }
    }

    /// Breadcrumb layout for the current drill stack in a pane of
    /// `pane_width` — shared by the renderer and mouse hit-testing.
    pub(crate) fn breadcrumb(&self, pane_width: u16) -> BreadcrumbLayout {
        let labels: Vec<String> = self
            .drill_stack
            .iter()
            .map(|frame| {
                format!(
                    "{} ({})",
                    &frame.merge_hash[..frame.merge_hash.len().min(8)],
                    frame.count
                )
            })
            .collect();
        let reserved = self.list_title_suffix().chars().count() + 2;
        breadcrumb_layout(
            &labels,
            self.breadcrumb_start,
            usize::from(pane_width).saturating_sub(reserved),
        )
    }

    /// What a click on the list's title row lands on while drilled in.
    pub(crate) fn breadcrumb_click(&self, column: u16, row: u16) -> Option<BreadcrumbHit> {
        if self.drill_stack.is_empty() {
            return None;
        }
        let (list_rect, _) = self.commit_list_regions()?;
        if row != list_rect.y {
            return None;
        }
        // Block titles start one cell after the pane's left border.
        let col = column.checked_sub(list_rect.x.saturating_add(1))?;
        breadcrumb_hit_in(&self.breadcrumb(list_rect.width), col)
    }

    pub(crate) fn commit_list_page_size(&self) -> usize {
        self.commit_list_regions()
            .map(|(list_rect, _)| pane_page_step(list_rect))
            .unwrap_or(10)
    }

    pub(crate) fn commit_list_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let (list_rect, _) = self.commit_list_regions()?;
        if !rect_contains(list_rect, column, row) || list_rect.height <= 2 {
            return None;
        }
        let content_top = list_rect.y.saturating_add(1);
        let content_bottom = list_rect
            .y
            .saturating_add(list_rect.height.saturating_sub(1));
        if row < content_top || row >= content_bottom {
            return None;
        }

        let viewport_row = usize::from(row.saturating_sub(content_top));
        let start = self.list_state.offset();
        let visible_idx = start.saturating_add(viewport_row);
        (visible_idx < self.visible.len()).then_some(visible_idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 40,
    };

    const SPLITS: Splits = Splits {
        list_pct: 40,
        diff_info_pct: 40,
        details_height: 8,
    };

    #[test]
    fn commit_list_fills_width_without_diff() {
        let (list, diff) = commit_list_regions_in(AREA, false, SPLITS);
        assert_eq!(list.width, 120);
        assert_eq!(diff.width, 0);
    }

    #[test]
    fn commit_list_splits_40_60_with_diff() {
        let (list, diff) = commit_list_regions_in(AREA, true, SPLITS);
        assert_eq!(list.width + diff.width, 120);
        assert_eq!(list.width, 48); // 40 % of 120
        assert_eq!(diff.x, list.right());
    }

    #[test]
    fn commit_list_split_follows_custom_percentage() {
        let custom = Splits {
            list_pct: 25,
            ..SPLITS
        };
        let (list, diff) = commit_list_regions_in(AREA, true, custom);
        assert_eq!(list.width, 30); // 25 % of 120
        assert_eq!(list.width + diff.width, 120);
    }

    #[test]
    fn diff_regions_stack_inside_diff_column() {
        let (info, patch) = commit_diff_regions_in(AREA, true, SPLITS).unwrap();
        assert_eq!(info.x, patch.x);
        assert_eq!(info.bottom(), patch.y);
        assert!(commit_diff_regions_in(AREA, false, SPLITS).is_none());
        // A custom info percentage moves the boundary.
        let custom = Splits {
            diff_info_pct: 70,
            ..SPLITS
        };
        let (big_info, _) = commit_diff_regions_in(AREA, true, custom).unwrap();
        assert!(big_info.height > info.height);
    }

    #[test]
    fn panes_share_the_content_row_band() {
        // List/diff and the details footer must tile the vertical space
        // between the title bar (1 row) and status bar (1 row).
        let (list, _) = commit_list_regions_in(AREA, true, SPLITS);
        let details = commit_details_region_in(AREA, SPLITS);
        assert_eq!(list.y, 1);
        assert_eq!(details.y, list.bottom());
        assert_eq!(details.height, 8);
        assert_eq!(details.bottom(), 39); // status bar takes the last row
    }

    #[test]
    fn staging_regions_tile_the_screen() {
        let (files, diff, message) = staging_regions_in(AREA, SPLITS);
        assert_eq!(files.y, 1);
        assert_eq!(files.width + diff.width, 120);
        assert_eq!(message.height, 3);
        assert_eq!(message.y, files.bottom());
    }

    #[test]
    fn page_step_accounts_for_borders_and_is_at_least_one() {
        assert_eq!(pane_page_step(Rect::new(0, 0, 10, 20)), 17);
        assert_eq!(pane_page_step(Rect::new(0, 0, 10, 3)), 1);
        assert_eq!(pane_page_step(Rect::new(0, 0, 10, 0)), 1);
    }

    #[test]
    fn breadcrumb_fits_elides_and_pages() {
        use super::{BreadcrumbHit, breadcrumb_hit_in, breadcrumb_layout};
        let labels: Vec<String> = (0..6).map(|i| format!("hash000{i} (3)")).collect();
        // Everything fits: no pagers, all segments clickable.
        let all = breadcrumb_layout(&labels, None, 200);
        assert!(all.front.is_none() && all.back.is_none());
        assert_eq!(all.segments.len(), 6);
        assert!(all.text.starts_with(" Commits \u{25B8} hash0000 (3)"));
        // Root zone hits.
        assert_eq!(breadcrumb_hit_in(&all, 3), Some(BreadcrumbHit::Root));
        let (idx, x, _) = all.segments[2];
        assert_eq!(
            breadcrumb_hit_in(&all, x),
            Some(BreadcrumbHit::Segment(idx))
        );

        // Narrow pane, auto mode: tail shown, front pager present.
        let tail = breadcrumb_layout(&labels, None, 40);
        assert!(tail.front.is_some() && tail.back.is_none());
        assert_eq!(tail.segments.last().unwrap().0, 5, "deepest level visible");
        assert!(tail.text.chars().count() <= 40);
        let front = tail.front.unwrap();
        assert_eq!(
            breadcrumb_hit_in(&tail, front.0 + 1),
            Some(BreadcrumbHit::PagePrev)
        );

        // Paged to the front: back pager appears, prev/next starts sane.
        let head = breadcrumb_layout(&labels, Some(0), 40);
        assert!(head.front.is_none() && head.back.is_some());
        assert_eq!(head.segments.first().unwrap().0, 0);
        let back = head.back.unwrap();
        assert_eq!(
            breadcrumb_hit_in(&head, back.0 + 1),
            Some(BreadcrumbHit::PageNext)
        );
        assert!(head.next_start > 0 && head.next_start < 6);
        // Middle window: both pagers.
        let mid = breadcrumb_layout(&labels, Some(2), 40);
        assert!(mid.front.is_some() && mid.back.is_some());
        assert!(mid.prev_start < 2);
    }

    #[test]
    fn rect_contains_is_edge_exclusive_on_right_and_bottom() {
        let rect = Rect::new(2, 3, 4, 5);
        assert!(rect_contains(rect, 2, 3));
        assert!(rect_contains(rect, 5, 7));
        assert!(!rect_contains(rect, 6, 3));
        assert!(!rect_contains(rect, 2, 8));
    }
}

// ── drill breadcrumb (list title row) ────────────────────────────────────────

/// Click target on the drill breadcrumb in the commit list's title row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreadcrumbHit {
    /// "Commits" root: back to the outer list.
    Root,
    /// A drill-level hash: back to that level (stack index).
    Segment(usize),
    /// Leading "…": page toward earlier levels.
    PagePrev,
    /// Trailing "…": page toward later levels.
    PageNext,
}

/// Layout of the breadcrumb: rendered text plus the click zones (x/width in
/// chars relative to the title's first char). ONE source for renderer and
/// mouse hit-testing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreadcrumbLayout {
    pub text: String,
    /// (stack index, x, width) of each shown segment label.
    pub segments: Vec<(usize, u16, u16)>,
    pub front: Option<(u16, u16)>,
    pub back: Option<(u16, u16)>,
    /// Window starts for the pagers (valid when front/back is Some).
    pub prev_start: usize,
    pub next_start: usize,
}

/// "Commits" root zone within the title text (after the leading space).
pub const BREADCRUMB_ROOT: (u16, u16) = (1, 7);

/// Compute the breadcrumb window. `requested_start` = None follows the tail
/// (deepest levels shown); Some(s) is a user-paged window. Both ends elide
/// with a clickable " ▸ …" when levels don't fit `max_width`.
pub fn breadcrumb_layout(
    labels: &[String],
    requested_start: Option<usize>,
    max_width: usize,
) -> BreadcrumbLayout {
    const SEP: usize = 3; // " ▸ "
    const MORE: usize = 1; // "…"
    let n = labels.len();
    let widths: Vec<usize> = labels.iter().map(|label| label.chars().count()).collect();
    let base = " Commits".chars().count();
    let fits_from = |s: usize| -> bool {
        let mut width = base + if s > 0 { SEP + MORE } else { 0 } + 1;
        for w in &widths[s..] {
            width += SEP + w;
        }
        width <= max_width
    };
    let start = match requested_start {
        Some(s) => s.min(n.saturating_sub(1)),
        None => (0..n)
            .find(|&s| fits_from(s))
            .unwrap_or(n.saturating_sub(1)),
    };
    // Greedy forward, reserving room for the trailing pager while more remain.
    let mut shown = 0usize;
    let mut width = base + if start > 0 { SEP + MORE } else { 0 } + 1;
    for (i, w) in widths.iter().enumerate().skip(start) {
        let reserve_back = if i + 1 < n { SEP + MORE } else { 0 };
        if width + SEP + w + reserve_back > max_width && shown > 0 {
            break;
        }
        width += SEP + w;
        shown += 1;
    }
    let shown = shown.max(usize::from(start < n));
    let end = start + shown;

    let mut text = String::from(" Commits");
    let mut segments = Vec::new();
    let mut front = None;
    let mut back = None;
    if start > 0 {
        front = Some((text.chars().count() as u16, (SEP + MORE) as u16));
        text.push_str(" \u{25B8} \u{2026}");
    }
    for (i, label) in labels.iter().enumerate().take(end).skip(start) {
        text.push_str(" \u{25B8} ");
        segments.push((i, text.chars().count() as u16, widths[i] as u16));
        text.push_str(label);
    }
    if end < n {
        back = Some((text.chars().count() as u16, (SEP + MORE) as u16));
        text.push_str(" \u{25B8} \u{2026}");
    }
    text.push(' ');
    BreadcrumbLayout {
        text,
        segments,
        front,
        back,
        prev_start: start.saturating_sub(shown.max(1)),
        next_start: end.min(n.saturating_sub(1)),
    }
}

/// Pure hit-test on a breadcrumb layout; `col` is relative to the title's
/// first char.
pub fn breadcrumb_hit_in(layout: &BreadcrumbLayout, col: u16) -> Option<BreadcrumbHit> {
    let inside = |zone: (u16, u16)| col >= zone.0 && col < zone.0 + zone.1;
    if inside(BREADCRUMB_ROOT) {
        return Some(BreadcrumbHit::Root);
    }
    if layout.front.is_some_and(inside) {
        return Some(BreadcrumbHit::PagePrev);
    }
    if layout.back.is_some_and(inside) {
        return Some(BreadcrumbHit::PageNext);
    }
    layout
        .segments
        .iter()
        .find(|(_, x, w)| inside((*x, *w)))
        .map(|(i, _, _)| BreadcrumbHit::Segment(*i))
}

/// Click target inside the search popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchPopupHit {
    /// A field row (index into `SearchField::ALL`).
    Field(usize),
    /// The clear mark at the right edge of a field row.
    ClearField(usize),
    /// Bottom button: 0 Search, 1 Cancel, 2 Reset/Undo Reset.
    Button(usize),
    /// The "Full history" checkbox row (shown when the file field has text).
    FullHistoryToggle,
    /// One option on the "File action" selector row (index into FileAction::ALL).
    FileAction(usize),
    /// A candidate on the Suggestions line (index into the current list).
    Suggestion(usize),
    /// The [OR] button on a multi-value row (Author/File/Branch/Tag).
    OrAppend(usize),
    /// The "..." pager at the end of the suggestions area: next page.
    SuggestionsMore,
    /// The "..." pager at the start of the first line: previous page.
    SuggestionsPrev,
    /// "..." at the start of a long input value: page the cursor left.
    ValueScrollLeft(usize),
    /// "..." at the end of a long input value: page the cursor right.
    ValueScrollRight(usize),
}

/// Layout of one single-field input line: where the visible text window,
/// optional "..." pagers and buttons sit. Shared by renderer and hit-testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldLineLayout {
    /// First visible char (window start) and how many chars show.
    pub window_start: usize,
    pub window_len: usize,
    pub text_x: u16,
    /// "..." zones (3 cells each), if the value overflows that side.
    pub left_more_x: Option<u16>,
    pub right_more_x: Option<u16>,
    /// Buttons; None when the value is empty.
    pub or_x: Option<u16>,
    pub clear_x: Option<u16>,
}

/// Compute the line layout. `cursor` is Some for the focused field — the
/// window follows it; unfocused long values window from 0. Buttons stay
/// inside `inner_width` ("smart": after the text while it fits, pinned at
/// the right edge once it doesn't).
pub fn layout_field_line(
    inner_width: u16,
    value_chars: usize,
    cursor: Option<usize>,
    show_or: bool,
    show_clear: bool,
    label: u16,
) -> FieldLineLayout {
    const MORE: u16 = 4; // "... " / " ..."
    let cursor_cell = u16::from(cursor.is_some());
    let buttons = if value_chars > 0 {
        (if show_clear { CLEAR_MARK_WIDTH } else { 0 })
            + if show_or { OR_BUTTON_WIDTH + 1 } else { 0 }
    } else {
        0
    };
    let natural = label + value_chars as u16 + cursor_cell + 2 + buttons;
    if natural <= inner_width {
        // Everything fits: buttons trail the text (original behavior).
        let mark = label + value_chars as u16 + cursor_cell + 2;
        return FieldLineLayout {
            window_start: 0,
            window_len: value_chars,
            text_x: label,
            left_more_x: None,
            right_more_x: None,
            or_x: (buttons > 0 && show_or).then_some(mark),
            clear_x: (buttons > 0 && show_clear)
                .then(|| mark + if show_or { OR_BUTTON_WIDTH + 1 } else { 0 }),
        };
    }
    // Overflow: pin buttons at the right edge, window the text.
    let clear_edge = if show_clear {
        inner_width.saturating_sub(CLEAR_MARK_WIDTH)
    } else {
        inner_width
    };
    let or_x = show_or.then(|| clear_edge.saturating_sub(OR_BUTTON_WIDTH + 1));
    let clear_x = show_clear.then_some(clear_edge);
    let area_end = or_x.unwrap_or(clear_edge).saturating_sub(1);
    let area = area_end.saturating_sub(label);
    // Assume both pagers first; drop the left one when windowing from 0.
    let mut shown = area.saturating_sub(2 * MORE + cursor_cell).max(1) as usize;
    let mut start = match cursor {
        Some(c) => c.saturating_sub(shown.saturating_sub(1)),
        None => 0,
    };
    if start == 0 {
        shown = area.saturating_sub(MORE + cursor_cell).max(1) as usize;
        if let Some(c) = cursor
            && c > shown.saturating_sub(1)
        {
            shown = area.saturating_sub(2 * MORE + cursor_cell).max(1) as usize;
            start = c.saturating_sub(shown.saturating_sub(1));
        }
    }
    let window_len = shown.min(value_chars - start);
    let left = (start > 0).then_some(label);
    let text_x = label + if left.is_some() { MORE } else { 0 };
    let right =
        (start + window_len < value_chars).then_some(text_x + window_len as u16 + cursor_cell + 1);
    FieldLineLayout {
        window_start: start,
        window_len,
        text_x,
        left_more_x: left,
        right_more_x: right,
        or_x,
        clear_x,
    }
}

/// Suggestion area height (lines under the fields).
pub const SUGGESTION_LINES: u16 = 3;
/// Indent of every suggestion line ("Suggestions: " on the first).
pub const SUGGESTION_INDENT: u16 = 13;

/// Start index of the page BEFORE the one starting at `current` — pages are
/// derived by replaying the greedy layout from 0, so prev/next stay
/// perfectly symmetric.
pub fn previous_page_start(widths: &[u16], inner_width: u16, current: usize) -> usize {
    let mut start = 0usize;
    loop {
        let (placed, more) = layout_suggestions(widths, start, inner_width);
        let next = placed.last().map(|p| p.index + 1).unwrap_or(widths.len());
        if next >= current || more.is_none() {
            return start;
        }
        start = next;
    }
}

/// Char position for a click at `col` (inner-relative) on a field line —
/// used to drop the cursor where the user clicked.
pub fn char_pos_at(layout: FieldLineLayout, value_chars: usize, col: u16) -> usize {
    if col <= layout.text_x {
        return layout.window_start;
    }
    (layout.window_start + (col - layout.text_x) as usize)
        .min(layout.window_start + layout.window_len)
        .min(value_chars)
}

/// One suggestion placed on the wrapped page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacedSuggestion {
    /// Index into the full candidate list.
    pub index: usize,
    /// Line 0..SUGGESTION_LINES and x within the inner area.
    pub line: u16,
    pub x: u16,
    pub width: u16,
}

/// Greedy wrap of candidates (never split mid-candidate) into up to
/// [`SUGGESTION_LINES`] lines starting at `start`. Returns placements and,
/// when more candidates remain, where the clickable "..." sits. Shared by
/// the renderer and hit-testing.
pub fn layout_suggestions(
    widths: &[u16],
    start: usize,
    inner_width: u16,
) -> (Vec<PlacedSuggestion>, Option<(u16, u16)>) {
    const GAP: u16 = 2;
    const MORE: u16 = 3; // "..."
    let mut placed = Vec::new();
    let mut line = 0u16;
    // A back-pager occupies the head of the first line on pages > 0.
    let mut x = SUGGESTION_INDENT + if start > 0 { MORE + 1 } else { 0 };
    let mut i = start.min(widths.len());
    while i < widths.len() {
        let width = widths[i].min(inner_width.saturating_sub(SUGGESTION_INDENT).max(1));
        if x + width > inner_width {
            line += 1;
            x = SUGGESTION_INDENT;
            if line >= SUGGESTION_LINES {
                break;
            }
        }
        placed.push(PlacedSuggestion {
            index: i,
            line,
            x,
            width,
        });
        x += width + GAP;
        i += 1;
    }
    if i < widths.len() {
        // Make room for "..." on the last used line.
        while let Some(last) = placed.last().copied() {
            if last.line < SUGGESTION_LINES - 1 || last.x + last.width + GAP + MORE <= inner_width {
                break;
            }
            placed.pop();
        }
        let (line, x) = placed
            .last()
            .map(|p| (p.line, p.x + p.width + GAP))
            .unwrap_or((0, SUGGESTION_INDENT));
        return (placed, Some((line, x)));
    }
    (placed, None)
}

/// Width of the [OR] button on multi-value rows.
pub const OR_BUTTON_WIDTH: u16 = 6;
/// Help popup rect for a given line count — ONE source for renderer and
/// hit-testing (same lesson as the search popup's off-by-one clicks).
pub fn help_popup_rect_in(lines: usize, area: Rect) -> Rect {
    let width = area.width * 64 / 100;
    let height = (lines as u16 + 4).max(8).min(area.height.saturating_sub(2));
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

/// Help popup [X] close mark: top-right corner of the border row.
/// Shared by renderer and mouse hit-testing.
pub fn help_close_rect(popup: Rect) -> Rect {
    Rect::new(popup.x + popup.width.saturating_sub(4), popup.y, 3, 1)
}

/// Help popup [ OK ] button: bottom row, left-aligned like every other
/// popup's buttons (search, confirm, editor).
pub fn help_ok_rect(popup: Rect) -> Rect {
    Rect::new(popup.x + 2, popup.y + popup.height.saturating_sub(2), 6, 1)
}

/// Edit popup rect — ONE source for renderer and hit-testing.
pub fn edit_popup_rect_in(area: Rect) -> Rect {
    crate::ui::helpers::centered_rect(78, 19, area)
}

/// Confirm popup rect for a given action shape — ONE source for renderer
/// and hit-testing.
pub fn confirm_popup_rect_in(dangerous: bool, area: Rect) -> Rect {
    crate::ui::helpers::centered_rect(58, if dangerous { 10 } else { 9 }, area)
}

/// Multiline editor popup rect (commit/squash message editors) — ONE source
/// for renderer and hit-testing.
pub fn multiline_editor_popup_rect_in(lines: usize, show_actions: bool, area: Rect) -> Rect {
    let height = (lines as u16 + if show_actions { 8 } else { 6 })
        .min(area.height.saturating_sub(2))
        .max(8);
    crate::ui::helpers::centered_rect(80, height, area)
}

/// Search popup height — ONE source for renderer and hit-testing; a mismatch
/// centers them differently and every click lands off by a row.
pub const SEARCH_POPUP_HEIGHT: u16 = 22;

/// The popup rect for a given screen area — the ONLY place popup geometry
/// is computed; renderer and hit-testing both call this.
pub fn search_popup_rect_in(area: Rect) -> Rect {
    crate::ui::helpers::centered_rect(82, SEARCH_POPUP_HEIGHT, area)
}

/// File-action selector labels, same order as `FileAction::ALL`.
pub const FILE_ACTION_LABELS: [&str; 5] = ["All", "Added", "Modified", "Deleted", "Renamed"];

/// (x, width) zones of the file-action options, offsets from inner left.
/// Options start at the shared value column (16), each "(Label)" wide + gap.
pub fn file_action_zones() -> [(u16, u16); 5] {
    let mut zones = [(0u16, 0u16); 5];
    let mut x = 16u16;
    for (i, label) in FILE_ACTION_LABELS.iter().enumerate() {
        let width = label.len() as u16 + 2;
        zones[i] = (x, width);
        x += width + 1;
    }
    zones
}

/// Checkbox label on the row above the popup buttons; "[x] " precedes it.
pub const FULL_HISTORY_LABEL: &str =
    "Full history file search (slower, but captures files changed in merges)";

/// Pure geometry for [`App::search_popup_hit`] — must mirror
/// `render_search_popup` (inner margin 2/1, 8 field rows, buttons at
/// fields + 4, `[ label ]` separated by three spaces).
/// Width of the red `[ clear ]` button.
pub const CLEAR_MARK_WIDTH: u16 = 9;

// ── search popup row geometry (9 logical fields on 8 visual rows) ──────────
// The two date fields share one row: `Date: <from> - <to> [ clear ]`.
pub const VISUAL_ROWS: u16 = 8;
pub const DATE_ROW: u16 = 4;
/// Cells reserved for the From value (dates are 10 chars + padding).
pub const DATE_FROM_SLOT: u16 = 18; // same as To: full date + cursor, no windowing
/// x of the ` - ` separator (relative to inner): label 16 + from slot.
pub const DATE_SEP_X: u16 = 16 + DATE_FROM_SLOT;
/// x where the To value starts: separator is 3 cells (` - `).
pub const DATE_TO_X: u16 = DATE_SEP_X + 3;
/// Cells reserved for the To value (fits the END placeholder).
pub const DATE_TO_SLOT: u16 = 18;
/// x of the date row's [ clear ] (staged: To first, then From).
pub const DATE_CLEAR_X: u16 = DATE_TO_X + DATE_TO_SLOT + 1;

/// Fields that own a whole visual row, in row order (dates excluded).
pub const SINGLE_FIELD_ROWS: [crate::app::SearchField; 7] = [
    crate::app::SearchField::Quick,
    crate::app::SearchField::Hash,
    crate::app::SearchField::Message,
    crate::app::SearchField::Author,
    crate::app::SearchField::File,
    crate::app::SearchField::Branch,
    crate::app::SearchField::Tag,
];

/// Visual row of a field (dates share DATE_ROW).
pub fn visual_row_of(field: crate::app::SearchField) -> u16 {
    use crate::app::SearchField as F;
    match field {
        F::Quick => 0,
        F::Hash => 1,
        F::Message => 2,
        F::Author => 3,
        F::DateFrom | F::DateTo => DATE_ROW,
        F::File => 5,
        F::Branch => 6,
        F::Tag => 7,
    }
}

/// Field on a visual row for hit-testing (dates resolved by column).
fn field_on_row(row: u16, column_in_row: u16) -> crate::app::SearchField {
    use crate::app::SearchField as F;
    match row {
        0 => F::Quick,
        1 => F::Hash,
        2 => F::Message,
        3 => F::Author,
        4 => {
            if column_in_row < DATE_TO_X {
                F::DateFrom
            } else {
                F::DateTo
            }
        }
        5 => F::File,
        6 => F::Branch,
        _ => F::Tag,
    }
}

// The argument list IS the popup state the renderer drew from — bundling it
// into a struct would just rename the same eight values.
#[allow(clippy::too_many_arguments)]
pub fn search_popup_hit_in(
    popup: Rect,
    has_undo_reset: bool,
    value_lens: &[usize; 9],
    active_field_index: usize,
    active_cursor: usize,
    or_button_visible: bool,
    column: u16,
    row: u16,
) -> Option<SearchPopupHit> {
    use crate::app::SearchField;
    let inner_x = popup.x + 2;
    let inner_y = popup.y + 1;
    let inner_width = popup.width.saturating_sub(4);
    if row >= inner_y
        && row < inner_y + VISUAL_ROWS
        && column >= inner_x
        && column < inner_x + inner_width
    {
        let visual = row - inner_y;
        let field = field_on_row(visual, column - inner_x);
        let index = field.index();
        if visual == DATE_ROW {
            // One [ clear ] for the whole date row, at a fixed column.
            let has_date = value_lens[SearchField::DateFrom.index()] > 0
                || value_lens[SearchField::DateTo.index()] > 0;
            let mark_x = inner_x + DATE_CLEAR_X;
            if has_date && column >= mark_x && column < mark_x + CLEAR_MARK_WIDTH {
                return Some(SearchPopupHit::ClearField(SearchField::DateFrom.index()));
            }
            return Some(SearchPopupHit::Field(index));
        }
        let active = index == active_field_index;
        let multi =
            crate::app::features::search::SearchState::multi_value_separator(field).is_some();
        let layout = layout_field_line(
            inner_width,
            value_lens[index],
            active.then_some(active_cursor),
            // [ OR ] only on the focused multi-value field with a non-empty
            // tail segment.
            multi && active && or_button_visible,
            true,
            16,
        );
        let col = column - inner_x;
        if let Some(x) = layout.left_more_x
            && col >= x
            && col < x + 3
        {
            return Some(SearchPopupHit::ValueScrollLeft(index));
        }
        if let Some(x) = layout.right_more_x
            && col >= x
            && col < x + 3
        {
            return Some(SearchPopupHit::ValueScrollRight(index));
        }
        if let Some(x) = layout.or_x
            && col >= x
            && col < x + OR_BUTTON_WIDTH
        {
            return Some(SearchPopupHit::OrAppend(index));
        }
        if let Some(x) = layout.clear_x
            && col >= x
            && col < x + CLEAR_MARK_WIDTH
        {
            return Some(SearchPopupHit::ClearField(index));
        }
        return Some(SearchPopupHit::Field(index));
    }
    if row == inner_y + VISUAL_ROWS + SUGGESTION_LINES + 2
        && value_lens[SearchField::File.index()] > 0
    {
        let col = column.checked_sub(inner_x)?;
        for (i, (x, width)) in file_action_zones().into_iter().enumerate() {
            if col >= x && col < x + width {
                return Some(SearchPopupHit::FileAction(i));
            }
        }
        return None;
    }
    if row == inner_y + VISUAL_ROWS + SUGGESTION_LINES + 3
        && value_lens[SearchField::File.index()] > 0
        && column >= inner_x
        && column < inner_x + 4 + FULL_HISTORY_LABEL.len() as u16
    {
        return Some(SearchPopupHit::FullHistoryToggle);
    }
    if row == inner_y + VISUAL_ROWS + SUGGESTION_LINES + 5 {
        let labels = [
            "Search",
            "Cancel",
            if has_undo_reset {
                "Undo Reset"
            } else {
                "Reset"
            },
            "Help",
        ];
        let mut x = inner_x;
        for (button, label) in labels.into_iter().enumerate() {
            let width = label.len() as u16 + 4; // "[ label ]"
            if column >= x && column < x + width {
                return Some(SearchPopupHit::Button(button));
            }
            x += width + 3;
        }
    }
    None
}

#[cfg(test)]
mod search_hit_tests {
    use super::{Rect, SearchPopupHit, layout_suggestions, search_popup_hit_in};

    #[test]
    fn suggestion_wrap_places_lines_and_pager() {
        // Width 40: line fits from x=13 to 40.
        let widths = vec![10u16, 10, 10, 10, 10, 10, 10, 10, 10, 10];
        let (placed, more) = layout_suggestions(&widths, 0, 40);
        assert!(placed.len() < widths.len(), "must paginate");
        assert!(more.is_some(), "pager expected");
        assert!(placed.iter().all(|p| p.line < super::SUGGESTION_LINES));
        assert_eq!(placed[0].x, super::SUGGESTION_INDENT);
        // Two 10-wide fit per line (13+10+2+10=35 <= 40); 3 lines => 6 shown.
        assert_eq!(placed.len(), 6);
        // Next page starts where this one ended.
        let next_start = placed.last().unwrap().index + 1;
        let (placed2, more2) = layout_suggestions(&widths, next_start, 40);
        assert_eq!(placed2[0].index, 6);
        assert!(placed2.len() == 4 && more2.is_none(), "last page, no pager");
    }

    #[test]
    fn back_pager_shifts_first_line_and_round_trips() {
        use super::previous_page_start;
        let widths = vec![10u16, 10, 10, 10, 10, 10, 10, 10, 10, 10];
        // Page 2 starts at 6 (see wrap test): first line indented past "...".
        let (placed, _) = super::layout_suggestions(&widths, 6, 40);
        assert_eq!(
            placed[0].x,
            super::SUGGESTION_INDENT + 4,
            "room for back pager"
        );
        // Prev from page 2 lands back at page 1's start.
        assert_eq!(previous_page_start(&widths, 40, 6), 0);
        assert_eq!(
            previous_page_start(&widths, 40, 0),
            0,
            "no page before the first"
        );
    }

    #[test]
    fn long_values_window_pin_buttons_and_page() {
        use super::{char_pos_at, layout_field_line};
        // Short value: buttons trail the text.
        let short = layout_field_line(60, 5, Some(5), false, true, 16);
        assert_eq!(short.window_start, 0);
        assert_eq!(short.clear_x, Some(16 + 5 + 1 + 2));
        assert!(short.left_more_x.is_none() && short.right_more_x.is_none());

        // Long value, cursor at end: window follows cursor, left "...",
        // buttons pinned at the right edge.
        let long = layout_field_line(60, 100, Some(100), true, true, 16);
        assert!(long.window_start > 0);
        assert!(long.left_more_x.is_some());
        assert_eq!(long.clear_x, Some(60 - super::CLEAR_MARK_WIDTH));
        assert_eq!(
            long.or_x,
            Some(60 - super::CLEAR_MARK_WIDTH - super::OR_BUTTON_WIDTH - 1)
        );
        assert!(
            long.window_start + long.window_len >= 100,
            "cursor visible at end"
        );

        // Cursor at start of a long value: no left pager, right "..." shown.
        let head = layout_field_line(60, 100, Some(0), false, true, 16);
        assert_eq!(head.window_start, 0);
        assert!(head.left_more_x.is_none());
        assert!(head.right_more_x.is_some());

        // Click mapping: text_x maps to window start, clicks land in-window.
        assert_eq!(char_pos_at(head, 100, head.text_x), 0);
        assert_eq!(char_pos_at(head, 100, head.text_x + 5), 5);
        let mid = layout_field_line(60, 100, Some(50), false, true, 16);
        assert_eq!(char_pos_at(mid, 100, mid.text_x + 3), mid.window_start + 3);
    }

    /// Repeated paging walks the whole value: cursor steps drive the window
    /// monotonically to the front and to the end (10-pages scenario).
    #[test]
    fn value_paging_reaches_both_ends() {
        use super::layout_field_line;
        let len = 500usize; // ~10 windows at width 60
        let page = 28usize; // (60-4)/2
        // Walk left from the end: window start must strictly decrease to 0.
        let mut cursor = len;
        let mut prev_start = usize::MAX;
        for _ in 0..40 {
            let l = layout_field_line(60, len, Some(cursor), false, true, 16);
            assert!(l.window_start < prev_start || l.window_start == 0);
            prev_start = l.window_start;
            if l.left_more_x.is_none() {
                break;
            }
            cursor = cursor.saturating_sub(page);
        }
        assert_eq!(prev_start, 0, "left paging must reach the front");
        // Walk right from the front: window must reach the very end.
        let mut cursor = 0usize;
        let mut reached_end = false;
        for _ in 0..40 {
            let l = layout_field_line(60, len, Some(cursor), false, true, 16);
            if l.right_more_x.is_none() {
                assert_eq!(l.window_start + l.window_len, len);
                reached_end = true;
                break;
            }
            cursor = (cursor + page).min(len);
        }
        assert!(reached_end, "right paging must reach the end");
    }

    #[test]
    fn hits_fields_clear_and_buttons() {
        use crate::app::SearchField;
        let popup = Rect::new(10, 5, 70, 20);
        let mut lens = [0usize; 9];
        lens[0] = 5; // Quick holds "hello"
        // inner starts at (12, 6); visual rows 6..14; buttons at row 18.
        assert_eq!(
            search_popup_hit_in(popup, false, &lens, 0, 0, true, 20, 6),
            Some(SearchPopupHit::Field(0))
        );
        // Bottom visual row (7) is Tag (logical index 8).
        assert_eq!(
            search_popup_hit_in(popup, false, &lens, 0, 0, true, 20, 13),
            Some(SearchPopupHit::Field(SearchField::Tag.index()))
        );
        // Quick is multi-value: [ OR ] first at 12+16+5+1+2 = 36..42, then
        // [ clear ] at 43..52.
        for x in 36..42 {
            assert_eq!(
                search_popup_hit_in(popup, false, &lens, 0, 0, true, x, 6),
                Some(SearchPopupHit::OrAppend(0)),
                "column {x}"
            );
        }
        for x in 43..52 {
            assert_eq!(
                search_popup_hit_in(popup, false, &lens, 0, 0, true, x, 6),
                Some(SearchPopupHit::ClearField(0)),
                "column {x}"
            );
        }
        // Past the buttons is plain field focus; with the OR button hidden
        // (empty tail segment) the clear moves back left.
        assert_eq!(
            search_popup_hit_in(popup, false, &lens, 0, 0, true, 53, 6),
            Some(SearchPopupHit::Field(0))
        );
        assert_eq!(
            search_popup_hit_in(popup, false, &lens, 0, 0, false, 36, 6),
            Some(SearchPopupHit::ClearField(0))
        );
        assert_eq!(
            search_popup_hit_in(popup, false, &lens, 0, 0, true, 30, 7),
            Some(SearchPopupHit::Field(1))
        );

        // The shared date row (visual row 4 = terminal row 10): left of the
        // separator is From, right is To; the row's [ clear ] targets both.
        assert_eq!(
            search_popup_hit_in(popup, false, &lens, 0, 0, true, 12 + 20, 10),
            Some(SearchPopupHit::Field(SearchField::DateFrom.index()))
        );
        assert_eq!(
            search_popup_hit_in(
                popup,
                false,
                &lens,
                0,
                0,
                true,
                12 + super::DATE_TO_X + 2,
                10
            ),
            Some(SearchPopupHit::Field(SearchField::DateTo.index()))
        );
        let mut with_date = lens;
        with_date[SearchField::DateFrom.index()] = 10;
        let clear_x = 12 + super::DATE_CLEAR_X;
        assert_eq!(
            search_popup_hit_in(popup, false, &with_date, 0, 0, true, clear_x, 10),
            Some(SearchPopupHit::ClearField(SearchField::DateFrom.index()))
        );
        // No date content -> the clear zone is just field focus.
        assert_eq!(
            search_popup_hit_in(popup, false, &lens, 0, 0, true, clear_x, 10),
            Some(SearchPopupHit::Field(SearchField::DateTo.index()))
        );

        // Buttons at row 6 + 8 + 3 + 5 = 22: [ Search ] from x=12 width 10.
        assert_eq!(
            search_popup_hit_in(popup, false, &lens, 0, 0, true, 12, 22),
            Some(SearchPopupHit::Button(0))
        );
        assert_eq!(
            search_popup_hit_in(popup, false, &lens, 0, 0, true, 25, 22),
            Some(SearchPopupHit::Button(1))
        );
        assert_eq!(
            search_popup_hit_in(popup, false, &lens, 0, 0, true, 38, 22),
            Some(SearchPopupHit::Button(2))
        );
        // Gap between buttons hits nothing.
        assert_eq!(
            search_popup_hit_in(popup, false, &lens, 0, 0, true, 22, 22),
            None
        );
        assert_eq!(
            search_popup_hit_in(popup, false, &lens, 0, 0, true, 5, 5),
            None
        );
    }

    /// File-option rows (action selector + checkbox, above the buttons) are
    /// targets only while the file field has text.
    #[test]
    fn full_history_checkbox_hit_needs_file_value() {
        use crate::app::SearchField;
        let popup = Rect::new(10, 5, 70, 22);
        let lens = [0usize; 9];
        // Checkbox row 6 + 8 + 3 + 3 = 20. Empty file field: nothing there.
        assert_eq!(
            search_popup_hit_in(popup, false, &lens, 0, 0, true, 12, 20),
            None
        );
        let mut with_file = lens;
        with_file[SearchField::File.index()] = 8;
        assert_eq!(
            search_popup_hit_in(popup, false, &with_file, 0, 0, true, 12, 20),
            Some(SearchPopupHit::FullHistoryToggle)
        );
        // Past the label is dead space.
        let past = 12 + 4 + super::FULL_HISTORY_LABEL.len() as u16;
        assert_eq!(
            search_popup_hit_in(popup, false, &with_file, 0, 0, true, past, 20),
            None
        );

        // Action row 6 + 8 + 3 + 2 = 19: options at their zones, gaps dead.
        let zones = super::file_action_zones();
        assert_eq!(
            search_popup_hit_in(popup, false, &with_file, 0, 0, true, 12, 19),
            None
        );
        for (i, (x, width)) in zones.into_iter().enumerate() {
            assert_eq!(
                search_popup_hit_in(popup, false, &with_file, 0, 0, true, 12 + x, 19),
                Some(SearchPopupHit::FileAction(i)),
                "option {i} start"
            );
            assert_eq!(
                search_popup_hit_in(popup, false, &with_file, 0, 0, true, 12 + x + width - 1, 19),
                Some(SearchPopupHit::FileAction(i)),
                "option {i} end"
            );
        }
        // Empty file field: the action row is inert too.
        assert_eq!(
            search_popup_hit_in(popup, false, &lens, 0, 0, true, 12 + zones[0].0, 19),
            None
        );
    }
}
