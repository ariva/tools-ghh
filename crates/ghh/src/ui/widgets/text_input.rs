//! Reusable single-line text input: windowed value that follows the cursor,
//! clickable "..." pagers on overflow, optional [ OR ] / [ clear ] buttons,
//! placeholder text, select-all highlight and colored " OR " separators.
//! Geometry comes from `layout_field_line`, the same function hit-testing
//! uses, so clicks and pixels cannot drift apart.

use crate::app::layout::{CLEAR_MARK_WIDTH, OR_BUTTON_WIDTH, layout_field_line};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

pub struct TextInput<'a> {
    pub value: &'a str,
    /// Shown (White) when the value is empty and the field is NOT focused.
    pub placeholder: &'a str,
    pub active: bool,
    pub cursor: usize,
    pub select_all: bool,
    /// Color " OR " separators cyan (multi-value fields).
    pub color_or_separators: bool,
    pub show_or_button: bool,
    pub show_clear_button: bool,
    /// Cells the caller's label occupies before the text area.
    pub label_width: u16,
}

/// Render into `area` (one row; x is the line start, label included in
/// `label_width` — the label itself is drawn by the caller).
pub fn render_text_input(frame: &mut Frame, area: Rect, input: &TextInput) {
    let chars: Vec<char> = input.value.chars().collect();
    let y = area.y;

    if chars.is_empty() && !input.active && !input.placeholder.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                input.placeholder.to_string(),
                Style::default().fg(Color::White),
            )),
            Rect {
                x: area.x + input.label_width,
                y,
                width: area.width.saturating_sub(input.label_width),
                height: 1,
            },
        );
        return;
    }

    let layout = layout_field_line(
        area.width,
        chars.len(),
        input.active.then(|| input.cursor.min(chars.len())),
        input.show_or_button,
        input.show_clear_button,
        input.label_width,
    );

    // " OR " separator chars get their own color.
    let sep_char = if input.color_or_separators {
        crate::app::features::search::or_separator_flags(&chars)
    } else {
        vec![false; chars.len()]
    };
    let selected = input.active && input.select_all && !chars.is_empty();
    let cursor = input.active.then(|| input.cursor.min(chars.len()));

    let mut spans: Vec<Span> = Vec::new();
    for i in layout.window_start..layout.window_start + layout.window_len {
        if cursor == Some(i) {
            spans.push(Span::styled("│", Style::default().fg(Color::Yellow)));
        }
        let style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if sep_char[i] {
            // Chip look: bg makes the joiner read as a button.
            Style::default()
                .fg(Color::Cyan)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        spans.push(Span::styled(chars[i].to_string(), style));
    }
    let window_end = layout.window_start + layout.window_len;
    // Covers the empty-field case too (0 == 0) — exactly ONE cursor.
    if cursor == Some(window_end) && window_end == chars.len() {
        spans.push(Span::styled("│", Style::default().fg(Color::Yellow)));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            x: area.x + layout.text_x,
            y,
            width: area.width.saturating_sub(layout.text_x),
            height: 1,
        },
    );

    let more = |frame: &mut Frame, x: u16| {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "...",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Rect {
                x: area.x + x,
                y,
                width: 3,
                height: 1,
            },
        );
    };
    if let Some(x) = layout.left_more_x {
        more(frame, x);
    }
    if let Some(x) = layout.right_more_x {
        more(frame, x);
    }
    if let Some(x) = layout.or_x {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "[ OR ]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Rect {
                x: area.x + x,
                y,
                width: OR_BUTTON_WIDTH,
                height: 1,
            },
        );
    }
    if let Some(x) = layout.clear_x {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "[ clear ]",
                Style::default()
                    .fg(Color::LightRed)
                    .add_modifier(Modifier::BOLD),
            )),
            Rect {
                x: area.x + x,
                y,
                width: CLEAR_MARK_WIDTH,
                height: 1,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{TextInput, render_text_input};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    fn cursor_count(value: &str, cursor: usize) -> usize {
        let backend = TestBackend::new(60, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_text_input(
                    frame,
                    Rect::new(0, 0, 60, 1),
                    &TextInput {
                        value,
                        placeholder: "",
                        active: true,
                        cursor,
                        select_all: false,
                        color_or_separators: false,
                        show_or_button: false,
                        show_clear_button: true,
                        label_width: 16,
                    },
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let row: String = (0..60)
            .map(|x| buffer.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        row.matches('\u{2502}').count()
    }

    /// Exactly ONE cursor at every position (double `││` regression on empty
    /// fields; guards mid-value and end-of-value too, short and windowed).
    #[test]
    fn active_field_always_has_single_cursor() {
        assert_eq!(cursor_count("", 0), 1, "empty field");
        assert_eq!(cursor_count("hello", 0), 1, "start of value");
        assert_eq!(cursor_count("hello", 3), 1, "mid value");
        assert_eq!(cursor_count("hello", 5), 1, "end of value");
        let long = "x".repeat(120);
        assert_eq!(cursor_count(&long, 0), 1, "long value, cursor at start");
        assert_eq!(cursor_count(&long, 60), 1, "long value, cursor mid-window");
        assert_eq!(cursor_count(&long, 120), 1, "long value, cursor at end");
    }
}
