use super::details::{render_details, render_status};
use super::diff::{render_diff_window, scroll_label};
use crate::app::{App, CommitListFocus, CommitListView, SquashState, VisibleRow};
use crate::git;
use crate::ui::helpers::truncate;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

/// Spans for one commit row — the hot path of the list, extracted pure-ish
/// (reads App, writes nothing) so colors/highlights are testable without a
/// TestBackend.
#[allow(clippy::too_many_arguments)]
pub(crate) fn commit_row_spans(
    app: &App,
    row_idx: usize,
    idx: usize,
    prefix_override: Option<&(String, Vec<u8>)>,
    depth: usize,
    row_count_width: usize,
    message_width: usize,
    file_statuses: &std::collections::HashMap<usize, char>,
    file_letters_active: bool,
    squash_ref: Option<&SquashState>,
) -> Vec<Span<'static>> {
    // Graph, HEAD mark and ref labels belong to the row; the commit
    // text follows the move-mode preview (same commit when not moving).
    let row_commit = &app.commits[idx];
    let c = &app.commits[app.display_idx(idx)];
    let (prefix, prefix_lanes): (&str, &[u8]) = match prefix_override {
        Some((p, l)) => (p.as_str(), l.as_slice()),
        None => (
            row_commit.graph_prefix.as_str(),
            row_commit.graph_prefix_lanes.as_slice(),
        ),
    };
    let is_collapsed = app.collapsed_merges.contains(&row_commit.hash);
    // Search hit: gitk-style bold row, cyan index (matches are
    // sorted commit indices, so this is a binary search per row).
    let is_search_match =
        !app.search.matches.is_empty() && app.search.matches.binary_search(&idx).is_ok();
    let mut spans: Vec<Span<'static>> = Vec::new();

    spans.push(Span::styled(
        format!(
            "{:>width$} ",
            app.row_number(row_idx),
            width = row_count_width
        ),
        if is_search_match {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        },
    ));
    // File-search status prefix (A/M/D/R) right after the row
    // number; unmatched rows get padding so columns stay aligned.
    if file_letters_active {
        match file_statuses.get(&idx) {
            Some(&status) => spans.push(Span::styled(
                format!("{status} "),
                Style::default()
                    .fg(file_status_color(status))
                    .add_modifier(Modifier::BOLD),
            )),
            None => spans.push(Span::raw("  ")),
        }
    }

    if app.commit_list_view != CommitListView::Simple {
        // Nested rows indent by their depth; the first row of a
        // deeper group gets a connector curving out of its parent.
        if depth > 0 {
            let prev_depth = row_idx
                .checked_sub(1)
                .and_then(|r| app.visible.get(r))
                .map(row_depth)
                .unwrap_or(0);
            let color = prefix_lanes
                .first()
                .map(|&lane| lane_color(lane))
                .unwrap_or(Color::DarkGray);
            let mut indent = "  ".repeat(depth);
            if depth > prev_depth {
                indent.truncate(indent.len() - 2);
                indent.push('\u{2570}');
                indent.push(' ');
            }
            spans.push(Span::styled(indent, Style::default().fg(color)));
        }
        let is_head = app.head_hash.as_deref() == Some(row_commit.hash.as_str());
        let graph_spans = build_graph_spans(prefix, prefix_lanes, is_collapsed, is_head);
        if !graph_spans.is_empty() {
            spans.extend(graph_spans);
            spans.push(Span::raw(" "));
        }
    }

    // Matched rows keep their normal colors; only the parts the
    // search actually hit are marked (red on yellow hash, yellow
    // elsewhere).
    let terms = if is_search_match {
        app.search.executed_terms.as_ref()
    } else {
        None
    };
    let hash_text = c.short_hash().to_string();
    if let Some(t) = terms {
        spans.extend(crate::ui::helpers::highlight_spans(
            &hash_text,
            Style::default().fg(Color::Yellow),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            &crate::app::features::search::match_ranges(&hash_text, &t.for_hash()),
        ));
        spans.push(Span::raw(" "));
    } else {
        spans.push(Span::styled(
            format!("{hash_text} "),
            Style::default().fg(Color::Yellow),
        ));
    }
    spans.push(Span::styled(
        format!("{} ", c.date_display_text()),
        Style::default().fg(Color::Cyan),
    ));
    let author_text = format!("{:<16}", truncate(&c.author_name, 16));
    if let Some(t) = terms {
        spans.extend(crate::ui::helpers::highlight_spans(
            &author_text,
            Style::default().fg(Color::Green),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
            &crate::app::features::search::match_ranges(&author_text, &t.for_author()),
        ));
        spans.push(Span::raw(" "));
    } else {
        spans.push(Span::styled(
            format!("{author_text} "),
            Style::default().fg(Color::Green),
        ));
    }

    let mut label_width = 0usize;
    if let Some(labels) = app.ref_labels.get(&row_commit.hash) {
        for label in labels {
            let span = ref_label_span(label);
            label_width += span.content.chars().count() + 1;
            spans.push(span);
            spans.push(Span::raw(" "));
        }
    }
    let avail = message_width.saturating_sub(label_width);
    if let Some(t) = terms {
        let full_ranges = crate::app::features::search::match_ranges(&c.message, &t.for_message());
        let message_text =
            crate::ui::helpers::truncate_keeping_match(&c.message, avail, &full_ranges);
        spans.extend(crate::ui::helpers::highlight_spans(
            &message_text,
            Style::default(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
            &crate::app::features::search::match_ranges(&message_text, &t.for_message()),
        ));
    } else {
        spans.push(Span::raw(truncate(&c.message, avail)));
    }

    if let Some(sq) = squash_ref {
        if idx == sq.base_idx {
            spans.push(Span::styled(
                "  [BASE]",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        } else if sq.squash_set.contains(&idx) {
            spans.push(Span::styled(
                "  [S]",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ));
        }
    }

    if app
        .move_state
        .as_ref()
        .is_some_and(|m| m.chain[m.pos] == idx)
    {
        spans.push(Span::styled(
            "  [MOVE]",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ));
    }

    spans
}

/// Left panel: the commit list — viewport, rows (commit_row_spans),
/// breadcrumb title, focus border.
fn render_commit_list_panel(frame: &mut Frame, app: &mut App, rect: Rect) {
    const UNCOUNTED_INDEX_WIDTH: usize = 7;
    let row_count_width = app
        .history_total_count
        .unwrap_or(app.visible.len())
        .to_string()
        .len()
        .max(UNCOUNTED_INDEX_WIDTH);
    let viewport_height = rect.height.saturating_sub(2) as usize;
    let (start, end, selected_local) = commit_viewport(
        app.list_state.selected(),
        app.list_state.offset(),
        app.visible.len(),
        viewport_height,
    );
    app.ensure_graph_prefixes_for_visible_range(start, end);
    *app.list_state.offset_mut() = start;
    let graph_width = graph_render_width(app, start, end);
    let message_width = commit_message_width(rect.width, row_count_width, graph_width);
    // A/M/D/R letters for matched rows of a file search: computed against
    // &mut app BEFORE the immutable row borrow below, viewport rows only,
    // cached by hash inside search_file_status.
    let file_statuses: std::collections::HashMap<usize, char> =
        if app.search.active_file_patterns.is_empty() {
            std::collections::HashMap::new()
        } else {
            let match_indices: Vec<usize> = (start..end.min(app.visible.len()))
                .filter_map(|row| match app.visible.get(row) {
                    Some(VisibleRow::Commit { idx, .. }) => Some(*idx),
                    _ => None,
                })
                .filter(|idx| app.search.matches.binary_search(idx).is_ok())
                .collect();
            match_indices
                .into_iter()
                .filter_map(|idx| app.search_file_status(idx).map(|status| (idx, status)))
                .collect()
        };
    let file_letters_active = !app.search.active_file_patterns.is_empty();
    let message_width = message_width.saturating_sub(if file_letters_active { 2 } else { 0 });
    let squash_ref: Option<&SquashState> = app.squash.as_ref();

    let items: Vec<ListItem<'static>> = app
        .visible
        .iter()
        .enumerate()
        .skip(start)
        .take(end.saturating_sub(start))
        .map(|(row_idx, row)| match row {
            VisibleRow::Commit { idx, prefix_override, depth } => {
                ListItem::new(Line::from(commit_row_spans(
                    app,
                    row_idx,
                    *idx,
                    prefix_override.as_ref(),
                    *depth,
                    row_count_width,
                    message_width,
                    &file_statuses,
                    file_letters_active,
                    squash_ref,
                )))
            }
            VisibleRow::CollapsedBranch { count, .. } => ListItem::new(Line::from(
                Span::styled(
                    format!(
                        "   \u{00B7}\u{00B7}\u{00B7}  [{count} commit{} hidden \u{2014} Space/\u{2190}\u{2192} to expand]",
                        if *count == 1 { "" } else { "s" }
                    ),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ),
            )),
            VisibleRow::WorkingTree => ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:>width$} ", 0, width = row_count_width),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("\u{25CF} {}", app.working_tree.title()),
                    Style::default()
                        .fg(Color::LightMagenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "  Enter: staging",
                    Style::default().fg(Color::DarkGray),
                ),
            ])),
            VisibleRow::DrillHeader {
                idx,
                prefix_override,
            } => {
                let c = &app.commits[*idx];
                let (prefix, prefix_lanes): (&str, &[u8]) = match prefix_override {
                    Some((p, l)) => (p.as_str(), l.as_slice()),
                    None => (c.graph_prefix.as_str(), c.graph_prefix_lanes.as_slice()),
                };
                let mut spans: Vec<Span<'static>> = vec![Span::styled(
                    format!("{:>width$} ", app.row_number(row_idx), width = row_count_width),
                    Style::default().fg(Color::DarkGray),
                )];
                if app.commit_list_view != CommitListView::Simple {
                    let graph_spans = build_graph_spans(prefix, prefix_lanes, false, false);
                    if !graph_spans.is_empty() {
                        spans.extend(graph_spans);
                        spans.push(Span::raw(" "));
                    }
                }
                spans.extend([
                    Span::styled(
                        format!("{} ", c.short_hash()),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::styled(
                        format!("{} ", c.date_display_text()),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        format!("{:<16} ", truncate(&c.author_name, 16)),
                        Style::default().fg(Color::Green),
                    ),
                    Span::raw(truncate(&c.message, message_width)),
                ]);
                ListItem::new(Line::from(spans))
            }
        })
        .collect();

    // Breadcrumb while drilled into collapsed branches: Commits ▸ ab12cd34 (12) ▸ …
    // Geometry comes from app.breadcrumb() so the "…" pagers and hash
    // segments are clickable — mouse hit-testing reads the SAME layout.
    let list_title = if app.drill_stack.is_empty() {
        format!(" Commits {}", app.list_title_suffix())
    } else {
        format!(
            "{}{}",
            app.breadcrumb(rect.width).text,
            app.list_title_suffix()
        )
    };
    let list = List::new(items)
        .block(
            Block::default()
                .title(list_title)
                .borders(Borders::ALL)
                .border_style(if app.commit_list_focus == CommitListFocus::List {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::DarkGray)
                }),
        )
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::REVERSED)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let mut render_state = ratatui::widgets::ListState::default().with_selected(selected_local);
    *render_state.offset_mut() = 0;
    frame.render_stateful_widget(list, rect, &mut render_state);
}

