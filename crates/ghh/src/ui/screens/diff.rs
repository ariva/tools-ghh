use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

pub fn scroll_label(scroll: usize, total_lines: usize, viewport_height: u16) -> String {
    if total_lines == 0 {
        return "0% - 0/0".to_string();
    }

    let visible = viewport_height.saturating_sub(2) as usize;
    if visible == 0 {
        let pos = scroll.min(total_lines.saturating_sub(1)) + 1;
        let pct = (pos * 100) / total_lines;
        return format!("{pct}% - {pos}:{pos} / {total_lines}");
    }

    let start = scroll.min(total_lines.saturating_sub(1)) + 1;
    let end = (scroll + visible).min(total_lines);
    let pct = (end * 100) / total_lines;
    format!("{pct}% - {start}:{end} / {total_lines}")
}

/// Style only the diff lines that fit the visible viewport.
///
/// `area_height` is the full widget height, borders included. Callers render the
/// result at scroll offset 0 — the window itself carries the offset, so the cost
/// of a frame stays proportional to the viewport instead of the whole diff.
pub fn render_diff_window(lines: &[String], scroll: usize, area_height: u16) -> Vec<Line<'static>> {
    let visible = area_height.saturating_sub(2) as usize;
    if visible == 0 {
        return Vec::new();
    }
    let start = scroll.min(lines.len());
    let end = start.saturating_add(visible).min(lines.len());
    render_diff_lines(&lines[start..end])
}

fn render_diff_lines(lines: &[String]) -> Vec<Line<'static>> {
    lines
        .iter()
        .map(|line| {
            let style = if line == "Loading commit data..." {
                Style::default().fg(Color::DarkGray)
            } else if line.starts_with('+') && !line.starts_with("+++") {
                Style::default().fg(Color::Green)
            } else if line.starts_with('-') && !line.starts_with("---") {
                Style::default().fg(Color::Red)
            } else if line.starts_with('@') {
                Style::default().fg(Color::Cyan)
            } else if line.starts_with("diff")
                || line.starts_with("index")
                || line.starts_with("+++")
                || line.starts_with("---")
            {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            Line::from(Span::styled(sanitize_diff_line(line), style))
        })
        .collect()
}

fn sanitize_diff_line(line: &str) -> String {
    let mut sanitized = String::with_capacity(line.len());
    for character in line.chars() {
        match character {
            '\t' => sanitized.push_str("    "),
            character if character.is_control() => sanitized.push('\u{fffd}'),
            character => sanitized.push(character),
        }
    }
    sanitized
}

#[cfg(test)]
mod tests {
    use super::{render_diff_window, sanitize_diff_line};

    fn sample(count: usize) -> Vec<String> {
        (0..count).map(|i| format!("line {i}")).collect()
    }

    fn texts(lines: &[ratatui::text::Line<'static>]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn sanitizes_terminal_control_characters() {
        assert_eq!(sanitize_diff_line("\tline\x1b[31m"), "    line\u{fffd}[31m");
    }

    #[test]
    fn window_starts_at_the_scroll_offset_and_fits_the_borders() {
        let lines = sample(100);
        // height 6 => 4 rows inside the block borders
        assert_eq!(
            texts(&render_diff_window(&lines, 10, 6)),
            ["line 10", "line 11", "line 12", "line 13"]
        );
    }

    #[test]
    fn window_stops_at_the_last_line() {
        let lines = sample(12);
        assert_eq!(
            texts(&render_diff_window(&lines, 10, 10)),
            ["line 10", "line 11"]
        );
    }

    #[test]
    fn window_is_empty_past_the_end_or_without_room() {
        let lines = sample(12);
        assert!(render_diff_window(&lines, 99, 10).is_empty());
        assert!(render_diff_window(&lines, 0, 2).is_empty());
        assert!(render_diff_window(&[], 0, 10).is_empty());
    }
}
