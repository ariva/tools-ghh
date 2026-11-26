//! `ghh faq`: the questions of `docs/FAQ.md` at the command line, with
//! filters. The file is compiled in and parsed here, so the document is the
//! only copy of the list. No repository, no terminal setup.

use serde_json::json;
use std::io::IsTerminal;

const SOURCE: &str = include_str!("../../../docs/FAQ.md");

/// One numbered section of the FAQ (`## 3. Changing history`).
#[derive(Debug, PartialEq)]
pub struct Section {
    pub number: u32,
    pub title: String,
    /// Paragraph between the heading and the table, if any.
    pub intro: String,
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// `3.7`
    pub number: String,
    pub question: String,
    /// As written, backticks around keys kept: the key filter reads them.
    pub answer: String,
}

/// Sections and rows of a FAQ document. Anything that is not a numbered
/// `## N. Title` heading, its intro paragraph or a `| N.M | … |` table row
/// is skipped (page intro, table header, the closing notes).
pub fn parse(source: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    let mut in_numbered = false;
    for line in source.lines() {
        let line = line.trim();
        if let Some(heading) = line.strip_prefix("## ") {
            let numbered = heading
                .split_once(". ")
                .and_then(|(number, title)| Some((number.parse().ok()?, title)));
            in_numbered = numbered.is_some();
            if let Some((number, title)) = numbered {
                sections.push(Section {
                    number,
                    title: title.to_string(),
                    intro: String::new(),
                    rows: Vec::new(),
                });
            }
            continue;
        }
        let Some(section) = sections.last_mut().filter(|_| in_numbered) else {
            continue;
        };
        if let Some(cells) = line.strip_prefix('|') {
            let cells: Vec<&str> = cells.split('|').map(str::trim).collect();
            let is_row = cells.len() >= 3
                && cells[0]
                    .split_once('.')
                    .is_some_and(|(a, b)| a.parse::<u32>().is_ok() && b.parse::<u32>().is_ok());
            if is_row {
                section.rows.push(Row {
                    number: cells[0].to_string(),
                    question: cells[1].to_string(),
                    answer: cells[2].to_string(),
                });
            }
        } else if !line.is_empty() && section.rows.is_empty() {
            if !section.intro.is_empty() {
                section.intro.push(' ');
            }
            section.intro.push_str(line);
        }
    }
    sections
}

/// The keys an answer names: every `` `…` `` span, as written.
fn named_keys(answer: &str) -> impl Iterator<Item = &str> {
    answer.split('`').skip(1).step_by(2)
}

/// Does `row` of `section` pass one filter word?
///
/// | Word | Passes when |
/// |---|---|
/// | `3` | the row is in section 3 |
/// | `3.7` | the row is 3.7 (`1.1` is not 1.10) |
/// | `` `E` `` | the answer names the key `E`, case kept |
/// | other | it is in the section title, question or answer, case ignored |
fn word_matches(section: &Section, row: &Row, word: &str) -> bool {
    if let Some(key) = word
        .strip_prefix('`')
        .and_then(|rest| rest.strip_suffix('`'))
    {
        return named_keys(&row.answer).any(|named| named == key);
    }
    if let Ok(number) = word.parse::<u32>() {
        return section.number == number;
    }
    if word
        .split_once('.')
        .is_some_and(|(a, b)| a.parse::<u32>().is_ok() && b.parse::<u32>().is_ok())
    {
        return row.number == word;
    }
    let word = word.to_lowercase();
    [&section.title, &row.question, &row.answer]
        .iter()
        .any(|text| text.to_lowercase().contains(&word))
}

/// The sections with only the rows every word lets through; sections left
/// without rows are dropped.
pub fn matching(sections: Vec<Section>, words: &[String]) -> Vec<Section> {
    sections
        .into_iter()
        .filter_map(|mut section| {
            let rows = std::mem::take(&mut section.rows);
            section.rows = rows
                .into_iter()
                .filter(|row| words.iter().all(|word| word_matches(&section, row, word)))
                .collect();
            (!section.rows.is_empty()).then_some(section)
        })
        .collect()
}

/// Greedy word wrap; a word longer than `width` gets a line of its own.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

/// Plain text: a heading per section, its intro, then per row the number
/// and question, with the answer indented under it. Backticks dropped.
pub fn render_text(sections: &[Section], width: usize) -> String {
    const INDENT: usize = 6;
    let mut out = String::new();
    for section in sections {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("{}. {}\n", section.number, section.title));
        for line in wrap(&section.intro, width.saturating_sub(2).max(20)) {
            out.push_str(&format!("  {line}\n"));
        }
        out.push('\n');
        let text_width = width.saturating_sub(INDENT).max(20);
        for row in &section.rows {
            let mut number = Some(row.number.as_str());
            for line in wrap(&row.question, text_width) {
                out.push_str(&format!("{:<INDENT$}{line}\n", number.take().unwrap_or("")));
            }
            for line in wrap(&row.answer.replace('`', ""), text_width) {
                out.push_str(&format!("{:<INDENT$}{line}\n", ""));
            }
        }
    }
    out
}