/// Right panels (only with the details view open): Commit Info + Diff,
/// borrowing the cached diff rather than cloning it — those vectors can hold
/// tens of thousands of lines and this runs on every frame.
fn render_diff_panels(
    frame: &mut Frame,
    app: &mut App,
    rect: Rect,
    selected: Option<&crate::git::CommitInfo>,
    stats: Option<&crate::git::CommitStats>,
) {
    if app.show_commit_diff {
        let diff_sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(app.splits.diff_info_pct),
                Constraint::Percentage(100 - app.splits.diff_info_pct),
            ])
            .split(rect);
        // Borrow the cached diff rather than cloning it: these vectors can hold
        // tens of thousands of lines and this runs on every frame.
        // The working tree row has no commit; its patch sits in the same
        // caches under its own key.
        let on_working_tree = app.working_tree_selected();
        let diff_key = app.selected_diff_key().map(str::to_string);
        let placeholder = match diff_key {
            Some(_) => vec!["Loading commit data...".to_string()],
            None => vec!["No commit selected".to_string()],
        };
        let diff_lines: &[String] = diff_key
            .as_ref()
            .and_then(|key| app.commit_diff_cache.get(key))
            .map(Vec::as_slice)
            .unwrap_or(&placeholder);
        let no_files = Vec::new();
        let diff_files = diff_key
            .as_ref()
            .and_then(|key| app.commit_diff_file_cache.get(key))
            .unwrap_or(&no_files);
        let info_terms = app
            .selected_commit_index()
            .filter(|idx| app.search.matches.binary_search(idx).is_ok())
            .and(app.search.executed_terms.as_ref());
        let info_lines = if on_working_tree {
            working_tree_info_lines(&app.working_tree, diff_files)
        } else {
            render_commit_info_lines(selected, stats, diff_files, info_terms)
        };
        let info_meta = scroll_label(
            app.commit_diff_info_scroll,
            info_lines.len(),
            diff_sections[0].height,
        );
        let info_para = Paragraph::new(info_lines)
            .block(
                Block::default()
                    .title(format!(
                        " {} [{info_meta}] ",
                        if on_working_tree {
                            "Uncommitted changes"
                        } else {
                            "Commit Info"
                        }
                    ))
                    .borders(Borders::ALL)
                    .border_style(if app.commit_list_focus == CommitListFocus::DiffInfo {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    }),
            )
            .scroll((app.commit_diff_info_scroll as u16, 0));
        frame.render_widget(Clear, diff_sections[0]);
        frame.render_widget(info_para, diff_sections[0]);

        let diff_title = selected
            .as_ref()
            .map(|c| format!(" Diff: {} ", c.short_hash()))
            .unwrap_or_else(|| {
                if on_working_tree {
                    " Diff: uncommitted changes ".to_string()
                } else {
                    " Diff ".to_string()
                }
            });
        let diff_meta = scroll_label(
            app.commit_diff_scroll,
            diff_lines.len(),
            diff_sections[1].height,
        );
        let diff_para = Paragraph::new(render_diff_window(
            diff_lines,
            app.commit_diff_scroll,
            diff_sections[1].height,
        ))
        .block(
            Block::default()
                .title(format!("{diff_title} [{diff_meta}] "))
                .borders(Borders::ALL)
                .border_style(if app.commit_list_focus == CommitListFocus::Diff {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::DarkGray)
                }),
        );
        frame.render_widget(Clear, diff_sections[1]);
        frame.render_widget(diff_para, diff_sections[1]);
    }
}

