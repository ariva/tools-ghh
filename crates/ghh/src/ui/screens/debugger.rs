use crate::app::App;
use crate::events::models::AppEvent;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

/// The debugger's display gate (settings `debug`): traces below the minimum
/// level are hidden; app events (KEY/SCRN/ACT) ride the show_events switch.
fn event_visible(ev: &AppEvent, min: crate::events::TraceLevel, show_events: bool) -> bool {
    match ev {
        AppEvent::Trace { level, .. } => *level >= min,
        _ => show_events,
    }
}

pub fn render_debugger(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title bar
            Constraint::Min(3),    // event log
            Constraint::Length(1), // status / hint
        ])
        .split(area);

    // ── Title ────────────────────────────────────────────────────────────────
    let title = Paragraph::new(
        "DEBUGGER  \u{2502}  j/k \u{2191}\u{2193}: scroll  \u{2502}  g/G: top/bottom  \u{2502}  c: clear  \u{2502}  Esc/Backspace/D: close",
    )
    .style(Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD))
    .alignment(ratatui::layout::Alignment::Center);
    frame.render_widget(title, chunks[0]);

    // ── Event list ────────────────────────────────────────────────────────────
    let bus = &app.event_bus;
    let events: Vec<&AppEvent> = bus
        .events()
        .iter()
        .filter(|ev| event_visible(ev, app.trace_min_level, app.debug_show_events))
        .collect();
    let total = events.len();
    let list_height = chunks[1].height.saturating_sub(2) as usize; // minus borders

    let start = total.saturating_sub(list_height).saturating_sub(bus.scroll);

    let items: Vec<ListItem<'static>> = events
        .iter()
        .skip(start)
        .take(list_height)
        .map(|ev| {
            // Show "HH:MM:SS" from the local wall-clock string.
            let time_str = &ev.at().local[11..]; // slice off "YYYY-MM-DD "
            let tag = ev.type_tag();
            let desc = ev.description();

            let (tag_color, desc_color) = match ev {
                AppEvent::KeyPress { .. } => (Color::DarkGray, Color::Gray),
                AppEvent::ScreenChange { .. } => (Color::Cyan, Color::White),
                AppEvent::Action { .. } => (Color::Yellow, Color::White),
                AppEvent::Trace { level, .. } => {
                    use crate::events::models::TraceLevel;
                    let c = match level {
                        TraceLevel::Trace => Color::Cyan,
                        TraceLevel::Debug => Color::Blue,
                        TraceLevel::Info => Color::White,
                        TraceLevel::Warn => Color::Yellow,
                        TraceLevel::Error => Color::Red,
                    };
                    (c, c)
                }
            };

            let line = Line::from(vec![
                Span::styled(
                    format!("{} ", time_str),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("[{tag}]"),
                    Style::default().fg(tag_color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(desc, Style::default().fg(desc_color)),
            ]);
            ListItem::new(line)
        })
        .collect();

    let level_tag = match app.trace_min_level {
        crate::events::TraceLevel::Trace => String::new(), // default: show all
        min => format!(" traces {}+ ", min.setting_str()),
    };
    let block = Block::default()
        .title(format!(" Events ({total}){level_tag}"))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    frame.render_widget(List::new(items).block(block), chunks[1]);

    // ── Status bar ───────────────────────────────────────────────────────────
    let showing_end = (start + list_height).min(total);
    let status = if total == 0 {
        "No events yet. Interact with the app to generate events.".to_string()
    } else {
        format!(
            "Showing {}-{} of {}  \u{2502}  G: latest  g: oldest",
            start + 1,
            showing_end,
            total
        )
    };
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

#[cfg(test)]
mod tests {
    use super::event_visible;
    use crate::events::models::{AppEvent, EventTime, ScreenId, TraceLevel};

    fn trace(level: TraceLevel) -> AppEvent {
        AppEvent::Trace {
            level,
            msg: "m".into(),
            at: EventTime::now(),
        }
    }

    /// min INFO: Info/Warn/Error traces show, Trace/Debug hide. App events
    /// ride the show_events switch, independent of the level. Defaults
    /// (TRACE + events on) show everything.
    #[test]
    fn display_gate_filters_traces_by_level_and_events_by_switch() {
        let min = TraceLevel::Info;
        assert!(!event_visible(&trace(TraceLevel::Trace), min, true));
        assert!(!event_visible(&trace(TraceLevel::Debug), min, true));
        assert!(event_visible(&trace(TraceLevel::Info), min, true));
        assert!(event_visible(&trace(TraceLevel::Error), min, true));
        let key = AppEvent::KeyPress {
            display: "j".into(),
            screen: ScreenId::CommitList,
            at: EventTime::now(),
        };
        assert!(
            event_visible(&key, TraceLevel::Error, true),
            "level never hides events"
        );
        assert!(
            !event_visible(&key, TraceLevel::Trace, false),
            "switch hides events"
        );
        assert!(
            event_visible(&trace(TraceLevel::Error), min, false),
            "switch spares traces"
        );
        for level in [TraceLevel::Trace, TraceLevel::Debug, TraceLevel::Error] {
            assert!(
                event_visible(&trace(level), TraceLevel::Trace, true),
                "default shows all"
            );
        }
    }
}
