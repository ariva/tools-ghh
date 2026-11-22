use crate::app::{App, SearchField};
use crate::ui::widgets::text_input::{TextInput, render_text_input};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

/// Field-specific hint for the popup's bottom line: what the input matches
/// and how alternatives are joined there (comma is an OR only in Files; in
/// Tag/Branch it is a literal character of the ref name).
pub(crate) fn hint_for(field: SearchField) -> &'static str {
    match field {
        SearchField::DateFrom | SearchField::DateTo => {
            "Date format: YYYY-MM-DD. Empty From = START of history, empty To = END (now)."
        }
        SearchField::Quick => {
            "Ctrl+U: clear field. Hash/message/author/date. ' OR ' joins. * ? wildcards."
        }
        SearchField::Hash => {
            "Ctrl+U: clear field. Substring of full/short hash. ' OR ' joins alternatives."
        }
        SearchField::Message => "Ctrl+U: clear field. Substring. ' OR ' joins. * ? wildcards.",
        SearchField::Author => {
            "Ctrl+U: clear field. Author name substring. ' OR ' joins alternatives."
        }
        SearchField::File => "Ctrl+U: clear field. Fuzzy terms. Comma or ' OR ' = any pattern.",
        SearchField::Branch => "Ctrl+U: clear field. Branch ref name. ' OR ' joins alternatives.",
        SearchField::Tag => "Ctrl+U: clear field. Tag ref name. ' OR ' joins alternatives.",
    }
}

pub fn render_search_popup(frame: &mut Frame, app: &mut App, area: Rect) {
    let popup = crate::app::layout::search_popup_rect_in(area);
    // Tiny/zero terminals (automation ptys report 0x0): every element below
    // positions absolutely inside the popup; with no room, fixed-width
    // widgets like [ clear ] would write outside the buffer and panic.
    if popup.width < 40 || popup.height < crate::app::layout::SEARCH_POPUP_HEIGHT {
        return;
    }
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(" Search Commits  (Enter: search  Tab: complete/next  Esc: close) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block, popup);

    let inner = Rect {
        x: popup.x + 2,
        y: popup.y + 1,
        width: popup.width.saturating_sub(4),
        height: popup.height.saturating_sub(2),
    };
    render_field_rows(frame, app, inner);
    render_date_row(frame, app, inner);
    render_suggestions_block(frame, app, inner);
    render_file_options(frame, app, inner);
    render_footer(frame, app, inner);
}

/// Label style shared by the field rows and the date row.
fn label_style(app: &App, active_for: &[SearchField]) -> Style {
    if active_for.contains(&app.search.field) {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    }
}

fn clear_button_span() -> Span<'static> {
    Span::styled(
        "[ clear ]",
        Style::default()
            .fg(Color::LightRed)
            .add_modifier(Modifier::BOLD),
    )
}

/// 7 single-value rows. 9 logical fields sit on 8 visual rows: the two date
/// fields share one line (see render_date_row). Geometry mirrored by
/// search_popup_hit_in.
fn render_field_rows(frame: &mut Frame, app: &mut App, inner: Rect) {
    for field in crate::app::layout::SINGLE_FIELD_ROWS {
        let visual_row = crate::app::layout::visual_row_of(field);
        let y = inner.y + visual_row;
        let active = field == app.search.field;
        let multi =
            crate::app::features::search::SearchState::multi_value_separator(field).is_some();
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("{:>14}: ", field.label()),
                label_style(app, &[field]),
            )),
            Rect {
                x: inner.x,
                y,
                width: 16.min(inner.width),
                height: 1,
            },
        );
        render_text_input(
            frame,
            Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            },
            &TextInput {
                value: app.search.value(field),
                placeholder: "",
                active,
                cursor: app.search.cursor,
                select_all: app.search.select_all,
                color_or_separators: multi,
                show_or_button: multi && active && !app.search.active_prefix().trim().is_empty(),
                show_clear_button: true,
                label_width: 16,
            },
        );
    }
}

