use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

pub fn render_help_popup(frame: &mut Frame, title: &str, items: &[String], area: Rect) {
    let popup = crate::app::layout::help_popup_rect_in(items.len(), area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        );

    let lines: Vec<Line<'static>> = items
        .iter()
        .map(|item| Line::from(vec![Span::raw("  "), Span::raw(item.clone())]))
        .collect();

    frame.render_widget(Paragraph::new(lines).block(block), popup);

    // Clickable close controls: [X] on the border's top-right, [ OK ] on the
    // bottom row (zones shared with mouse hit-testing via layout helpers).
    frame.render_widget(
        Paragraph::new(Span::styled(
            "[X]",
            Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD),
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
            .draw(|frame| render_help_popup(frame, "Search Help", &items, area))
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
            let row: String = (0..80).map(|x| buffer[(x, y)].symbol().to_string()).collect();
            assert!(!row.contains("Enter"), "stale Enter/Esc hint drawn");
        }
    }
}
