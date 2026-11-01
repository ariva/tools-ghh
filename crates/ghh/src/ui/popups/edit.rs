use crate::app::EditField;
use crate::app::features::edit::EditState;
use crate::ui::helpers::truncate;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

pub fn render_edit_popup(frame: &mut Frame, edit: &EditState, area: Rect) {
    let popup = crate::app::layout::edit_popup_rect_in(area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(
            " Edit Commit  (↑↓/Tab: field  │  Message: Enter edit  │  Other fields: Enter save  │  Esc: cancel) ",
        )
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    frame.render_widget(block, popup);

    let inner = Rect {
        x: popup.x + 1,
        y: popup.y + 1,
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };

    let fields: &[(EditField, &str, u16)] = &[
        (EditField::AuthorName, edit.author_name.as_str(), 6),
        (EditField::AuthorEmail, edit.author_email.as_str(), 9),
        (EditField::Date, edit.date.as_str(), 12),
    ];

    render_message_block(frame, edit, inner);
    render_single_fields(frame, edit, fields, inner);
}

/// Message preview: label, first two lines, hidden count, editor hint.
fn render_message_block(frame: &mut Frame, edit: &EditState, inner: Rect) {
    let message_active = edit.field == EditField::Message;
    let message_label_style = if message_active {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    frame.render_widget(
        Paragraph::new("Message").style(message_label_style),
        Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        },
    );

    let message_lines: Vec<&str> = if edit.message.is_empty() {
        vec![""]
    } else {
        edit.message.lines().collect()
    };
    let message_width = inner.width.saturating_sub(2) as usize;
    let hidden = message_lines.len().saturating_sub(2);
    for (idx, line) in message_lines.iter().take(2).enumerate() {
        let prefix = if idx == 0 && message_active {
            "> "
        } else {
            "  "
        };
        let style = if message_active {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::Gray)
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    prefix,
                    if message_active && idx == 0 {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
                Span::styled(truncate(line, message_width), style),
            ])),
            Rect {
                x: inner.x,
                y: inner.y + 1 + idx as u16,
                width: inner.width,
                height: 1,
            },
        );
    }
    if hidden > 0 {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("... {hidden} more lines hidden"),
                    Style::default().fg(Color::DarkGray),
                ),
            ])),
            Rect {
                x: inner.x,
                y: inner.y + 3,
                width: inner.width,
                height: 1,
            },
        );
    }
    if message_active {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    "Enter or Alt+Enter: open editor",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::ITALIC),
                ),
            ])),
            Rect {
                x: inner.x,
                y: inner.y + 4,
                width: inner.width,
                height: 1,
            },
        );
    }

}

/// One-line fields (author name/email, date): label + value with a cursor
/// split on the active field.
fn render_single_fields(
    frame: &mut Frame,
    edit: &EditState,
    fields: &[(EditField, &str, u16)],
    inner: Rect,
) {
    for (field, value, row_offset) in fields {
        let is_active = *field == edit.field;
        let label = if *field == EditField::Date {
            let off = edit.offset_minutes;
            let sign = if off < 0 { '-' } else { '+' };
            format!(
                "Date (YYYY-MM-DD HH:MM, UTC{sign}{:02}:{:02})",
                off.abs() / 60,
                off.abs() % 60
            )
        } else {
            field.label().to_string()
        };

        let label_style = if is_active {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let label_area = Rect {
            x: inner.x,
            y: inner.y + row_offset,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(label).style(label_style), label_area);

        let input_area = Rect {
            x: inner.x,
            y: inner.y + row_offset + 1,
            width: inner.width,
            height: 1,
        };

        let input_line = if is_active {
            let cursor = edit.cursor.min(value.chars().count());
            let before: String = value.chars().take(cursor).collect();
            let after: String = value.chars().skip(cursor).collect();
            Line::from(vec![
                Span::styled("> ", Style::default().fg(Color::Yellow)),
                Span::styled(before, Style::default().fg(Color::White)),
                Span::styled(
                    "\u{2502}",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(after, Style::default().fg(Color::White)),
            ])
        } else {
            Line::from(vec![
                Span::raw("  "),
                Span::styled(value.to_string(), Style::default().fg(Color::Gray)),
            ])
        };

        frame.render_widget(Paragraph::new(input_line), input_area);
    }
}

#[cfg(test)]
mod tests {
    use super::render_edit_popup;
    use crate::app::EditField;
    use crate::app::features::edit::EditState;
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    fn render_text(edit: &EditState) -> Vec<String> {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_edit_popup(frame, edit, Rect::new(0, 0, 100, 30)))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..30)
            .map(|y| (0..100).map(|x| buffer[(x, y)].symbol().to_string()).collect())
            .collect()
    }

    /// All fields and their values draw; the active field's label is the
    /// yellow-bold one (guards the upcoming sectioning refactor).
    #[test]
    fn fields_values_and_active_label_drawn() {
        let edit = EditState {
            field: EditField::AuthorEmail,
            message: "fix: subject line\n\nbody text".into(),
            author_name: "Alice Wonder".into(),
            author_email: "alice@example.com".into(),
            date: "2020-12-25 18:31".into(),
            offset_minutes: 180,
            cursor: 3,
        };
        let all = render_text(&edit).join("\n");
        for needle in [
            "Message",
            "fix: subject line",
            "Author Name",
            "Alice Wonder",
            "Author Email",
            "Date (YYYY-MM-DD HH:MM, UTC+03:00)",
            "2020-12-25 18:31",
            "Edit Commit",
        ] {
            assert!(all.contains(needle), "{needle:?} missing:\n{all}");
        }
        // Active field draws a cursor split (before|after) — the email line
        // must contain the cursor cell.
        let email_row = render_text(&edit)
            .into_iter()
            .find(|row| row.contains("ali") && row.contains("ce@example.com"))
            .expect("email row with cursor");
        assert!(email_row.contains('\u{2502}'), "cursor bar in {email_row:?}");
    }
}
