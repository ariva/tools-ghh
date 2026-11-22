use crate::app::editor::MultilineEditor;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

pub fn render_multiline_editor_popup(
    frame: &mut Frame,
    editor: &MultilineEditor,
    title: &str,
    area: Rect,
    save_selected: bool,
    show_actions: bool,
    changed: bool,
) {
    let popup =
        crate::app::layout::multiline_editor_popup_rect_in(editor.lines.len(), show_actions, area);
    frame.render_widget(Clear, popup);

    let mode_label = if editor.insert_mode {
        "INSERT"
    } else {
        "VIEW ONLY"
    };
    let mode_color = if editor.insert_mode {
        Color::Green
    } else {
        Color::Yellow
    };
    let changed_label = if changed { "  * CHANGED" } else { "" };
    let block = Block::default()
        .title(format!(" {title}{changed_label}  [{mode_label}] "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(mode_color).add_modifier(Modifier::BOLD));

    let inner = Rect {
        x: popup.x + 1,
        y: popup.y + 1,
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    frame.render_widget(block, popup);

    let hint_y = inner.y
        + inner
            .height
            .saturating_sub(if show_actions { 3 } else { 2 });
    let hint = if editor.insert_mode {
        Paragraph::new(Line::from(vec![
            Span::styled(" Enter", Style::default().fg(Color::Green)),
            Span::raw(":newline  "),
            Span::styled("Esc", Style::default().fg(Color::Yellow)),
            Span::raw(":view only"),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled(" i", Style::default().fg(Color::Cyan)),
            Span::raw(":ins  "),
            Span::styled("a/A", Style::default().fg(Color::Cyan)),
            Span::raw(":append  "),
            Span::styled("o/O", Style::default().fg(Color::Cyan)),
            Span::raw(":new line+insert  "),
            Span::styled("dd", Style::default().fg(Color::Red)),
            Span::raw(":del line  "),
            Span::styled("Enter", Style::default().fg(Color::Green)),
            Span::raw(":confirm  "),
            Span::styled("Esc", Style::default().fg(Color::DarkGray)),
            Span::raw(":cancel"),
        ]))
    };
    frame.render_widget(
        hint,
        Rect {
            x: inner.x,
            y: hint_y,
            width: inner.width,
            height: 1,
        },
    );

    let content_height = inner
        .height
        .saturating_sub(if show_actions { 3 } else { 2 }) as usize;
    let content_width = inner.width as usize;
    let visible_start = editor.cursor_line.saturating_sub(content_height / 2);
    let visible_end = (visible_start + content_height).min(editor.lines.len());

    for (screen_row, line_idx) in (visible_start..visible_end).enumerate() {
        let y = inner.y + screen_row as u16;
        if y >= hint_y {
            break;
        }

        let raw = &editor.lines[line_idx];
        let is_current = line_idx == editor.cursor_line;

        if is_current {
            let chars: Vec<char> = raw.chars().collect();
            let col = editor.cursor_col.min(chars.len());
            let view_start = horizontal_view_start(col, content_width);
            let visible_chars = visible_chars(&chars, view_start, content_width.max(1));
            let visible_col = col.saturating_sub(view_start).min(visible_chars.len());
            let before: String = visible_chars[..visible_col].iter().collect();
            let cursor_ch = if visible_col < visible_chars.len() {
                visible_chars[visible_col]
            } else {
                ' '
            };
            let after: String = if visible_col < visible_chars.len() {
                visible_chars[visible_col + 1..].iter().collect()
            } else {
                String::new()
            };

            let is_comment = raw.starts_with('#');
            let text_style = if is_comment {
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC)
            } else {
                Style::default().fg(Color::White)
            };
            let spans = Line::from(vec![
                Span::styled(before, text_style),
                Span::styled(
                    cursor_ch.to_string(),
                    Style::default().fg(Color::Black).bg(mode_color),
                ),
                Span::styled(after, text_style),
            ]);
            frame.render_widget(
                Paragraph::new(spans),
                Rect {
                    x: inner.x,
                    y,
                    width: inner.width,
                    height: 1,
                },
            );
        } else {
            let is_comment = raw.starts_with('#');
            let style = if is_comment {
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC)
            } else {
                Style::default().fg(Color::White)
            };
            let visible: String = visible_chars(&raw.chars().collect::<Vec<_>>(), 0, content_width)
                .into_iter()
                .collect();
            frame.render_widget(
                Paragraph::new(Span::styled(visible, style)),
                Rect {
                    x: inner.x,
                    y,
                    width: inner.width,
                    height: 1,
                },
            );
        }
    }

    if show_actions {
        let button_y = inner.y + inner.height.saturating_sub(1);
        let save_style = if save_selected {
            Style::default().fg(Color::Black).bg(Color::Green)
        } else {
            Style::default().fg(Color::Green)
        };
        let cancel_style = if save_selected {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Black).bg(Color::Red)
        };
        frame.render_widget(
            Paragraph::new(" [ SAVE ] ").style(save_style),
            Rect {
                x: inner.x,
                y: button_y,
                width: 12,
                height: 1,
            },
        );
        frame.render_widget(
            Paragraph::new(" [ CANCEL ] ").style(cancel_style),
            Rect {
                x: inner.x + 14,
                y: button_y,
                width: 12,
                height: 1,
            },
        );
    }
}

fn horizontal_view_start(cursor_col: usize, content_width: usize) -> usize {
    let safe_width = content_width.max(1);
    cursor_col.saturating_sub(safe_width.saturating_sub(1))
}

fn visible_chars(chars: &[char], start: usize, width: usize) -> Vec<char> {
    chars.iter().skip(start).take(width).copied().collect()
}