/// The shared date row: `Date: <from> - <to> [ clear ]` at fixed slots.
fn render_date_row(frame: &mut Frame, app: &mut App, inner: Rect) {
    {
        use crate::app::layout::{DATE_FROM_SLOT, DATE_ROW, DATE_SEP_X, DATE_TO_SLOT, DATE_TO_X};
        let y = inner.y + DATE_ROW;
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("{:>14}: ", "Date"),
                label_style(app, &[SearchField::DateFrom, SearchField::DateTo]),
            )),
            Rect {
                x: inner.x,
                y,
                width: 16.min(inner.width),
                height: 1,
            },
        );
        render_text_input(
            frame,
            Rect {
                x: inner.x + 16,
                y,
                width: DATE_FROM_SLOT.min(inner.width.saturating_sub(16)),
                height: 1,
            },
            &TextInput {
                value: app.search.value(SearchField::DateFrom),
                placeholder: "START",
                active: app.search.field == SearchField::DateFrom,
                cursor: app.search.cursor,
                select_all: app.search.select_all,
                color_or_separators: false,
                show_or_button: false,
                show_clear_button: false,
                label_width: 0,
            },
        );
        if DATE_SEP_X + 2 <= inner.width {
            frame.render_widget(
                Paragraph::new(Span::styled("-", Style::default().fg(Color::Gray))),
                Rect {
                    x: inner.x + DATE_SEP_X + 1,
                    y,
                    width: 1,
                    height: 1,
                },
            );
        }
        render_text_input(
            frame,
            Rect {
                x: inner.x + DATE_TO_X,
                y,
                width: DATE_TO_SLOT.min(inner.width.saturating_sub(DATE_TO_X)),
                height: 1,
            },
            &TextInput {
                value: app.search.value(SearchField::DateTo),
                placeholder: "END",
                active: app.search.field == SearchField::DateTo,
                cursor: app.search.cursor,
                select_all: app.search.select_all,
                color_or_separators: false,
                show_or_button: false,
                show_clear_button: false,
                label_width: 0,
            },
        );
        // One clear for the whole date row (clears both sides).
        if !app.search.value(SearchField::DateFrom).is_empty()
            || !app.search.value(SearchField::DateTo).is_empty()
        {
            let width = crate::app::layout::CLEAR_MARK_WIDTH
                .min(inner.width.saturating_sub(crate::app::layout::DATE_CLEAR_X));
            if width > 0 {
                frame.render_widget(
                    Paragraph::new(clear_button_span()),
                    Rect {
                        x: inner.x + crate::app::layout::DATE_CLEAR_X,
                        y,
                        width,
                        height: 1,
                    },
                );
            }
        }
    }
}

/// Fuzzy-ranked suggestions: up to three wrapped lines, each candidate
/// clickable, "..." pages to the next batch. Layout shared with hit-testing
/// via layout_suggestions.
fn render_suggestions_block(frame: &mut Frame, app: &mut App, inner: Rect) {
    let suggestions = app.search_suggestions();
    if !suggestions.is_empty() {
        use crate::app::layout::{SUGGESTION_INDENT, layout_suggestions};
        let widths: Vec<u16> = suggestions
            .iter()
            .map(|(c, _)| c.chars().count() as u16)
            .collect();
        let (placed, more) =
            layout_suggestions(&widths, app.search.suggestion_page_start, inner.width);
        frame.render_widget(
            Paragraph::new(Span::styled(
                "Suggestions: ",
                Style::default().fg(Color::Gray),
            )),
            Rect {
                x: inner.x,
                y: inner.y + crate::app::layout::VISUAL_ROWS + 1,
                width: SUGGESTION_INDENT.min(inner.width),
                height: 1,
            },
        );
        for p in &placed {
            let (candidate, positions) = &suggestions[p.index];
            let spans: Vec<Span> = candidate
                .chars()
                .take(p.width as usize)
                .enumerate()
                .map(|(char_idx, ch)| {
                    let style = if positions.binary_search(&char_idx).is_ok() {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    };
                    Span::styled(ch.to_string(), style)
                })
                .collect();
            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect {
                    x: inner.x + p.x,
                    y: inner.y + crate::app::layout::VISUAL_ROWS + 1 + p.line,
                    width: p.width,
                    height: 1,
                },
            );
        }
        if app.search.suggestion_page_start > 0 {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "...",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
                Rect {
                    x: inner.x + SUGGESTION_INDENT,
                    y: inner.y + crate::app::layout::VISUAL_ROWS + 1,
                    width: 3,
                    height: 1,
                },
            );
        }
        if let Some((line, x)) = more {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "...",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
                Rect {
                    x: inner.x + x,
                    y: inner.y + crate::app::layout::VISUAL_ROWS + 1 + line,
                    width: 3,
                    height: 1,
                },
            );
        }
    }
}

