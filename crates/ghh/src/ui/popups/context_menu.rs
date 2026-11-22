use crate::app::{ContextMenuAction, ContextMenuState};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

pub fn render_context_menu(frame: &mut Frame, area: Rect, menu: &ContextMenuState) {
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Menu ")
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
        );

    let mut lines: Vec<Line> = Vec::with_capacity(menu.content_line_count());
    for (index, action) in menu.actions.iter().enumerate() {
        let disabled =
            matches!(action, ContextMenuAction::PasteClipboard) && !menu.clipboard_available;
        let mut style = if disabled {
            Style::default().fg(Color::DarkGray)
        } else if action.is_destructive() {
            Style::default().fg(Color::LightRed)
        } else {
            Style::default().fg(Color::Yellow)
        };
        if index == menu.selected {
            style = style.bg(Color::DarkGray).add_modifier(Modifier::BOLD);
        }
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!(
                    "- {} [{}]",
                    menu.shortcut_for(index),
                    action.label(menu.clipboard_available)
                ),
                style,
            ),
            Span::raw(" "),
        ]));
        if menu.has_separator_after(index) {
            // The danger fence gets a label; ordinary separators stay plain.
            let danger_fence = !action.is_destructive()
                && menu
                    .actions
                    .get(index + 1)
                    .is_some_and(|next| next.is_destructive());
            let width = usize::from(area.width.saturating_sub(2));
            let line = if danger_fence {
                let label = " destructive ";
                format!(
                    "──{label}{}",
                    "─".repeat(width.saturating_sub(label.chars().count() + 2))
                )
            } else {
                "─".repeat(width)
            };
            lines.push(Line::from(Span::styled(
                line,
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::render_context_menu;
    use crate::app::{ContextMenuAction, ContextMenuState};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    /// Render into a test buffer: the danger fence must carry its label and
    /// the destructive entries must sit below it.
    #[test]
    fn danger_fence_renders_with_label() {
        let menu = ContextMenuState {
            x: 0,
            y: 0,
            actions: vec![
                ContextMenuAction::CopyHash,
                ContextMenuAction::EditCommit,
                ContextMenuAction::SquashFromHere,
            ],
            selected: 0,
            clipboard_available: false,
        };
        let backend = TestBackend::new(40, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_context_menu(frame, Rect::new(0, 0, 36, 7), &menu))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rows: Vec<String> = (0..7)
            .map(|y| {
                (0..36)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol().to_string())
                    .collect::<String>()
            })
            .collect();
        let all = rows.join("\n");
        assert!(all.contains("Copy hash"), "menu content missing:\n{all}");
        assert!(all.contains("destructive"), "fence label missing:\n{all}");
        assert!(
            all.contains("Edit commit"),
            "destructive entry missing:\n{all}"
        );
        assert!(all.contains("Squash from here"), "entry missing:\n{all}");
        let fence_row = rows.iter().position(|r| r.contains("destructive")).unwrap();
        let edit_row = rows.iter().position(|r| r.contains("Edit commit")).unwrap();
        let copy_row = rows.iter().position(|r| r.contains("Copy hash")).unwrap();
        assert!(
            copy_row < fence_row && fence_row < edit_row,
            "fence order wrong:\n{all}"
        );
    }
}