pub fn render_normal(frame: &mut Frame, app: &mut App, area: Rect, squash_active: bool) {
    // Geometry comes from app::layout so mouse hit-testing always agrees.
    let chunks = crate::app::layout::main_chunks(area, app.splits.details_height);
    let (list_rect, diff_rect) =
        crate::app::layout::commit_list_regions_in(area, app.show_commit_diff, app.splits);

    if app.move_state.is_some() {
        render_mode_title(
            frame,
            chunks[0],
            "MOVE MODE  \u{2502}  j/k \u{2191}\u{2193}: move commit  \u{2502}  Enter: apply (rewrites history)  \u{2502}  Esc: cancel",
        );
    } else if squash_active {
        render_squash_title(frame, chunks[0]);
    } else {
        render_title(frame, chunks[0], app.commit_list_view, app.commit_sort);
    }

    render_commit_list_panel(frame, app, list_rect);

    let selected = app.selected_commit().cloned();
    let stats = selected
        .as_ref()
        .and_then(|c| app.stats_cache.get(&c.hash))
        .cloned();
    render_diff_panels(frame, app, diff_rect, selected.as_ref(), stats.as_ref());

    if app.working_tree_selected() {
        crate::ui::screens::details::render_working_tree_details(
            frame,
            &app.working_tree,
            app.commit_list_focus == crate::app::CommitListFocus::Details,
            chunks[2],
        );
        render_status(frame, app, chunks[3]);
        return;
    }
    let merged_via = app
        .selected_commit_index()
        .and_then(|idx| app.merge_point_of(idx))
        .map(|merge_idx| {
            let c = &app.commits[merge_idx];
            format!("{} {}", c.short_hash(), truncate(&c.message, 70))
        });
    let details_terms = app
        .selected_commit_index()
        .filter(|idx| app.search.matches.binary_search(idx).is_ok())
        .and(app.search.executed_terms.as_ref());
    render_details(
        frame,
        selected.as_ref(),
        stats.as_ref(),
        merged_via.as_deref(),
        details_terms,
        chunks[2],
    );
    render_status(frame, app, chunks[3]);
}