/// File options while the file field has text: the action selector (ring
/// slot 5, click an option or Enter/Space cycles) and the full-history
/// checkbox (ring slot 4) on the next row.
fn render_file_options(frame: &mut Frame, app: &mut App, inner: Rect) {
    if !app
        .search
        .value(crate::app::SearchField::File)
        .trim()
        .is_empty()
    {
        use crate::app::features::search::FileAction;
        use crate::app::layout::{FILE_ACTION_LABELS, file_action_zones};
        let action_focused = app.search.button_focus == Some(5);
        let selected = FileAction::ALL
            .iter()
            .position(|a| *a == app.search.file_action)
            .unwrap_or(0);
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("{:>14}: ", "File action"),
                Style::default().fg(Color::Gray),
            )),
            Rect {
                x: inner.x,
                y: inner.y
                    + crate::app::layout::VISUAL_ROWS
                    + crate::app::layout::SUGGESTION_LINES
                    + 2,
                width: 16.min(inner.width),
                height: 1,
            },
        );
        for (i, (x, width)) in file_action_zones().into_iter().enumerate() {
            if x + width > inner.width {
                break;
            }
            let text = if i == selected {
                format!("({})", FILE_ACTION_LABELS[i])
            } else {
                format!(" {} ", FILE_ACTION_LABELS[i])
            };
            let style = if i == selected && action_focused {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if i == selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            frame.render_widget(
                Paragraph::new(Span::styled(text, style)),
                Rect {
                    x: inner.x + x,
                    y: inner.y
                        + crate::app::layout::VISUAL_ROWS
                        + crate::app::layout::SUGGESTION_LINES
                        + 2,
                    width,
                    height: 1,
                },
            );
        }
        let focused = app.search.button_focus == Some(4);
        let mark = if app.search.file_full_history {
            "x"
        } else {
            " "
        };
        let box_style = if focused {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("[{mark}] "), box_style),
                Span::styled(
                    crate::app::layout::FULL_HISTORY_LABEL,
                    if focused {
                        box_style
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                ),
            ])),
            Rect {
                x: inner.x,
                y: inner.y
                    + crate::app::layout::VISUAL_ROWS
                    + crate::app::layout::SUGGESTION_LINES
                    + 3,
                width: inner.width,
                height: 1,
            },
        );
    }
}