pub fn render_json(sections: &[Section]) -> String {
    let rows: Vec<_> = sections
        .iter()
        .flat_map(|section| {
            section.rows.iter().map(|row| {
                json!({
                    "number": row.number,
                    "section": section.title,
                    "question": row.question,
                    "answer": row.answer.replace('`', ""),
                })
            })
        })
        .collect();
    serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".to_string())
}

/// Print the FAQ rows that pass `words` and `keys` (each key is one more
/// filter: the answer must name it).
pub fn run(words: &[String], keys: &[String], as_json: bool) {
    let mut filters = words.to_vec();
    filters.extend(keys.iter().map(|key| format!("`{key}`")));
    let found = matching(parse(SOURCE), &filters);
    if as_json {
        println!("{}", render_json(&found));
    } else if found.is_empty() {
        // Not an error: the question is simply not in the list.
        let asked = filters.join(" ");
        println!("No question matches \"{asked}\". `ghh faq` lists them all.");
    } else {
        let width = if std::io::stdout().is_terminal() {
            crossterm::terminal::size().map_or(80, |(columns, _)| usize::from(columns))
        } else {
            80
        };
        print!("{}", render_text(&found, width.min(100)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(words: &[&str]) -> Vec<String> {
        let words: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        matching(parse(SOURCE), &words)
            .iter()
            .flat_map(|section| section.rows.iter().map(|row| row.number.clone()))
            .collect()
    }

    /// The document itself: every numbered section has rows, and row
    /// numbers are its own section's, unique and ascending. A row added in
    /// the wrong place fails here.
    #[test]
    fn faq_document_is_numbered_in_order() {
        let sections = parse(SOURCE);
        assert!(sections.len() >= 6, "{}", sections.len());
        for (index, section) in sections.iter().enumerate() {
            assert_eq!(section.number as usize, index + 1);
            assert!(!section.rows.is_empty(), "section {}", section.number);
            for (at, row) in section.rows.iter().enumerate() {
                assert_eq!(
                    row.number,
                    format!("{}.{}", section.number, at + 1),
                    "row out of order in section {}",
                    section.number
                );
                assert!(!row.question.is_empty() && !row.answer.is_empty());
            }
        }
        // The intro of the one section that has one is kept, on one line.
        assert!(
            sections[2]
                .intro
                .starts_with("Every row here rewrites commits")
        );
        assert!(sections[0].intro.is_empty());
    }

    #[test]
    fn filters_by_section_row_word_and_key() {
        assert!(found(&[]).len() > 40, "no words: everything");
        // A section number takes the whole section, nothing else.
        let three = found(&["3"]);
        assert!(three.len() >= 9 && three.iter().all(|n| n.starts_with("3.")));
        // An exact row: 1.1 is not 1.10, 1.11…
        assert_eq!(found(&["1.1"]), ["1.1"]);
        // Words: case ignored, all must match; the section title counts.
        assert_eq!(found(&["MOVE", "conflict"]), ["3.7"]);
        assert!(found(&["stashes"]).contains(&"4.1".to_string()), "title");
        assert!(found(&["nosuchword"]).is_empty());
        // A key: named in the answer, case kept. `E` moves, `e` edits.
        let upper = found(&["`E`"]);
        let lower = found(&["`e`"]);
        assert!(upper.contains(&"3.6".to_string()) && !upper.contains(&"3.1".to_string()));
        assert!(lower.contains(&"3.1".to_string()) && !lower.contains(&"3.6".to_string()));
    }

    #[test]
    fn text_output_wraps_and_drops_backticks() {
        let sections = matching(parse(SOURCE), &["3.7".to_string()]);
        let text = render_text(&sections, 60);
        assert!(text.starts_with("3. Changing history\n  Every row here"));
        assert!(text.contains("\n3.7   A move stopped on a conflict. Now what?\n"));
        assert!(text.contains("press E:") && !text.contains('`'));
        assert!(
            text.lines().all(|line| line.chars().count() <= 60),
            "{text}"
        );
    }

    #[test]
    fn json_output_is_an_array_of_rows() {
        let sections = matching(parse(SOURCE), &["3.7".to_string()]);
        let rows: serde_json::Value = serde_json::from_str(&render_json(&sections)).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["number"], "3.7");
        assert_eq!(rows[0]["section"], "Changing history");
        assert_eq!(render_json(&[]), "[]");
    }
}