/// Info pane for the "Uncommitted changes" row. The file list starts at
/// line `INFO_HEADER_LINES`: the click-to-jump hit test counts on it.
fn working_tree_info_lines(
    working_tree: &crate::app::features::working_tree::WorkingTreeState,
    files: &[crate::app::CommitDiffFile],
) -> Vec<Line<'static>> {
    let label = |text: &'static str| Span::styled(text, Style::default().fg(Color::Yellow));
    let (staged, unstaged, untracked) = working_tree.counts();
    let additions: usize = files.iter().map(|f| f.additions).sum();
    let deletions: usize = files.iter().map(|f| f.deletions).sum();
    let mut lines = vec![
        Line::from(Span::styled(
            "Uncommitted changes (not a commit yet)",
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            label("Files: "),
            Span::raw(format!(
                "{staged} staged, {unstaged} unstaged, {untracked} untracked"
            )),
        ]),
        Line::from(vec![
            label("Changes: "),
            Span::styled(format!("+{additions}"), Style::default().fg(Color::Green)),
            Span::raw(" "),
            Span::styled(format!("-{deletions}"), Style::default().fg(Color::Red)),
        ]),
        Line::from(""),
        Line::from(label("Files (click to jump):")),
    ];
    debug_assert_eq!(
        lines.len(),
        crate::app::features::working_tree::INFO_HEADER_LINES
    );
    lines.extend(files.iter().map(|file| {
        Line::from(vec![
            Span::styled(
                format!("{} ", file.status),
                Style::default()
                    .fg(file_status_color(file.status))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("+{}", file.additions),
                Style::default().fg(Color::Green),
            ),
            Span::raw(" "),
            Span::styled(
                format!("-{}", file.deletions),
                Style::default().fg(Color::Red),
            ),
            Span::styled(
                format!("  {:<9} ", working_tree.section_of(file.diff_line)),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw(file.path.clone()),
        ])
    }));
    lines
}