/// Bottom block: buttons row, the field-specific hint line (hidden while the
/// bottom slots have focus) and the notice/status line.
fn render_footer(frame: &mut Frame, app: &mut App, inner: Rect) {
    let hint = hint_for(app.search.field);
    if app.search.button_focus.is_none() {
        frame.render_widget(
            Paragraph::new(hint).style(Style::default().fg(Color::Gray)),
            Rect {
                x: inner.x,
                y: inner.y + inner.height.saturating_sub(2),
                width: inner.width,
                height: 1,
            },
        );
    }
    // Popup status line: bottom row, under the buttons.
    if let Some(notice) = &app.search.notice {
        frame.render_widget(
            Paragraph::new(notice.as_str()).style(
                Style::default()
                    .fg(Color::LightRed)
                    .add_modifier(Modifier::BOLD),
            ),
            Rect {
                x: inner.x,
                y: inner.y + inner.height.saturating_sub(1),
                width: inner.width,
                height: 1,
            },
        );
    }
    // Buttons; the third slot flips to "Undo Reset" right after a Reset.
    let reset_label = if app.search.reset_backup.is_some() {
        "Undo Reset"
    } else {
        "Reset"
    };
    let mut spans: Vec<Span> = Vec::new();
    for (i, label) in ["Search", "Cancel", reset_label, "Help"]
        .into_iter()
        .enumerate()
    {
        let focused = app.search.button_focus == Some(i);
        // Enter in any field runs Search, so it stays visually the default.
        let is_default = i == 0 && app.search.button_focus.is_none();
        let style = if focused {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if is_default {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(format!("[ {label} ]"), style));
        spans.push(Span::raw("   "));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            x: inner.x,
            y: inner.y + crate::app::layout::VISUAL_ROWS + crate::app::layout::SUGGESTION_LINES + 5,
            width: inner.width,
            height: 1,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::render_search_popup;
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    /// Tiny/zero-sized terminals must not panic the popup (automation ptys
    /// report 0x0; the [ clear ] button has a fixed width and used to write
    /// outside the buffer).
    #[test]
    fn popup_survives_tiny_terminals() {
        let dir = std::env::temp_dir().join(format!("ghh-popup-tiny-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = crate::app::App::new(dir, true);
        app.nav.push(crate::app::models::Mode::Search);
        *app.search.value_mut() = "hello".into();
        for (w, h) in [(1, 1), (10, 5), (39, 15), (40, 3)] {
            let backend = TestBackend::new(w.max(1), h.max(1));
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    render_search_popup(frame, &mut app, Rect::new(0, 0, w, h));
                })
                .unwrap();
        }
    }

    /// Full-history checkbox: drawn only with file text, [x] by default,
    /// [ ] after toggling off.
    #[test]
    fn full_history_checkbox_renders_with_file_value() {
        let dir = std::env::temp_dir().join(format!("ghh-popup-fullhist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = crate::app::App::new(dir, true);
        app.nav.push(crate::app::models::Mode::Search);
        let render_text = |app: &mut crate::app::App| {
            let backend = TestBackend::new(120, 30);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| render_search_popup(frame, app, Rect::new(0, 0, 120, 30)))
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            (0..30)
                .map(|y| {
                    (0..120)
                        .map(|x| buffer[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let hidden = render_text(&mut app);
        assert!(!hidden.contains("Full history"), "hidden without file text");
        assert!(!hidden.contains("File action"), "action row hidden too");
        app.search.set_field(crate::app::SearchField::File);
        *app.search.value_mut() = "src/a.rs".into();
        let shown = render_text(&mut app);
        assert!(shown.contains("[x] Full history"), "on by default");
        assert!(shown.contains("(All)"), "All selected by default");
        assert!(shown.contains(" Added "), "other options visible");
        app.search.file_full_history = false;
        app.search.file_action = crate::app::features::search::FileAction::Deleted;
        let toggled = render_text(&mut app);
        assert!(toggled.contains("[ ] Full history"), "off after toggle");
        assert!(toggled.contains("(Deleted)"), "selection moved");
        assert!(toggled.contains(" All "), "All unselected now");
    }

    /// Drift catcher: render the popup, find the drawn interactive elements
    /// in the buffer, and assert the hit test resolves EXACTLY those screen
    /// coordinates. If renderer and hit-test geometry ever disagree again
    /// (the off-by-one-row click bug), a landmark lands outside its zone.
    #[test]
    fn rendered_elements_match_hit_zones() {
        use crate::app::layout::{SearchPopupHit, search_popup_hit_in, search_popup_rect_in};
        let dir = std::env::temp_dir().join(format!("ghh-popup-drift-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = crate::app::App::new(dir, true);
        app.nav.push(crate::app::models::Mode::Search);
        app.search.set_field(crate::app::SearchField::DateFrom);
        *app.search.value_mut() = "2020-12-25".into();
        app.search.set_field(crate::app::SearchField::File);
        *app.search.value_mut() = "src/a.rs".into();
        app.search.cursor = 8;

        let area = Rect::new(0, 0, 100, 30);
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_search_popup(frame, &mut app, area))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rows: Vec<String> = (0..30)
            .map(|y| {
                (0..100)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect()
            })
            .collect();
        let find = |needle: &str| -> (u16, u16) {
            // Cell-accurate: byte offsets drift on multibyte border chars.
            let n: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
            for y in 0..30u16 {
                for x in 0..100u16.saturating_sub(n.len() as u16) {
                    if (0..n.len()).all(|i| buffer[(x + i as u16, y)].symbol() == n[i]) {
                        return (x, y);
                    }
                }
            }
            panic!("{needle:?} not drawn:\n{}", rows.join("\n"));
        };

        let popup = search_popup_rect_in(area);
        let mut lens = [0usize; 9];
        lens[crate::app::SearchField::DateFrom.index()] = 10;
        lens[crate::app::SearchField::File.index()] = 8;
        let hit = |x: u16, y: u16| {
            search_popup_hit_in(
                popup,
                false,
                &lens,
                crate::app::SearchField::File.index(),
                8,
                true,
                x,
                y,
            )
        };

        let (x, y) = find("[ Search ]");
        assert_eq!(
            hit(x + 1, y),
            Some(SearchPopupHit::Button(0)),
            "Search button"
        );
        let (x, y) = find("[ Cancel ]");
        assert_eq!(
            hit(x + 1, y),
            Some(SearchPopupHit::Button(1)),
            "Cancel button"
        );
        let (x, y) = find("[ Help ]");
        assert_eq!(
            hit(x + 1, y),
            Some(SearchPopupHit::Button(3)),
            "Help button"
        );
        let (x, y) = find("[x] Full history");
        assert_eq!(
            hit(x + 1, y),
            Some(SearchPopupHit::FullHistoryToggle),
            "checkbox"
        );
        let (x, y) = find("(All)");
        assert_eq!(
            hit(x + 1, y),
            Some(SearchPopupHit::FileAction(0)),
            "action All"
        );
        let (x, y) = find(" Deleted "); // unique to the action row
        assert_eq!(
            hit(x + 2, y),
            Some(SearchPopupHit::FileAction(3)),
            "action Deleted"
        );
        let (x, y) = find("Quick search:");
        assert_eq!(
            hit(x + 15, y),
            Some(SearchPopupHit::Field(
                crate::app::SearchField::Quick.index()
            )),
            "quick field row"
        );
        let (x, y) = find("2020-12-25");
        assert_eq!(
            hit(x + 2, y),
            Some(SearchPopupHit::Field(
                crate::app::SearchField::DateFrom.index()
            )),
            "date from value"
        );
        let (x, y) = find("[ clear ]"); // first match: the date row's clear
        assert_eq!(
            hit(x + 1, y),
            Some(SearchPopupHit::ClearField(
                crate::app::SearchField::DateFrom.index()
            )),
            "date clear"
        );
    }

    /// Hints per field come from hint_for (pure, table-tested); one render
    /// verifies the wiring and that the line hides while buttons have focus.
    #[test]
    fn hint_line_matches_focused_field() {
        use super::hint_for;
        let cases = [
            (crate::app::SearchField::Quick, "Hash/message/author/date"),
            (
                crate::app::SearchField::Hash,
                "Substring of full/short hash",
            ),
            (crate::app::SearchField::Message, "Substring. ' OR ' joins"),
            (crate::app::SearchField::Author, "Author name substring"),
            (crate::app::SearchField::DateFrom, "Date format: YYYY-MM-DD"),
            (crate::app::SearchField::DateTo, "Date format: YYYY-MM-DD"),
            (
                crate::app::SearchField::File,
                "Comma or ' OR ' = any pattern",
            ),
            (crate::app::SearchField::Branch, "Branch ref name"),
            (crate::app::SearchField::Tag, "Tag ref name"),
        ];
        for (field, expected) in cases {
            let hint = hint_for(field);
            assert!(
                hint.contains(expected),
                "{field:?}: {expected:?} not in {hint:?}"
            );
        }
        // Only File may claim comma semantics.
        for field in crate::app::SearchField::ALL {
            let claims_comma = hint_for(field).to_lowercase().contains("comma");
            assert_eq!(
                claims_comma,
                field == crate::app::SearchField::File,
                "{field:?}"
            );
        }

        let dir = std::env::temp_dir().join(format!("ghh-popup-hints-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = crate::app::App::new(dir, true);
        app.nav.push(crate::app::models::Mode::Search);
        app.search.set_field(crate::app::SearchField::Tag);
        let render_text = |app: &mut crate::app::App| {
            let backend = TestBackend::new(120, 30);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| render_search_popup(frame, app, Rect::new(0, 0, 120, 30)))
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            (0..30)
                .map(|y| {
                    (0..120)
                        .map(|x| buffer[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(
            render_text(&mut app).contains("Tag ref name"),
            "hint wired into render"
        );
        app.search.button_focus = Some(0);
        assert!(
            !render_text(&mut app).contains("Ctrl+U: clear field"),
            "hint should hide while buttons have focus"
        );
    }

    /// The clear mark and the in-popup notice must actually draw.
    #[test]
    fn popup_renders_clear_mark_and_notice() {
        let dir = std::env::temp_dir().join(format!("ghh-popup-render-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = crate::app::App::new(dir, true);
        app.nav.push(crate::app::models::Mode::Search);
        *app.search.value_mut() = "hello".into();
        app.search.notice = Some("No matches. Adjust the search.".into());

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_search_popup(frame, &mut app, Rect::new(0, 0, 80, 24)))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let all: String = (0..24)
            .map(|y| {
                (0..80)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol().to_string())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        assert!(all.contains("[ clear ]"), "clear button missing:\n{all}");
        // Shared date row: label, placeholder tokens and the separator on one line.
        let date_row = all
            .lines()
            .find(|l| l.contains("Date:"))
            .expect("date row missing");
        assert!(
            date_row.contains("START"),
            "from placeholder missing: {date_row}"
        );
        assert!(
            date_row.contains("END"),
            "to placeholder missing: {date_row}"
        );
        assert!(date_row.contains(" - "), "separator missing: {date_row}");
        assert!(all.contains("No matches"), "notice missing:\n{all}");
        assert!(all.contains("[ Search ]"), "buttons missing:\n{all}");
        // Notice is the popup's status line: below the buttons.
        let rows: Vec<&str> = all.lines().collect();
        let notice_row = rows.iter().position(|r| r.contains("No matches")).unwrap();
        let buttons_row = rows.iter().position(|r| r.contains("[ Search ]")).unwrap();
        assert!(
            notice_row > buttons_row,
            "notice must sit under the buttons"
        );
    }
}
