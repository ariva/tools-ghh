use crate::app::ConfirmAction;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

pub fn render_confirm_popup(
    frame: &mut Frame,
    action: &ConfirmAction,
    yes_selected: bool,
    area: Rect,
) {
    let popup = crate::app::layout::confirm_popup_rect_in(action.is_dangerous(), area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Confirm ")
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];
    if action.is_dangerous() {
        lines.push(Line::styled(
            "  DANGER",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }
    for l in action.prompt_lines() {
        lines.push(Line::styled(
            format!("  {l}"),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));
    }
    lines.push(Line::raw(""));

    let yes_style = if yes_selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green)
    };
    let no_style = if !yes_selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Red)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Red)
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("  Yes  ", yes_style),
        Span::raw("   "),
        Span::styled("  No  ", no_style),
        Span::styled(
            "      \u{2190}\u{2192} switch  Enter accept",
            Style::default().fg(Color::DarkGray),
        ),
    ]));

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, popup);
}

#[cfg(test)]
mod tests {
    use super::render_confirm_popup;
    use crate::app::ConfirmAction;
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    /// Drift catcher: the rendered Yes/No must sit on the button zones
    /// App::confirm_popup_button_rects hands to mouse hit-testing.
    #[test]
    fn buttons_drawn_on_hit_zones() {
        let area = Rect::new(0, 0, 90, 24);
        let action = ConfirmAction::RemoveOrSegment {
            field_index: 3,
            segment_index: 0,
            preview: "OR alice".into(),
        };
        let backend = TestBackend::new(90, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_confirm_popup(frame, &action, true, area))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let find = |needle: &str| -> (u16, u16) {
            let n: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
            for y in 0..24u16 {
                for x in 0..90u16.saturating_sub(n.len() as u16) {
                    if (0..n.len()).all(|i| buffer[(x + i as u16, y)].symbol() == n[i]) {
                        return (x, y);
                    }
                }
            }
            panic!("{needle:?} not drawn");
        };
        let popup = crate::app::layout::confirm_popup_rect_in(action.is_dangerous(), area);
        // Same math as App::confirm_popup_button_rects.
        let button_y = popup.y
            + action.prompt_lines().len() as u16
            + if action.is_dangerous() { 4 } else { 3 };
        let (yes_x, yes_y) = find("Yes");
        assert_eq!(yes_y, button_y, "Yes on the hit row");
        assert!(
            (popup.x + 3..popup.x + 10).contains(&yes_x),
            "Yes inside its zone (x={yes_x}, zone starts {})",
            popup.x + 3
        );
        let (no_x, no_y) = find("No");
        assert_eq!(no_y, button_y);
        assert!(
            (popup.x + 13..popup.x + 19).contains(&no_x),
            "No inside its zone (x={no_x}, zone starts {})",
            popup.x + 13
        );
    }
}