fn render_commit_info_lines(
    commit: Option<&crate::git::CommitInfo>,
    stats: Option<&crate::git::CommitStats>,
    files: &[crate::app::CommitDiffFile],
    search_terms: Option<&crate::app::features::search::ExecutedTerms>,
) -> Vec<Line<'static>> {
    use crate::app::features::search::match_ranges;
    use crate::ui::helpers::highlight_spans;
    let Some(commit) = commit else {
        return vec![Line::from("No commit selected")];
    };
    // Search-hit marking, same convention as the Details pane.
    let hl = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let marked = |text: &str, terms: &[&str]| -> Vec<Span<'static>> {
        match search_terms {
            Some(_) => highlight_spans(text, Style::default(), hl, &match_ranges(text, terms)),
            None => vec![Span::raw(text.to_string())],
        }
    };
    let empty_terms = crate::app::features::search::ExecutedTerms::default();
    let terms = search_terms.unwrap_or(&empty_terms);
    let stats = stats.cloned().unwrap_or(crate::git::CommitStats {
        files_added: 0,
        files_modified: 0,
        files_deleted: 0,
        lines_inserted: 0,
        lines_deleted: 0,
    });
    let email = if commit.author_email.is_empty() {
        "loading..."
    } else {
        commit.author_email.as_str()
    };
    let full_message = commit.full_message.as_deref().unwrap_or(&commit.message);
    let message_lines: Vec<&str> = if full_message.is_empty() {
        vec!["(empty message)"]
    } else {
        full_message.lines().collect()
    };
    let file_count = stats.files_added + stats.files_modified + stats.files_deleted;
    let mut hash_line = vec![Span::styled("Hash: ", Style::default().fg(Color::Yellow))];
    hash_line.extend(marked(&commit.hash, &terms.for_hash()));
    let mut author_line = vec![Span::styled("Author: ", Style::default().fg(Color::Yellow))];
    author_line.extend(marked(
        &format!("{} <{email}>", commit.author_name),
        &terms.for_author(),
    ));
    let mut lines = vec![
        Line::from(hash_line),
        Line::from(author_line),
        {
            let date_text = commit.date_display_text().into_owned();
            let mut date_line = vec![Span::styled("Date: ", Style::default().fg(Color::Yellow))];
            match search_terms {
                Some(t) => date_line.extend(highlight_spans(
                    &date_text,
                    Style::default(),
                    hl,
                    &t.date_ranges(&date_text),
                )),
                None => date_line.push(Span::raw(date_text)),
            }
            Line::from(date_line)
        },
        Line::from(vec![
            Span::styled("Changes: ", Style::default().fg(Color::Yellow)),
            Span::styled(
                format!("+{}", stats.lines_inserted),
                Style::default().fg(Color::Green),
            ),
            Span::raw(" "),
            Span::styled(
                format!("-{}", stats.lines_deleted),
                Style::default().fg(Color::Red),
            ),
            Span::raw(format!(" ({file_count} files)")),
        ]),
        Line::from(vec![
            Span::styled("Files: ", Style::default().fg(Color::Yellow)),
            Span::styled(
                format!("+{}", stats.files_added),
                Style::default().fg(Color::Green),
            ),
            Span::raw(" "),
            Span::styled(
                format!("~{}", stats.files_modified),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw(" "),
            Span::styled(
                format!("-{}", stats.files_deleted),
                Style::default().fg(Color::Red),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled("Message:", Style::default().fg(Color::Yellow))),
    ];
    lines.extend(
        message_lines
            .into_iter()
            .map(|line| Line::from(marked(line, &terms.for_message()))),
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Files (click to jump):",
        Style::default().fg(Color::Yellow),
    )));
    lines.extend(files.iter().map(|file| {
        Line::from(
            vec![
                Span::styled(
                    format!("{} ", file.status),
                    Style::default()
                        .fg(file_status_color(file.status))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("+{}", file.additions),
                    Style::default().fg(Color::Green),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("-{}", file.deletions),
                    Style::default().fg(Color::Red),
                ),
                Span::raw("  "),
            ]
            .into_iter()
            .chain(marked(&file.path, &terms.for_file()))
            .collect::<Vec<_>>(),
        )
    }));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "--------------------------------",
        Style::default().fg(Color::DarkGray),
    )));
    lines
}

