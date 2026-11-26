use super::diff::{render_diff_window, scroll_label};
use crate::app::App;
use crate::app::features::staging::StagingFocus;
use crate::git::StageState;
use crate::ui::helpers::truncate;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

pub fn render_staging(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Geometry comes from app::layout so mouse hit-testing always agrees.
    let chunks = crate::app::layout::main_chunks(area, 1);
    let (files_rect, diff_rect, hint_rect) =
        crate::app::layout::staging_regions_in(area, app.splits);

    let title = Paragraph::new(
        "STAGING  │  Tab: file list / diff  │  j/k: list or diff scroll  │  Space: toggle  │  a: stage all  │  u: unstage all  │  Enter: commit  │  r: reload  │  Esc/Backspace: back",
    )
    .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
    .alignment(ratatui::layout::Alignment::Center);
    frame.render_widget(title, chunks[0]);

    render_files_panel(frame, app, files_rect);
    render_staging_diff_panel(frame, app, diff_rect);
    render_commit_hint_row(frame, app, hint_rect);
    render_staging_status_row(frame, app, chunks[3]);
}

/// File list with stage-state markers and focus border.
fn render_files_panel(frame: &mut Frame, app: &mut App, rect: Rect) {
    let (items, file_count) = {
        let staging = match app.staging.as_ref() {
            Some(s) => s,
            None => return,
        };
        let count = staging.files.len();
        let items: Vec<ListItem<'static>> = staging
            .files
            .iter()
            .map(|f| {
                let (ch_color, label) = match f.stage_state {
                    StageState::Staged => (Color::Green, " staged"),
                    StageState::Unstaged => (Color::Red, " unstaged"),
                    StageState::Untracked => (Color::DarkGray, " untracked"),
                };
                let line = Line::from(vec![
                    Span::styled(
                        format!("[{}] ", f.status_char),
                        Style::default().fg(ch_color).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(truncate(&f.path, 28)),
                    Span::styled(label, Style::default().fg(ch_color)),
                ]);
                ListItem::new(line)
            })
            .collect();
        (items, count)
    };

    let file_block = Block::default()
        .title(format!(" Files ({file_count}) "))
        .borders(Borders::ALL)
        .border_style(
            if app
                .staging
                .as_ref()
                .map(|s| s.focus == StagingFocus::FileList)
                .unwrap_or(false)
            {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        );
    let file_list = List::new(items)
        .block(file_block)
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::REVERSED)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    frame.render_stateful_widget(
        file_list,
        rect,
        &mut app.staging.as_mut().unwrap().list_state,
    );
}

/// Diff pane for the selected file, scroll-labelled.
fn render_staging_diff_panel(frame: &mut Frame, app: &mut App, rect: Rect) {
    let staging = match app.staging.as_mut() {
        Some(s) => s,
        None => return,
    };
    // Keys and wheel only add to the scroll; it stops here, with the last
    // line on the last row.
    let rows = usize::from(rect.height.saturating_sub(2)).max(1);
    staging.diff_scroll = staging
        .diff_scroll
        .min(staging.diff_lines.len().saturating_sub(rows));

    let selected_path = staging
        .selected_file()
        .map(|f| f.path.as_str())
        .unwrap_or("");
    let diff_meta = scroll_label(staging.diff_scroll, staging.diff_lines.len(), rect.height);
    let diff_block = Block::default()
        .title(format!(" Diff: {selected_path} [{diff_meta}] "))
        .borders(Borders::ALL)
        .border_style(if staging.focus == StagingFocus::Diff {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        });

    let diff_para = Paragraph::new(render_diff_window(
        &staging.diff_lines,
        staging.diff_scroll,
        rect.height,
    ))
    .block(diff_block);
    frame.render_widget(diff_para, rect);
}

/// What Enter does, shown only while there is something to commit.
fn render_commit_hint_row(frame: &mut Frame, app: &App, rect: Rect) {
    let staged = app.staging.as_ref().map_or(0, |staging| {
        staging
            .files
            .iter()
            .filter(|file| file.stage_state == StageState::Staged)
            .count()
    });
    if staged == 0 {
        return;
    }
    let files = if staged == 1 { "file" } else { "files" };
    frame.render_widget(
        Paragraph::new(format!(
            " Enter: write the message and commit ({staged} staged {files})"
        ))
        .style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        rect,
    );
}

/// Status line: staging message first, app message as fallback.
fn render_staging_status_row(frame: &mut Frame, app: &App, rect: Rect) {
    let staging = match app.staging.as_ref() {
        Some(s) => s,
        None => return,
    };
    let status_text = staging
        .status_msg
        .as_deref()
        .or(app.status_message.as_deref())
        .unwrap_or("");
    let status_style =
        if status_text.starts_with("Error") || status_text.starts_with("Commit error") {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Green)
        };
    frame.render_widget(
        Paragraph::new(status_text.to_string()).style(status_style),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    /// Staging screen smoke + landmarks: title, file with stage marker, diff
    /// pane, commit hint, status line (guards the sectioning refactor).
    #[test]
    fn staging_screen_draws_all_blocks() {
        let dir = std::env::temp_dir().join(format!("ghh-staging-render-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = crate::app::App::new(dir, true);
        app.nav.push(crate::app::models::Mode::Staging);
        let mut list_state = ratatui::widgets::ListState::default();
        list_state.select(Some(0));
        app.staging = Some(crate::app::features::staging::StagingState {
            files: vec![
                crate::git::FileStatus {
                    path: "src/lib.rs".into(),
                    stage_state: crate::git::StageState::Staged,
                    status_char: 'M',
                },
                crate::git::FileStatus {
                    path: "notes.txt".into(),
                    stage_state: crate::git::StageState::Untracked,
                    status_char: '?',
                },
            ],
            list_state,
            diff_lines: vec!["+added line".into(), "-removed line".into()],
            diff_scroll: 0,
            commit_msg: String::new(),
            focus: crate::app::features::staging::StagingFocus::FileList,
            status_msg: Some("2 files".into()),
        });
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::render_staging(frame, &mut app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let all: String = (0..30)
            .map(|y| {
                (0..120)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        for needle in [
            "STAGING",
            "src/lib.rs",
            "notes.txt",
            "+added line",
            "Enter: write the message and commit (1 staged file)",
            "2 files",
        ] {
            assert!(all.contains(needle), "{needle:?} missing:\n{all}");
        }

        // Nothing staged: no commit hint.
        app.staging.as_mut().unwrap().files.remove(0);
        terminal
            .draw(|frame| super::render_staging(frame, &mut app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let all: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(all.contains("notes.txt") && !all.contains("Enter: write the message"));
    }
}
