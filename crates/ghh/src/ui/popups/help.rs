//! Help popup: the key list of the active screen, scrollable when it is
//! longer than the popup, with close controls.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

pub fn render_help_popup(
    frame: &mut Frame,
    title: &str,
    items: &[String],
    scroll: usize,
    area: Rect,
) {
    let popup = crate::app::layout::help_popup_rect_in(items.len(), area);
    frame.render_widget(Clear, popup);

    // Clamped here too: the terminal may have grown since the last key.
    let max_scroll = crate::app::layout::help_max_scroll(items.len(), popup);
    let scroll = scroll.min(max_scroll);
    let title = if max_scroll > 0 {
        format!(" {title} (j/k, PgUp/PgDn: scroll) ")
    } else {
        format!(" {title} ")
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        );

    let lines: Vec<Line<'static>> = items
        .iter()
        .map(|item| {
            // Section dividers (see app/features/help.rs) stand out from the keys.
            let style = if item.starts_with('─') {
                Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(vec![Span::raw("  "), Span::styled(item.clone(), style)])
        })
        .collect();

    frame.render_widget(block, popup);
    // The text keeps clear of the blank row and the [ OK ] row below it.
    let pane = crate::app::layout::help_text_pane(popup);
    let text = Rect::new(
        pane.x + 1,
        pane.y + 1,
        pane.width.saturating_sub(2),
        pane.height.saturating_sub(2),
    );
    frame.render_widget(
        Paragraph::new(lines).scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0)),
        text,
    );
    if max_scroll > 0 {
        // On the right border, beside the text rows only.
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            Rect::new(popup.x, text.y, popup.width, text.height),
            &mut ScrollbarState::new(max_scroll + 1).position(scroll),
        );
    }

    // Clickable close controls: [X] on the border's top-right, [ OK ] on the
    // bottom row (zones shared with mouse hit-testing via layout helpers).
    frame.render_widget(
        Paragraph::new(Span::styled(
            "[X]",
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        )),
        crate::app::layout::help_close_rect(popup),
    );
    // Selected by default: Enter accepts it (the only button here).
    frame.render_widget(
        Paragraph::new(Span::styled(
            "[ OK ]",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        crate::app::layout::help_ok_rect(popup),
    );
}

#[cfg(test)]
mod tests {
    use super::render_help_popup;
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    /// [X] and [ OK ] draw exactly on the zones the mouse hit-test uses
    /// (drift catcher, same idea as the search popup's).
    #[test]
    fn close_controls_drawn_at_hit_zones() {
        let area = Rect::new(0, 0, 80, 24);
        let items: Vec<String> = (0..5).map(|i| format!("line {i}")).collect();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_help_popup(frame, "Search Help", &items, 0, area))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let find = |needle: &str| -> (u16, u16) {
            let n: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
            for y in 0..24u16 {
                for x in 0..80u16.saturating_sub(n.len() as u16) {
                    if (0..n.len()).all(|i| buffer[(x + i as u16, y)].symbol() == n[i]) {
                        return (x, y);
                    }
                }
            }
            panic!("{needle:?} not drawn");
        };
        let popup = crate::app::layout::help_popup_rect_in(items.len(), area);
        let close = crate::app::layout::help_close_rect(popup);
        let ok = crate::app::layout::help_ok_rect(popup);
        assert_eq!(find("[X]"), (close.x, close.y), "[X] on its hit zone");
        assert_eq!(find("[ OK ]"), (ok.x, ok.y), "[ OK ] on its hit zone");
        assert_eq!(ok.x, popup.x + 2, "buttons left-aligned like other popups");
        // The Enter/Esc hint line is gone — the selected [ OK ] says it all.
        for y in 0..24u16 {
            let row: String = (0..80)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect();
            assert!(!row.contains("Enter"), "stale Enter/Esc hint drawn");
        }
    }

    /// Rows of an 80x24 frame with `items` drawn at `scroll`.
    fn rows(items: &[String], scroll: usize) -> Vec<String> {
        let area = Rect::new(0, 0, 80, 24);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| render_help_popup(frame, "Help", items, scroll, area))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..24u16)
            .map(|y| {
                (0..80)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect()
            })
            .collect()
    }

    /// Help longer than the terminal: the end is reachable by scrolling and
    /// the text never runs under [ OK ].
    #[test]
    fn long_help_scrolls_to_its_last_line() {
        let items: Vec<String> = (0..50).map(|i| format!("line-{i:02}")).collect();
        let top = rows(&items, 0).join("\n");
        assert!(top.contains("line-00") && !top.contains("line-49"));
        assert!(top.contains("scroll"), "title says the text scrolls");

        // Past the end on purpose: the renderer clamps.
        let end = rows(&items, 999);
        let joined = end.join("\n");
        assert!(joined.contains("line-49") && !joined.contains("line-00"));
        let ok_row = end.iter().position(|r| r.contains("[ OK ]")).unwrap();
        let last = end.iter().position(|r| r.contains("line-49")).unwrap();
        assert_eq!(last + 2, ok_row, "blank row between text and [ OK ]");
    }

    #[test]
    fn short_help_has_no_scroll_hint() {
        let items: Vec<String> = (0..5).map(|i| format!("line-{i:02}")).collect();
        assert!(!rows(&items, 0).join("\n").contains("scroll"));
    }
}