pub fn render_title(
    frame: &mut Frame,
    area: Rect,
    commit_list_view: CommitListView,
    commit_sort: git::CommitSort,
) {
    let title = Paragraph::new(
        format!(
            "ghh  \u{2502}  j/k \u{2191}\u{2193}: list or pane scroll  \u{2502}  Tab/Shift+Tab: panes  \u{2502}  v: toggle diff  \u{2502}  .: menu  \u{2502}  l: list view {}  \u{2502}  o: order {}  \u{2502}  e: edit  \u{2502}  /: search  \u{2502}  n/N: matches  \u{2502}  f: filter  \u{2502}  r: squash  \u{2502}  E: move  \u{2502}  s: staging  \u{2502}  S: stashes  \u{2502}  h/?: help  \u{2502}  q: quit",
            commit_list_view.label(),
            commit_sort.label()
        ),
    )
    .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
    .alignment(ratatui::layout::Alignment::Center);
    frame.render_widget(title, area);
}

pub fn render_squash_title(frame: &mut Frame, area: Rect) {
    render_mode_title(
        frame,
        area,
        "SQUASH MODE  \u{2502}  j/k \u{2191}\u{2193}: navigate  \u{2502}  Space: toggle [S]  \u{2502}  Enter: confirm  \u{2502}  Esc: cancel  \u{2502}  [BASE]=kept  [S]=squash into base",
    );
}

