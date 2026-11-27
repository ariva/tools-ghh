use ratatui::layout::Rect;
use std::sync::atomic::{AtomicBool, Ordering};

/// ASCII symbol fallback (--ascii / GHH_ASCII=1) for terminals or fonts that
/// butcher the Unicode graph glyphs. Set once at startup, read per glyph.
pub static ASCII_SYMBOLS: AtomicBool = AtomicBool::new(false);

pub fn ascii_symbols() -> bool {
    ASCII_SYMBOLS.load(Ordering::Relaxed)
}

/// Map a graph glyph to its ASCII stand-in when the fallback is active.
pub fn display_glyph(ch: char) -> char {
    if !ascii_symbols() {
        return ch;
    }
    match ch {
        '\u{25CF}' => 'o',               // ● commit
        '\u{25CB}' => 'O',               // ○ HEAD commit
        '\u{25BC}' => 'v',               // ▼ merge (expanded)
        '\u{25BA}' => '>',               // ► merge (collapsed)
        '\u{256D}' | '\u{256F}' => '/',  // ╭ ╯
        '\u{256E}' | '\u{2570}' => '\\', // ╮ ╰
        '\u{2502}' | '\u{250A}' => '|',  // │ ┊
        '\u{2500}' => '-',               // ─
        '\u{2524}' => '+',               // ┤
        '\u{2191}' => '^',               // ↑
        '\u{2193}' => 'v',               // ↓
        other => other,
    }
}

pub fn centered_rect(percent_x: u16, height: u16, r: Rect) -> Rect {
    let popup_width = r.width * percent_x / 100;
    let popup_x = r.x + r.width.saturating_sub(popup_width) / 2;
    let popup_height = height.min(r.height);
    let popup_y = r.y + r.height.saturating_sub(popup_height) / 2;
    Rect::new(popup_x, popup_y, popup_width, popup_height)
}

pub fn truncate(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars.saturating_sub(3)).collect();
        format!("{truncated}...")
    }
}

/// A file path cut to `room` characters with the file name kept, for every
/// pane that lists files: `.../` and the trailing directories that fit as a
/// whole (`.../features/staging.rs`). A file name too long even for that
/// keeps its end (`...ery_long_name.rs`), where the extension is.
pub fn fit_path(path: &str, room: usize) -> String {
    const MARK: &str = ".../";
    let length = path.chars().count();
    if length <= room {
        return path.to_string();
    }
    // First `/` from the left whose remainder fits = the longest tail.
    let tail = path
        .match_indices('/')
        .map(|(at, _)| &path[at + 1..])
        .find(|tail| tail.chars().count() + MARK.len() <= room);
    if let Some(tail) = tail {
        return format!("{MARK}{tail}");
    }
    // No room for the mark itself: the last characters say more than dots.
    let mark = if room > 3 { "..." } else { "" };
    let kept: String = path.chars().skip(length - (room - mark.len())).collect();
    format!("{mark}{kept}")
}

/// Like `truncate`, but when `ranges` (char ranges of search hits) would be
/// cut off, the window shifts so the FIRST hit stays visible, with a leading
/// "..." marking the skipped prefix. Result is at most `max_chars` chars.
pub fn truncate_keeping_match(s: &str, max_chars: usize, ranges: &[(usize, usize)]) -> String {
    let count = s.chars().count();
    let plain_visible = if count <= max_chars {
        max_chars
    } else {
        max_chars.saturating_sub(3)
    };
    let first = match ranges.first() {
        Some(&(start, _)) if start >= plain_visible => (start, ranges[0].1),
        _ => return truncate(s, max_chars),
    };
    if max_chars < 7 {
        return truncate(s, max_chars);
    }
    // Window: lead-in of a quarter width before the hit, "..." on the left
    // (and right if text continues past the window).
    let body = max_chars - 3;
    let mut window_start = first.0.saturating_sub(body / 4);
    window_start = window_start.min(count.saturating_sub(body));
    let tail_cut = window_start + body < count;
    let take = if tail_cut { body - 3 } else { body };
    let shown: String = s.chars().skip(window_start).take(take).collect();
    if tail_cut {
        format!("...{shown}...")
    } else {
        format!("...{shown}")
    }
}

/// Split `text` into spans: `base` style everywhere, `hl` over the given
/// char ranges (as produced by `search::match_ranges`).
pub fn highlight_spans(
    text: &str,
    base: ratatui::style::Style,
    hl: ratatui::style::Style,
    ranges: &[(usize, usize)],
) -> Vec<ratatui::text::Span<'static>> {
    use ratatui::text::Span;
    if ranges.is_empty() {
        return vec![Span::styled(text.to_string(), base)];
    }
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut pos = 0usize;
    for &(start, end) in ranges {
        let start = start.min(chars.len());
        let end = end.min(chars.len());
        if start > pos {
            spans.push(Span::styled(
                chars[pos..start].iter().collect::<String>(),
                base,
            ));
        }
        if end > start {
            spans.push(Span::styled(
                chars[start..end].iter().collect::<String>(),
                hl,
            ));
        }
        pos = pos.max(end);
    }
    if pos < chars.len() {
        spans.push(Span::styled(chars[pos..].iter().collect::<String>(), base));
    }
    spans
}

#[cfg(test)]
mod highlight_tests {
    use super::{fit_path, highlight_spans, truncate_keeping_match};
    use ratatui::style::{Color, Style};

