use super::diff::{render_diff_window, scroll_label};
use crate::app::App;
use crate::ui::helpers::truncate;
use ratatui::{
    Frame,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

pub fn render_stash(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    // Geometry comes from app::layout so mouse hit-testing always agrees.
    let chunks = crate::app::layout::main_chunks(area, app.splits.details_height);
    let (list_rect, diff_rect, details_rect) =
        crate::app::layout::stash_regions_in(area, app.splits);

    let title = Paragraph::new(
        "STASHES  \u{2502}  j/k \u{2191}\u{2193}: select or scroll  \u{2502}  Tab: list/changes  \u{2502}  PgUp/PgDn: scroll changes  \u{2502}  a: apply  \u{2502}  p: pop  \u{2502}  d: drop  \u{2502}  r: reload  \u{2502}  Esc: back",
    )
    .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
    .alignment(ratatui::layout::Alignment::Center);
    frame.render_widget(title, chunks[0]);
    super::details::render_status(frame, app, chunks[3]);

    let Some(stash) = app.stash.as_mut() else { return };
    let border = |focused: bool| {
        Style::default().fg(if focused { Color::Yellow } else { Color::DarkGray })
    };

    let message_width = usize::from(list_rect.width).saturating_sub(34);
    let items: Vec<ListItem<'static>> = stash
        .entries
        .iter()
        .map(|e| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<10} ", e.name), Style::default().fg(Color::Yellow)),
                Span::styled(format!("{} ", e.date), Style::default().fg(Color::Cyan)),
                Span::raw(truncate(&e.message, message_width)),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(" Stashes ({}) ", stash.entries.len()))
                .borders(Borders::ALL)
                .border_style(border(!stash.diff_focused)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, list_rect, &mut stash.list_state);

    let meta = scroll_label(stash.diff_scroll, stash.diff_lines.len(), diff_rect.height);
    let diff = Paragraph::new(render_diff_window(&stash.diff_lines, stash.diff_scroll, diff_rect.height))
        .block(
            Block::default()
                .title(format!(" Changes [{meta}] "))
                .borders(Borders::ALL)
                .border_style(border(stash.diff_focused)),
        );
    frame.render_widget(diff, diff_rect);

    let label = |text: &'static str| Span::styled(text, Style::default().fg(Color::DarkGray));
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(e) = stash.selected() {
        lines.push(Line::from(vec![
            Span::styled(e.name.clone(), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {}", &e.hash[..7.min(e.hash.len())]), Style::default().fg(Color::Yellow)),
            label("   base: "),
            Span::raw(e.base.clone()),
            label("   date: "),
            Span::styled(e.date.clone(), Style::default().fg(Color::Cyan)),
            label("   author: "),
            Span::styled(e.author.clone(), Style::default().fg(Color::Green)),
        ]));
        lines.push(Line::from(vec![label("Message: "), Span::raw(e.message.clone())]));
        lines.extend(stash.stat_lines.iter().map(|l| Line::raw(l.clone())));
    } else {
        lines.push(Line::styled(
            "No stashes — create one with `git stash` (add -u to include untracked files).",
            Style::default().fg(Color::DarkGray),
        ));
    }
    let details = Paragraph::new(lines).block(
        Block::default()
            .title(" Stash Details ")
            .borders(Borders::ALL)
            .border_style(border(false)),
    );
    frame.render_widget(details, details_rect);
}

#[cfg(test)]
mod tests {
    use crate::app::features::stash::StashState;
    use crate::app::models::{ConfirmAction, Mode};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    /// Landmarks of all three panes, plus the key flow around them: focus
    /// switch, drop asks first (default No) and cancelling changes nothing.
    #[test]
    fn stash_screen_draws_panes_and_guards_drop() {
        let dir = std::env::temp_dir().join(format!("ghh-stash-render-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = crate::app::App::new(dir, true).unwrap();
        let entry = |n: usize, message: &str| crate::git::StashEntry {
            name: format!("stash@{{{n}}}"),
            hash: format!("{n:040x}"),
            date: "2026-11-20 14:02".into(),
            author: "Ann".into(),
            base: "9f8e7d6".into(),
            message: message.into(),
        };
        let mut list_state = ratatui::widgets::ListState::default();
        list_state.select(Some(0));
        app.stash = Some(StashState {
            entries: vec![entry(0, "On main: fix auth"), entry(1, "WIP on main: older")],
            list_state,
            diff_lines: vec!["+added line".into(), "-removed line".into()],
            diff_scroll: 0,
            stat_lines: vec!["1 file changed, 1 insertion(+)".into(), "a.rs | 2 +-".into()],
            diff_focused: false,
        });
        app.nav.push(Mode::Stash);

        let screen = |app: &mut crate::app::App| {
            let mut terminal = Terminal::new(TestBackend::new(160, 30)).unwrap();
            terminal.draw(|frame| crate::ui::render(frame, app)).unwrap();
            let buffer = terminal.backend().buffer().clone();
            buffer.content().iter().map(|cell| cell.symbol()).collect::<String>()
        };
        let text = screen(&mut app);
        for needle in [
            "STASHES",
            "Stashes (2)",
            "stash@{1}",
            "WIP on main: older",
            "+added line",
            "Stash Details",
            "base: 9f8e7d6",
            "Message: On main: fix auth",
            "1 file changed",
        ] {
            assert!(text.contains(needle), "{needle:?} missing");
        }

        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        app.handle_key(key(KeyCode::Tab));
        assert!(app.stash.as_ref().unwrap().diff_focused);
        app.handle_key(key(KeyCode::Down)); // scrolls the diff, selection stays
        assert_eq!(app.stash.as_ref().unwrap().diff_scroll, 1);
        assert_eq!(app.stash.as_ref().unwrap().list_state.selected(), Some(0));

        app.handle_key(key(KeyCode::Char('d')));
        assert!(matches!(app.mode(), Mode::Confirm(ConfirmAction::StashDrop { .. }, false)));
        let popup = screen(&mut app);
        assert!(popup.contains("DANGER") && popup.contains("Drop stash@{0}?"));
        assert!(popup.contains("STASHES"), "stash screen stays under the popup");
        app.handle_key(key(KeyCode::Enter)); // default No
        assert!(matches!(app.mode(), Mode::Stash));
        assert_eq!(app.stash.as_ref().unwrap().entries.len(), 2);

        app.handle_key(key(KeyCode::Esc));
        assert!(app.stash.is_none() && matches!(app.mode(), Mode::Normal));
    }
}