fn render_mode_title(frame: &mut Frame, area: Rect, text: &'static str) {
    let title = Paragraph::new(text)
        .style(
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(ratatui::layout::Alignment::Center);
    frame.render_widget(title, area);
}

/// gitk colors: green box = local branch (bold for HEAD), orange = remote, yellow = tag.
fn ref_label_span(label: &git::RefLabel) -> Span<'static> {
    // Bright/indexed backgrounds: ANSI Green/Yellow are dark in many default
    // themes and Terminal.app (pre-macOS 26) drops truecolor, so black text
    // on those backgrounds was barely readable.
    let style = match label.kind {
        git::RefKind::Branch => Style::default().bg(Color::LightGreen).fg(Color::Black),
        git::RefKind::RemoteBranch => Style::default().bg(Color::Indexed(172)).fg(Color::Black),
        git::RefKind::Tag => Style::default().bg(Color::LightYellow).fg(Color::Black),
    };
    let style = if label.is_head {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    };
    Span::styled(format!("[{}]", label.name), style)
}

/// A green, M yellow, D red, R magenta — same palette as the info pane.
fn file_status_color(status: char) -> Color {
    match status {
        'A' => Color::Green,
        'D' => Color::Red,
        'R' => Color::Magenta,
        _ => Color::Yellow,
    }
}

pub fn lane_color(lane: u8) -> Color {
    match lane % 8 {
        0 => Color::White,
        1 => Color::Cyan,
        2 => Color::LightGreen,
        3 => Color::LightYellow,
        4 => Color::LightMagenta,
        5 => Color::LightRed,
        6 => Color::LightBlue,
        _ => Color::LightCyan,
    }
}

fn commit_message_width(list_width: u16, row_count_width: usize, graph_width: usize) -> usize {
    let reserved = 2usize  // list highlight symbol
        + 2  // list borders
        + row_count_width
        + 1  // row index trailing space
        + graph_width
        + 1  // space after graph prefix
        + 8  // short hash + space
        + 17 // date + space
        + 17; // author column + space

    usize::from(list_width).saturating_sub(reserved).max(12)
}

/// Indent level of a row: `Commit` rows carry it, headers are the top level.
fn row_depth(row: &VisibleRow) -> usize {
    match row {
        VisibleRow::Commit { depth, .. } => *depth,
        VisibleRow::DrillHeader { .. }
        | VisibleRow::CollapsedBranch { .. }
        | VisibleRow::WorkingTree => 0,
    }
}

fn graph_render_width(app: &App, start: usize, end: usize) -> usize {
    if app.commit_list_view == CommitListView::Simple {
        return 0;
    }
    let row_prefix = |idx: &usize, prefix_override: &Option<(String, Vec<u8>)>| -> usize {
        match prefix_override {
            Some((p, _)) => p.chars().count(),
            None => app.commits[*idx].graph_prefix.chars().count(),
        }
    };
    app.visible[start.min(app.visible.len())..end.min(app.visible.len())]
        .iter()
        .filter_map(|row| match row {
            VisibleRow::Commit {
                idx,
                prefix_override,
                depth,
            } => Some(row_prefix(idx, prefix_override) + 2 * depth),
            VisibleRow::DrillHeader {
                idx,
                prefix_override,
            } => Some(row_prefix(idx, prefix_override)),
            VisibleRow::CollapsedBranch { .. } | VisibleRow::WorkingTree => None,
        })
        .max()
        .unwrap_or(0)
}