    #[test]
    fn fit_path_keeps_the_file_name() {
        let path = "crates/ghh/src/app/features/staging.rs";
        assert_eq!(fit_path(path, 80), path, "fits: untouched");
        assert_eq!(fit_path(path, path.len()), path);
        // Whole directories from the right, as many as fit.
        assert_eq!(fit_path(path, 30), ".../app/features/staging.rs");
        assert_eq!(fit_path(path, 24), ".../features/staging.rs");
        assert_eq!(fit_path(path, 14), ".../staging.rs");
        // The name alone does not fit: its end, with the extension.
        assert_eq!(fit_path(path, 10), "...ging.rs");
        assert_eq!(fit_path("a_very_long_file_name.rs", 12), "...e_name.rs");
        // Never longer than the room, down to nothing.
        for room in 0..path.len() {
            assert!(fit_path(path, room).chars().count() <= room, "room {room}");
        }
        assert_eq!(fit_path(path, 3), ".rs");
        // Characters, not bytes.
        assert_eq!(
            fit_path("katalogas/ąčęėįšųū/failas.rs", 14),
            ".../failas.rs"
        );
    }

    #[test]
    fn truncate_keeping_match_windows_to_first_hit() {
        let text = "prefix prefix prefix TARGET tail";
        // Hit fits in plain truncation -> plain truncate.
        assert_eq!(truncate_keeping_match(text, 40, &[(21, 27)]), text);
        // Hit would be cut off -> window shifts, leading "...", hit visible.
        let windowed = truncate_keeping_match(text, 16, &[(21, 27)]);
        assert!(windowed.contains("TARGET"), "hit visible in {windowed:?}");
        assert!(windowed.starts_with("..."));
        assert!(windowed.chars().count() <= 16);
        // No hits -> plain truncate with trailing "...".
        assert_eq!(truncate_keeping_match(text, 10, &[]), "prefix ...");
        // Degenerate width falls back to plain truncate without panicking.
        assert_eq!(
            truncate_keeping_match(text, 4, &[(21, 27)]).chars().count(),
            4
        );
    }

    #[test]
    fn highlight_spans_splits_at_ranges() {
        let base = Style::default();
        let hl = Style::default().fg(Color::Yellow);
        let spans = highlight_spans("fix auth bug", base, hl, &[(4, 8)]);
        let parts: Vec<(&str, bool)> = spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style == hl))
            .collect();
        assert_eq!(
            parts,
            vec![("fix ", false), ("auth", true), (" bug", false)]
        );
        // No ranges → single base span, text unchanged.
        let plain = highlight_spans("abc", base, hl, &[]);
        assert_eq!(plain.len(), 1);
        assert_eq!(plain[0].content.as_ref(), "abc");
        // Range clamped to text length.
        let clamped = highlight_spans("ab", base, hl, &[(1, 99)]);
        let text: String = clamped.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "ab");
    }
}

fn cell_has_text(buffer: &ratatui::buffer::Buffer, column: u16, row: u16) -> bool {
    buffer
        .cell((column, row))
        .is_some_and(|cell| cell.symbol().chars().any(|ch| !ch.is_whitespace()))
}

/// Word bounds around (column,row) within the panel columns: expands over
/// contiguous non-whitespace cells. None when the clicked cell is blank.
pub fn word_bounds_at(
    buffer: &ratatui::buffer::Buffer,
    panel_start_column: u16,
    panel_end_column: u16,
    column: u16,
    row: u16,
) -> Option<(u16, u16)> {
    if !cell_has_text(buffer, column, row) {
        return None;
    }
    let mut start = column;
    let mut end = column;
    while start > panel_start_column && cell_has_text(buffer, start - 1, row) {
        start -= 1;
    }
    while end < panel_end_column && cell_has_text(buffer, end + 1, row) {
        end += 1;
    }
    Some((start, end))
}

/// End column of the next word to the right of `from` on `row` (skips the
/// whitespace gap first). None when only blanks remain up to the panel edge.
pub fn next_word_end(
    buffer: &ratatui::buffer::Buffer,
    panel_end_column: u16,
    row: u16,
    from: u16,
) -> Option<u16> {
    let mut next = from.saturating_add(1);
    while next <= panel_end_column && !cell_has_text(buffer, next, row) {
        next = next.saturating_add(1);
    }
    if next > panel_end_column {
        return None;
    }
    let mut end = next;
    while end < panel_end_column && cell_has_text(buffer, end.saturating_add(1), row) {
        end = end.saturating_add(1);
    }
    Some(end)
}

#[cfg(test)]
mod selection_geometry_tests {
    use super::{next_word_end, word_bounds_at};
    use ratatui::{Terminal, backend::TestBackend, widgets::Paragraph};

    fn buffer_with(text: &str) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(30, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| frame.render_widget(Paragraph::new(text), frame.area()))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    #[test]
    fn word_bounds_expand_over_the_word_only() {
        let buffer = buffer_with("fix search bug");
        assert_eq!(
            word_bounds_at(&buffer, 0, 29, 5, 0),
            Some((4, 9)),
            "inside 'search'"
        );
        assert_eq!(
            word_bounds_at(&buffer, 0, 29, 0, 0),
            Some((0, 2)),
            "'fix' at edge"
        );
        assert_eq!(word_bounds_at(&buffer, 0, 29, 3, 0), None, "blank cell");
    }

    #[test]
    fn next_word_end_skips_gap_and_stops_at_word_end() {
        let buffer = buffer_with("fix search bug");
        assert_eq!(
            next_word_end(&buffer, 29, 0, 2),
            Some(9),
            "from 'fix' to end of 'search'"
        );
        assert_eq!(
            next_word_end(&buffer, 29, 0, 9),
            Some(13),
            "to end of 'bug'"
        );
        assert_eq!(
            next_word_end(&buffer, 29, 0, 13),
            None,
            "nothing right of the last word"
        );
    }
}
