use crate::app::{App, VisibleRow};
use crate::git::{CommitInfo, CommitStats};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

pub fn render_details(
    frame: &mut Frame,
    commit: Option<&CommitInfo>,
    stats: Option<&CommitStats>,
    merged_via: Option<&str>,
    search_terms: Option<&crate::app::features::search::ExecutedTerms>,
    area: Rect,
) {
    use crate::app::features::search::match_ranges;
    use crate::ui::helpers::highlight_spans;
    // Marks the parts the executed search hit (yellow) so the user sees WHY
    // this commit matched.
    let hl = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let marked = |text: &str, terms: &[&str]| -> Vec<Span<'static>> {
        match search_terms {
            Some(_) => highlight_spans(text, Style::default(), hl, &match_ranges(text, terms)),
            None => vec![Span::raw(text.to_string())],
        }
    };
    let block = Block::default()
        .title(" Details ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let content = match commit {
        None => Paragraph::new("No commit selected").block(block),
        Some(c) => {
            let author_text = if c.author_email.is_empty() {
                c.author_name.clone()
            } else {
                format!("{} <{}>", c.author_name, c.author_email)
            };
            let empty = crate::app::features::search::ExecutedTerms::default();
            let t = search_terms.unwrap_or(&empty);
            let mut hash_line = vec![Span::styled(
                "Hash:    ",
                Style::default().fg(Color::Yellow),
            )];
            hash_line.extend(marked(&c.hash, &t.for_hash()));
            let mut author_line = vec![Span::styled(
                "Author:  ",
                Style::default().fg(Color::Yellow),
            )];
            author_line.extend(marked(&author_text, &t.for_author()));
            let mut message_line = vec![Span::styled(
                "Message: ",
                Style::default().fg(Color::Yellow),
            )];
            message_line.extend(marked(&c.message, &t.for_message()));
            let mut lines = vec![
                Line::from(hash_line),
                Line::from(author_line),
                {
                    let date_text = c.date_display_text().into_owned();
                    let mut date_line = vec![Span::styled(
                        "Date:    ",
                        Style::default().fg(Color::Yellow),
                    )];
                    match search_terms {
                        Some(terms) => date_line.extend(highlight_spans(
                            &date_text,
                            Style::default(),
                            hl,
                            &terms.date_ranges(&date_text),
                        )),
                        None => date_line.push(Span::raw(date_text)),
                    }
                    Line::from(date_line)
                },
                Line::from(message_line),
            ];

            if let Some(text) = merged_via {
                lines.push(Line::from(vec![
                    Span::styled("Merged:  ", Style::default().fg(Color::Yellow)),
                    Span::raw(text.to_string()),
                ]));
            }

            if let Some(s) = stats {
                let total_files = s.files_added + s.files_modified + s.files_deleted;
                lines.push(Line::from(vec![
                    Span::styled("Changes: ", Style::default().fg(Color::Yellow)),
                    Span::styled(
                        format!("+{} ", s.files_added),
                        Style::default().fg(Color::Green),
                    ),
                    Span::styled(
                        format!("~{} ", s.files_modified),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::styled(
                        format!("-{}  ", s.files_deleted),
                        Style::default().fg(Color::Red),
                    ),
                    Span::styled(
                        format!(
                            "({total_files} file{})",
                            if total_files == 1 { "" } else { "s" }
                        ),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::raw("   "),
                    Span::styled(
                        format!("+{}", s.lines_inserted),
                        Style::default().fg(Color::Green),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        format!("-{}", s.lines_deleted),
                        Style::default().fg(Color::Red),
                    ),
                    Span::styled(" lines", Style::default().fg(Color::DarkGray)),
                ]));
            } else {
                lines.push(Line::from(Span::styled(
                    "Changes: loading...",
                    Style::default().fg(Color::DarkGray),
                )));
            }

            Paragraph::new(lines).block(block)
        }
    };

    frame.render_widget(content, area);
}

pub fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(Paragraph::new(status_line_content(app)), area);
}

/// Compute the status bar content for the main commit-list view.
pub fn status_line_content(app: &App) -> Line<'static> {
    let loading_line = || {
        Line::from(vec![
            Span::styled("Loading ", Style::default().fg(Color::Yellow)),
            Span::styled(
                format!("{} commits shown", app.commits.len()),
                Style::default().fg(Color::White),
            ),
            Span::styled(
                "  /  full history still reading...",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    };

    if let Some(sel) = app.list_state.selected()
        && let Some(VisibleRow::CollapsedBranch { count, .. }) = app.visible.get(sel)
    {
        let count = *count;
        return Line::from(vec![
            Span::styled(
                "\u{25BA} ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{count} commit{} hidden", if count == 1 { "" } else { "s" }),
                Style::default().fg(Color::White),
            ),
            Span::styled(
                "  \u{2014}  Space / \u{2190}\u{2192} to expand",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
    }

    // Search segment is DERIVED from live state — it disappears the moment
    // matches are cleared (Reset, F, new empty search), so it can never go
    // stale the way a stored status string did.
    let mut spans: Vec<Span> = Vec::new();
    if !app.search.matches.is_empty() {
        spans.push(Span::styled(
            format!(
                "Search {}/{}",
                app.search.match_pos + 1,
                app.search.matches.len()
            ),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        if app.search.filter_active {
            spans.push(Span::styled(
                " [filtered]",
                Style::default().fg(Color::Yellow),
            ));
        }
        spans.push(Span::styled(
            "  n/p: next/prev match  f: toggle filter  F: clear search",
            Style::default().fg(Color::DarkGray),
        ));
    }

    if let Some(msg) = &app.status_message {
        let style = if msg.starts_with("Error") || msg.starts_with("Commit error") {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Green)
        };
        if !spans.is_empty() {
            spans.push(Span::styled("  |  ", Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::styled(msg.clone(), style));
        if app.history_loading {
            spans.push(Span::styled("  |  ", Style::default().fg(Color::DarkGray)));
            spans.push(Span::styled(
                match app.history_total_count {
                    Some(total) => format!(
                        "loading full history... {} / {} commits",
                        app.commits.len(),
                        total
                    ),
                    None => format!("loading full history... {} commits", app.commits.len()),
                },
                Style::default().fg(Color::Yellow),
            ));
        }
        return Line::from(spans);
    }

    if spans.is_empty() && app.history_loading {
        return loading_line();
    }

    Line::from(spans)
}