fn lane_count(prefix: &str) -> usize {
    prefix.chars().count().div_ceil(2)
}

/// Map a layout glyph to what gets drawn. gitk's node is a filled circle and
/// its merge joins are curves, so `*` becomes `●` and the box corners become
/// rounded ones. Merges keep `▼`/`►`: unlike gitk, ghh can collapse a merge's
/// branch, and that marker is the only affordance for it.
fn graph_glyph(ch: char, is_collapsed: bool, is_head: bool) -> char {
    match ch {
        'M' if is_collapsed => '\u{25BA}', // ►
        'M' => '\u{25BC}',                 // ▼
        // gitk draws the commit HEAD points at as a hollow circle.
        '*' if is_head => '\u{25CB}', // ○
        '*' => '\u{25CF}',            // ●
        '┌' => '\u{256D}',            // ╭
        '┐' => '\u{256E}',            // ╮
        other => other,
    }
}

fn build_graph_spans(
    prefix: &str,
    prefix_lanes: &[u8],
    is_collapsed: bool,
    is_head: bool,
) -> Vec<Span<'static>> {
    let chars: Vec<char> = prefix.chars().collect();
    let total_lanes = lane_count(prefix);
    let mut spans: Vec<Span<'static>> = Vec::new();

    for lane in 0..total_lanes {
        let char_idx = lane * 2;
        let ch = chars.get(char_idx).copied().unwrap_or(' ');
        let lane_idx = prefix_lanes.get(char_idx).copied().unwrap_or(255u8);
        let is_node = matches!(ch, '*' | 'M');
        let display =
            crate::ui::helpers::display_glyph(graph_glyph(ch, is_collapsed, is_head && is_node));
        let style = if lane_idx == 255u8 {
            Style::default()
        } else if is_head && is_node {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            let color = lane_color(lane_idx);
            if matches!(ch, '*' | 'M' | '┌' | '┐' | '─') {
                Style::default().fg(color).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(color)
            }
        };
        spans.push(Span::styled(display.to_string(), style));
        if lane + 1 < total_lanes {
            spans.push(Span::raw(" "));
        }
    }

    spans
}

fn commit_viewport(
    selected: Option<usize>,
    current_offset: usize,
    total_items: usize,
    viewport_height: usize,
) -> (usize, usize, Option<usize>) {
    if total_items == 0 {
        return (0, 0, None);
    }

    let height = viewport_height.max(1);
    let mut start = current_offset.min(total_items.saturating_sub(1));
    let selected = selected.unwrap_or(0).min(total_items.saturating_sub(1));

    if selected < start {
        start = selected;
    } else if selected >= start.saturating_add(height) {
        start = selected.saturating_add(1).saturating_sub(height);
    }

    let end = (start + height).min(total_items);
    (start, end, Some(selected.saturating_sub(start)))
}

#[cfg(test)]
mod tests {
    use super::graph_glyph;

    #[test]
    fn commit_nodes_use_gitk_circles() {
        assert_eq!(graph_glyph('*', false, false), '\u{25CF}'); // ●
        assert_eq!(graph_glyph('*', false, true), '\u{25CB}'); // ○ at HEAD
    }

    #[test]
    fn merges_keep_the_collapse_marker() {
        // gitk has no collapse; ghh does, and this marker is its only affordance.
        assert_eq!(graph_glyph('M', false, false), '\u{25BC}'); // ▼ expanded
        assert_eq!(graph_glyph('M', true, false), '\u{25BA}'); // ► collapsed
        assert_eq!(graph_glyph('M', true, true), '\u{25BA}'); // HEAD does not hide it
    }

    #[test]
    fn merge_joins_are_rounded() {
        assert_eq!(graph_glyph('┌', false, false), '╭');
        assert_eq!(graph_glyph('┐', false, false), '╮');
        for unchanged in ['│', '─', '↑', '↓', ' '] {
            assert_eq!(graph_glyph(unchanged, false, false), unchanged);
        }
    }
}
