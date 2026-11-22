//! Search over the loaded commit list (`Mode::Search`): field state,
//! execution and autocomplete.

use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchField {
    Quick,
    Hash,
    Message,
    Author,
    DateFrom,
    DateTo,
    File,
    Branch,
    Tag,
}

impl SearchField {
    pub const ALL: [Self; 9] = [
        Self::Quick,
        Self::Hash,
        Self::Message,
        Self::Author,
        Self::DateFrom,
        Self::DateTo,
        Self::File,
        Self::Branch,
        Self::Tag,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Quick => "Quick search",
            Self::Hash => "Hash",
            Self::Message => "Message",
            Self::Author => "Author",
            Self::DateFrom => "Date from",
            Self::DateTo => "Date to",
            Self::File => "Modified file",
            Self::Branch => "Branch",
            Self::Tag => "Tag",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Quick => 0,
            Self::Hash => 1,
            Self::Message => 2,
            Self::Author => 3,
            Self::DateFrom => 4,
            Self::DateTo => 5,
            Self::File => 6,
            Self::Branch => 7,
            Self::Tag => 8,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchState {
    pub values: [String; 9],
    pub field: SearchField,
    pub cursor: usize,
    pub matches: Vec<usize>,
    pub match_pos: usize,
    /// When set, the commit list shows only `matches` (f toggles).
    pub filter_active: bool,
    /// Focused bottom button (0 Search, 1 Cancel, 2 Reset/Undo); None = fields.
    pub button_focus: Option<usize>,
    /// Inputs saved by Reset; present = "Undo Reset" shown. Any typing drops it.
    pub reset_backup: Option<[String; 9]>,
    /// Message shown inside the popup (no matches / search errors). The
    /// status line is hidden behind the popup, so errors must live here.
    pub notice: Option<String>,
    /// Inputs as they were when the popup opened; Cancel/Esc restores them,
    /// a successful Search keeps the edits.
    pub open_backup: Option<[String; 9]>,
    /// File patterns of the EXECUTED search (empty = no file pattern):
    /// drives the A/M/D/R letters on matched rows.
    pub active_file_patterns: Vec<String>,
    /// (field, query, commit count) → ranked suggestions. Rendering runs per
    /// frame; the commit-derived fields scan the whole list, so results are
    /// cached until the query (or the list) changes.
    pub suggestion_cache: Option<SuggestionCache>,
    /// Whole active-field value selected (double-click): Backspace/Delete
    /// clears it, typing replaces it, anything else deselects.
    pub select_all: bool,
    /// First candidate shown on the suggestions page ("..." advances it;
    /// reset whenever suggestions recompute).
    pub suggestion_page_start: usize,
    /// Non-wildcard OR parts of the EXECUTED search, per category — drives
    /// match highlighting in the list and details (derived display state).
    pub executed_terms: Option<ExecutedTerms>,
    /// File search runs git log --full-history (finds files changed in
    /// merges, slower). Checkbox in the popup, on by default.
    pub file_full_history: bool,
    /// File search filters on this change action (All = any).
    pub file_action: FileAction,
}

/// Which change action file search filters on (git --diff-filter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileAction {
    #[default]
    All,
    Added,
    Modified,
    Deleted,
    Renamed,
}

impl FileAction {
    pub const ALL: [FileAction; 5] = [
        FileAction::All,
        FileAction::Added,
        FileAction::Modified,
        FileAction::Deleted,
        FileAction::Renamed,
    ];
    pub fn diff_filter(self) -> Option<char> {
        match self {
            FileAction::All => None,
            FileAction::Added => Some('A'),
            FileAction::Modified => Some('M'),
            FileAction::Deleted => Some('D'),
            FileAction::Renamed => Some('R'),
        }
    }
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|a| *a == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// Cached fuzzy suggestions: (field, query, commits len, ranked candidates
/// with matched char positions).
pub type SuggestionCache = (SearchField, String, usize, Vec<(String, Vec<usize>)>);

/// Pure cursor+string edits for the popup inputs — no App, direct tests.
/// Each op returns the new (value, cursor).
pub mod input_edit {
    use super::{date_digits_plausible, format_date_digits, or_separator_flags};

    fn inside_joiner(chars: &[char], flags: &[bool], p: usize) -> bool {
        p > 0
            && p < chars.len()
            && flags.get(p).copied().unwrap_or(false)
            && flags.get(p - 1).copied().unwrap_or(false)
    }

    /// One cursor step that hops a whole " OR " joiner as a unit.
    pub fn step_cursor(value: &str, cursor: usize, forward: bool) -> usize {
        let chars: Vec<char> = value.chars().collect();
        let flags = or_separator_flags(&chars);
        if forward {
            let mut p = (cursor + 1).min(chars.len());
            while inside_joiner(&chars, &flags, p) {
                p += 1;
            }
            p.min(chars.len())
        } else {
            let mut p = cursor.saturating_sub(1);
            while inside_joiner(&chars, &flags, p) {
                p -= 1;
            }
            p
        }
    }

    /// Backspace; with `atomic_or`, a cursor right after " OR " removes the
    /// whole separator (joining the neighbouring texts).
    pub fn backspace(value: &str, cursor: usize, atomic_or: bool) -> (String, usize) {
        if cursor == 0 {
            return (value.to_string(), 0);
        }
        let mut chars: Vec<char> = value.chars().collect();
        let c = cursor.min(chars.len());
        if atomic_or && c >= 4 && chars[c - 4..c] == [' ', 'O', 'R', ' '] {
            chars.drain(c - 4..c);
            (chars.into_iter().collect(), c - 4)
        } else {
            chars.remove(c - 1);
            (chars.into_iter().collect(), c - 1)
        }
    }

    pub fn delete(value: &str, cursor: usize) -> (String, usize) {
        let mut chars: Vec<char> = value.chars().collect();
        if cursor < chars.len() {
            chars.remove(cursor);
        }
        (chars.into_iter().collect(), cursor)
    }

    pub fn insert(value: &str, cursor: usize, ch: char) -> (String, usize) {
        let mut chars: Vec<char> = value.chars().collect();
        let at = cursor.min(chars.len());
        chars.insert(at, ch);
        (chars.into_iter().collect(), at + 1)
    }

    /// Masked date input: a digit extends the date if it can still lead to a
    /// real date (dashes appear on their own); anything else is swallowed
    /// (None). Cursor is pinned at the end by the caller.
    pub fn date_input(value: &str, ch: char) -> Option<String> {
        if !ch.is_ascii_digit() {
            return None;
        }
        let extended: String = format!("{value}{ch}")
            .chars()
            .filter(char::is_ascii_digit)
            .collect();
        date_digits_plausible(&extended).then(|| format_date_digits(&extended))
    }

    /// Backspace/Delete on a date drop the LAST digit (masked input).
    pub fn date_backspace(value: &str) -> String {
        let digits: String = value.chars().filter(char::is_ascii_digit).collect();
        let trimmed: String = digits
            .chars()
            .take(digits.chars().count().saturating_sub(1))
            .collect();
        format_date_digits(&trimmed)
    }
}

/// Parsed, executable form of the popup inputs — pure data, no App/git.
/// `parse` validates; `matches` is the per-commit predicate (ref/file hash
/// sets are resolved by the caller, which needs the repo).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchQuery {
    pub quick: Vec<String>,
    pub hash: Vec<String>,
    pub message: Vec<String>,
    pub author: Vec<String>,
    pub file_patterns: Vec<String>,
    pub date_start: Option<String>,
    pub date_end: Option<String>,
}

fn split_or(raw: &str) -> Vec<String> {
    raw.split(" OR ")
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

impl SearchQuery {
    pub fn parse(values: &[String; 9]) -> Result<SearchQuery, String> {
        // From empty + To empty = no bound; To empty = from..now; From empty
        // = ..to. Both set = inclusive range.
        let (date_start, date_end) = match (
            validate_date(&values[SearchField::DateFrom.index()]),
            validate_date(&values[SearchField::DateTo.index()]),
        ) {
            (Ok(start), Ok(end)) => (start, end),
            (Err(err), _) | (_, Err(err)) => return Err(err),
        };
        if values.iter().all(|value| value.trim().is_empty()) {
            return Err("Enter search text.".into());
        }
        Ok(SearchQuery {
            quick: split_or(values[SearchField::Quick.index()].trim()),
            hash: split_or(values[SearchField::Hash.index()].trim()),
            message: split_or(values[SearchField::Message.index()].trim()),
            author: split_or(values[SearchField::Author.index()].trim()),
            // Comma-separated patterns are OR'd (git treats multiple
            // pathspecs as union).
            file_patterns: values[SearchField::File.index()]
                .split(" OR ")
                .flat_map(|part| part.split(','))
                .map(str::trim)
                .filter(|pattern| !pattern.is_empty())
                .map(str::to_string)
                .collect(),
            date_start,
            date_end,
        })
    }

    /// Highlight terms for the executed query (derived, not stored twice).
    pub fn executed_terms(&self) -> ExecutedTerms {
        ExecutedTerms {
            quick: self.quick.clone(),
            hash: self.hash.clone(),
            message: self.message.clone(),
            author: self.author.clone(),
            file: self.file_patterns.clone(),
            date_bounded: self.date_start.is_some() || self.date_end.is_some(),
        }
    }

    /// Per-commit predicate: every populated dimension must hold (AND);
    /// OR parts within a dimension match any.
    pub fn matches(
        &self,
        commit: &git::CommitInfo,
        file_hashes: Option<&std::collections::HashSet<String>>,
        branch_hashes: Option<&std::collections::HashSet<String>>,
        tag_hashes: Option<&std::collections::HashSet<String>>,
    ) -> bool {
        let date = commit.date_display_text();
        let date_key = date.get(..10).unwrap_or(date.as_ref());
        let quick_match = self.quick.is_empty()
            || self.quick.iter().any(|part| {
                wildcard_or_contains(&commit.hash, part)
                    || wildcard_or_contains(commit.short_hash(), part)
                    || wildcard_or_contains(&commit.message, part)
                    || wildcard_or_contains(&commit.author_name, part)
                    || wildcard_or_contains(date.as_ref(), part)
            });
        quick_match
            && (self.hash.is_empty()
                || self.hash.iter().any(|part| {
                    wildcard_or_contains(&commit.hash, part)
                        || wildcard_or_contains(commit.short_hash(), part)
                }))
            && (self.message.is_empty()
                || self
                    .message
                    .iter()
                    .any(|part| wildcard_or_contains(&commit.message, part)))
            && (self.author.is_empty()
                || self
                    .author
                    .iter()
                    .any(|part| wildcard_or_contains(&commit.author_name, part)))
            && self
                .date_start
                .as_ref()
                .is_none_or(|start| date_key >= start.as_str())
            && self
                .date_end
                .as_ref()
                .is_none_or(|end| date_key <= end.as_str())
            && file_hashes.is_none_or(|hashes| hashes.contains(&commit.hash))
            && branch_hashes.is_none_or(|hashes| hashes.contains(&commit.hash))
            && tag_hashes.is_none_or(|hashes| hashes.contains(&commit.hash))
    }
}

/// Indices of items satisfying `pred`, scanned in parallel chunks that keep
/// index order (568k commits: ~360 ms serial -> well under 100 ms).
pub fn parallel_filter_indices<T: Sync>(
    items: &[T],
    pred: impl Fn(&T) -> bool + Sync,
) -> Vec<usize> {
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    let chunk_size = items.len().div_ceil(workers).max(1);
    let per_chunk: Vec<Vec<usize>> = std::thread::scope(|s| {
        let handles: Vec<_> = items
            .chunks(chunk_size)
            .enumerate()
            .map(|(chunk_no, chunk)| {
                let pred = &pred;
                s.spawn(move || {
                    chunk
                        .iter()
                        .enumerate()
                        .filter(|(_, item)| pred(item))
                        .map(|(i, _)| chunk_no * chunk_size + i)
                        .collect()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("search worker panicked"))
            .collect()
    });
    per_chunk.concat()
}

/// What the last executed search looked for, split for highlighting.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutedTerms {
    pub quick: Vec<String>,
    pub hash: Vec<String>,
    pub message: Vec<String>,
    pub author: Vec<String>,
    pub file: Vec<String>,
    /// A From/To date bound was active — mark the day part of date lines.
    pub date_bounded: bool,
}

impl ExecutedTerms {
    pub fn for_message(&self) -> Vec<&str> {
        self.message
            .iter()
            .chain(&self.quick)
            .map(String::as_str)
            .collect()
    }
    pub fn for_hash(&self) -> Vec<&str> {
        self.hash
            .iter()
            .chain(&self.quick)
            .map(String::as_str)
            .collect()
    }
    pub fn for_author(&self) -> Vec<&str> {
        self.author
            .iter()
            .chain(&self.quick)
            .map(String::as_str)
            .collect()
    }
    pub fn for_file(&self) -> Vec<&str> {
        self.file.iter().map(String::as_str).collect()
    }
    /// Ranges to mark in a "YYYY-MM-DD HH:MM" date text: quick-term hits,
    /// plus the day part when a date bound filtered this search.
    pub fn date_ranges(&self, date_text: &str) -> Vec<(usize, usize)> {
        let mut ranges = match_ranges(
            date_text,
            &self.quick.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        if self.date_bounded && date_text.len() >= 10 {
            ranges.push((0, 10));
            ranges.sort_unstable();
            let mut merged: Vec<(usize, usize)> = Vec::new();
            for (start, end) in ranges {
                match merged.last_mut() {
                    Some(last) if start <= last.1 => last.1 = last.1.max(end),
                    _ => merged.push((start, end)),
                }
            }
            return merged;
        }
        ranges
    }
}

/// Char ranges of `text` matching any term (case-insensitive substring;
/// wildcard terms are skipped). Overlaps merged, sorted.
pub fn match_ranges(text: &str, terms: &[&str]) -> Vec<(usize, usize)> {
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for term in terms {
        if term.is_empty() || term.contains(['*', '?']) {
            continue;
        }
        let needle: Vec<char> = term.to_lowercase().chars().collect();
        if needle.len() > lower.len() {
            continue;
        }
        for start in 0..=lower.len() - needle.len() {
            if lower[start..start + needle.len()] == needle[..] {
                ranges.push((start, start + needle.len()));
            }
        }
    }
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            values: Default::default(),
            field: SearchField::Quick,
            cursor: 0,
            matches: Vec::new(),
            match_pos: 0,
            filter_active: false,
            button_focus: None,
            reset_backup: None,
            notice: None,
            open_backup: None,
            active_file_patterns: Vec::new(),
            suggestion_cache: None,
            select_all: false,
            suggestion_page_start: 0,
            executed_terms: None,
            file_full_history: true,
            file_action: FileAction::default(),
        }
    }
}

impl SearchState {
    pub fn value(&self, field: SearchField) -> &str {
        &self.values[field.index()]
    }

    pub fn value_mut(&mut self) -> &mut String {
        &mut self.values[self.field.index()]
    }

    pub fn set_field(&mut self, field: SearchField) {
        self.field = field;
        self.select_all = false;
        // Focusing an empty "Date to" pre-fills it with "Date from" (edit a
        // copied date instead of typing one). When From is empty too, the
        // END token shows instead.
        if field == SearchField::DateTo
            && self.value(SearchField::DateTo).is_empty()
            && !self.value(SearchField::DateFrom).trim().is_empty()
        {
            self.values[SearchField::DateTo.index()] =
                self.value(SearchField::DateFrom).trim().to_string();
            // Pre-filled value arrives SELECTED: typing replaces it wholesale
            // (no "2024-01-152..." appends), Backspace clears it.
            self.select_all = true;
        }
        self.cursor = self.value(field).chars().count();
    }

    pub fn clear_matches(&mut self) {
        self.matches.clear();
        self.match_pos = 0;
        self.active_file_patterns.clear();
        self.executed_terms = None;
    }

    pub fn next_match(&mut self, forward: bool) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        if forward {
            self.match_pos = (self.match_pos + 1) % self.matches.len();
        } else if self.match_pos == 0 {
            self.match_pos = self.matches.len() - 1;
        } else {
            self.match_pos -= 1;
        }
        self.matches.get(self.match_pos).copied()
    }

    /// Fields that take multiple `` OR ``-joined values (the [OR] button
    /// appends the separator). File additionally still accepts commas.
    pub fn multi_value_separator(field: SearchField) -> Option<&'static str> {
        match field {
            SearchField::DateFrom | SearchField::DateTo => None,
            _ => Some(" OR "),
        }
    }

    /// The text autocomplete matches against: the whole value, except for
    /// multi-value fields where it is the segment after the last separator
    /// (File: comma counts as a separator too).
    pub fn active_prefix(&self) -> &str {
        let value = self.value(self.field);
        match Self::multi_value_separator(self.field) {
            Some(sep) => {
                let tail = value.rsplit(sep).next().unwrap_or(value);
                let tail = if self.field == SearchField::File {
                    tail.rsplit(',').next().unwrap_or(tail)
                } else {
                    tail
                };
                tail.trim_start()
            }
            None => value,
        }
    }
}

/// fzf-style multi-term fuzzy ranking for path candidates. Every whitespace-
/// separated query term must match (any order); contiguous substring matches
/// rank far above scattered subsequences, earlier and shorter matches rank
/// higher. Returns up to `limit` candidates, best first, with the matched
/// char positions for highlighting.
pub fn fuzzy_rank<'a>(
    candidates: impl Iterator<Item = &'a str>,
    query: &str,
    limit: usize,
) -> Vec<(String, Vec<usize>)> {
    fuzzy_rank_scored(candidates, query, limit, false)
        .into_iter()
        .map(|(_, candidate, positions)| (candidate, positions))
        .collect()
}

/// Like [`fuzzy_rank`] but keeps the scores, so per-chunk results from a
/// parallel scan can be merged and re-ranked.
/// `substring_only` disables the scattered-subsequence fallback — right for
/// hex hashes, where "abc" would otherwise match almost everything.
pub fn fuzzy_rank_scored<'a>(
    candidates: impl Iterator<Item = &'a str>,
    query: &str,
    limit: usize,
    substring_only: bool,
) -> Vec<(i64, String, Vec<usize>)> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut seen: HashSet<String> = HashSet::new();
    if terms.is_empty() {
        // First `limit` UNIQUE candidates: authors repeat per commit — taking
        // first-8-then-dedup collapsed to a single suggestion.
        return candidates
            .filter(|c| seen.insert(c.to_string()))
            .take(limit)
            .map(|c| (0, c.to_string(), Vec::new()))
            .collect();
    }
    let mut ranked: Vec<(i64, String, Vec<usize>)> = Vec::new();
    for candidate in candidates {
        if seen.contains(candidate) {
            continue;
        }
        let lower: Vec<char> = candidate.to_lowercase().chars().collect();
        let mut score = 0i64;
        let mut positions: Vec<usize> = Vec::new();
        let mut matched_all = true;
        for term in &terms {
            let term_chars: Vec<char> = term.chars().collect();
            // Contiguous match first (strongly preferred).
            if let Some(start) = lower
                .windows(term_chars.len().max(1))
                .position(|window| window == term_chars.as_slice())
            {
                score += 100 - (start as i64).min(50);
                positions.extend(start..start + term_chars.len());
                continue;
            }
            if substring_only {
                matched_all = false;
                break;
            }
            // Greedy in-order subsequence fallback.
            let mut found = Vec::with_capacity(term_chars.len());
            let mut cursor = 0usize;
            for &ch in &term_chars {
                match lower[cursor..].iter().position(|&c| c == ch) {
                    Some(offset) => {
                        cursor += offset;
                        found.push(cursor);
                        cursor += 1;
                    }
                    None => break,
                }
            }
            if found.len() == term_chars.len() {
                let spread = found.last().copied().unwrap_or(0) as i64
                    - found.first().copied().unwrap_or(0) as i64;
                score += 40 - spread.min(30);
                positions.extend(found);
            } else {
                matched_all = false;
                break;
            }
        }
        if !matched_all {
            continue;
        }
        score -= (lower.len() as i64) / 8; // shorter paths edge out longer ones
        positions.sort_unstable();
        positions.dedup();
        seen.insert(candidate.to_string());
        ranked.push((score, candidate.to_string(), positions));
    }
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    ranked.truncate(limit);
    ranked
}

/// A multi-value input parsed into structure: text segments joined by
/// `` OR `` tokens. The string stays the storage (editing machinery is
/// untouched); this is the DERIVED lens every consumer goes through — the
/// renderer (coloring), the mouse (cursor snapping) and, later, the
/// remove-this-OR popup via [`OrSegments::remove_segment`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrSegment {
    /// Char range of the `` OR `` joiner BEFORE this segment (None = first).
    pub joiner: Option<std::ops::Range<usize>>,
    /// Char range of the segment's text.
    pub text: std::ops::Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OrSegments {
    pub segments: Vec<OrSegment>,
}

impl OrSegments {
    pub fn parse(chars: &[char]) -> Self {
        let mut segments = Vec::new();
        let mut seg_start = 0usize;
        let mut joiner: Option<std::ops::Range<usize>> = None;
        let mut i = 0;
        while i + 4 <= chars.len() {
            if chars[i] == ' ' && chars[i + 1] == 'O' && chars[i + 2] == 'R' && chars[i + 3] == ' '
            {
                segments.push(OrSegment {
                    joiner: joiner.take(),
                    text: seg_start..i,
                });
                joiner = Some(i..i + 4);
                seg_start = i + 4;
                i += 4;
            } else {
                i += 1;
            }
        }
        segments.push(OrSegment {
            joiner,
            text: seg_start..chars.len(),
        });
        Self { segments }
    }

    /// Char flags marking every char belonging to a joiner.
    pub fn joiner_flags(&self, len: usize) -> Vec<bool> {
        let mut flags = vec![false; len];
        for segment in &self.segments {
            if let Some(joiner) = &segment.joiner {
                for flag in &mut flags[joiner.clone()] {
                    *flag = true;
                }
            }
        }
        flags
    }

    /// The future popup's handle: remove segment `i` together with its
    /// adjacent joiner (the one before it; for the first segment, the one
    /// after). Returns the new value.
    pub fn remove_segment(&self, value: &str, i: usize) -> String {
        let chars: Vec<char> = value.chars().collect();
        let Some(segment) = self.segments.get(i) else {
            return value.to_string();
        };
        let (cut_start, cut_end) = match &segment.joiner {
            Some(joiner) => (joiner.start, segment.text.end),
            None => {
                // First segment: swallow the joiner AFTER it, if any.
                let next_start = self
                    .segments
                    .get(i + 1)
                    .map(|next| next.text.start)
                    .unwrap_or(segment.text.end);
                (segment.text.start, next_start)
            }
        };
        chars[..cut_start].iter().chain(&chars[cut_end..]).collect()
    }
}

/// Char flags for `` OR `` joiners (thin wrapper over [`OrSegments`]).
pub fn or_separator_flags(chars: &[char]) -> Vec<bool> {
    OrSegments::parse(chars).joiner_flags(chars.len())
}

pub fn wildcard_or_contains(value: &str, pattern: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    // Allocation-free fast path: plain ASCII substring search. This runs once
    // per commit per searched field on a full-history scan — the allocating
    // fallback below costs >1 s on a 568k-commit repo, this costs ~50 ms.
    if !pattern.contains(['*', '?']) && value.is_ascii() && pattern.is_ascii() {
        let value = value.as_bytes();
        let pattern = pattern.as_bytes();
        return pattern.len() <= value.len()
            && value
                .windows(pattern.len())
                .any(|window| window.eq_ignore_ascii_case(pattern));
    }
    let value: Vec<char> = value.to_lowercase().chars().collect();
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    if !pattern.iter().any(|ch| matches!(ch, '*' | '?')) {
        return value.windows(pattern.len()).any(|window| window == pattern)
            || value.iter().eq(pattern.iter());
    }
    let mut states = vec![false; value.len() + 1];
    states[0] = true;
    for token in pattern {
        let mut next = vec![false; value.len() + 1];
        match token {
            '*' => {
                next[0] = states[0];
                for index in 1..=value.len() {
                    next[index] = states[index] || next[index - 1];
                }
            }
            '?' => {
                next[1..=value.len()].copy_from_slice(&states[..value.len()]);
            }
            literal => {
                for index in 1..=value.len() {
                    next[index] = states[index - 1] && value[index - 1] == literal;
                }
            }
        }
        states = next;
    }
    states[value.len()]
}

#[cfg(test)]
mod tests {
    use super::{SearchField, SearchState, wildcard_or_contains};

    use super::fuzzy_rank;

    #[test]
    fn substring_only_rejects_scattered_matches() {
        use super::fuzzy_rank_scored;
        let hashes = ["a1b2c3d4", "abc12345"];
        // Substring-only: "abc" matches only the contiguous run.
        let strict: Vec<String> = fuzzy_rank_scored(hashes.iter().copied(), "abc", 8, true)
            .into_iter()
            .map(|(_, c, _)| c)
            .collect();
        assert_eq!(strict, vec!["abc12345"]);
        // Fuzzy mode would also take the scattered a..b..c.
        let loose = fuzzy_rank_scored(hashes.iter().copied(), "abc", 8, false);
        assert_eq!(loose.len(), 2);
    }

    #[test]
    fn match_ranges_case_insensitive_merged_and_wildcard_skipped() {
        use super::match_ranges;
        // Case-insensitive, all occurrences.
        assert_eq!(
            match_ranges("Fix FIX fix", &["fix"]),
            vec![(0, 3), (4, 7), (8, 11)]
        );
        // Overlapping terms merge into one range.
        assert_eq!(match_ranges("abcde", &["abc", "cde"]), vec![(0, 5)]);
        // Wildcard terms don't highlight; empty terms ignored.
        assert_eq!(
            match_ranges("abc", &["a*c", ""]),
            Vec::<(usize, usize)>::new()
        );
        assert_eq!(match_ranges("", &["x"]), Vec::<(usize, usize)>::new());
    }

    #[test]
    fn executed_terms_category_includes_quick() {
        use super::ExecutedTerms;
        let t = ExecutedTerms {
            quick: vec!["q".into()],
            hash: vec!["h".into()],
            message: vec!["m".into()],
            author: vec!["a".into()],
            file: vec!["src/f.rs".into()],
            date_bounded: true,
        };
        assert_eq!(t.for_message(), vec!["m", "q"]);
        assert_eq!(t.for_hash(), vec!["h", "q"]);
        assert_eq!(t.for_author(), vec!["a", "q"]);
        assert_eq!(
            t.for_file(),
            vec!["src/f.rs"],
            "quick terms don't apply to files"
        );
        // Date-bounded search marks the day part; quick hits merge in.
        assert_eq!(t.date_ranges("2020-12-25 18:31"), vec![(0, 10)]);
        let quick_hit = ExecutedTerms {
            quick: vec!["18:31".into()],
            ..Default::default()
        };
        assert_eq!(quick_hit.date_ranges("2020-12-25 18:31"), vec![(11, 16)]);
    }

    #[test]
    fn input_edit_ops_cover_joiners_and_dates() {
        use super::input_edit::{
            backspace, date_backspace, date_input, delete, insert, step_cursor,
        };
        // Cursor hops the whole " OR ".
        assert_eq!(step_cursor("alice OR bob", 9, false), 5);
        assert_eq!(step_cursor("alice OR bob", 5, true), 9);
        assert_eq!(step_cursor("ab", 0, false), 0, "clamped left");
        assert_eq!(step_cursor("ab", 2, true), 2, "clamped right");
        // Atomic OR backspace joins texts; plain mode eats one char.
        assert_eq!(backspace("alice OR bob", 9, true), ("alicebob".into(), 5));
        assert_eq!(
            backspace("alice OR bob", 9, false),
            ("alice ORbob".into(), 8)
        );
        assert_eq!(backspace("x", 0, true), ("x".into(), 0), "cursor 0 no-op");
        assert_eq!(delete("abc", 1), ("ac".into(), 1));
        assert_eq!(delete("abc", 3), ("abc".into(), 3), "past end no-op");
        assert_eq!(insert("ac", 1, 'b'), ("abc".into(), 2));
        // Date mask: digits extend with dashes, junk and impossible swallowed.
        assert_eq!(date_input("2020-1", '2'), Some("2020-12".into()));
        assert_eq!(date_input("2020-1", '9'), None, "month 19 impossible");
        assert_eq!(date_input("2020", 'x'), None);
        assert_eq!(date_backspace("2020-12"), "2020-1");
        assert_eq!(date_backspace(""), "");
    }

    #[test]
    fn search_query_parse_validates_and_splits() {
        use super::SearchQuery;
        let mut values: [String; 9] = Default::default();
        assert_eq!(
            SearchQuery::parse(&values),
            Err("Enter search text.".into())
        );

        values[SearchField::Message.index()] = "feat OR api endpoints".into();
        values[SearchField::File.index()] = "a.rs, b.rs OR c.rs".into();
        values[SearchField::DateFrom.index()] = "2020-12-25".into();
        let query = SearchQuery::parse(&values).unwrap();
        assert_eq!(query.message, vec!["feat", "api endpoints"]);
        assert_eq!(
            query.file_patterns,
            vec!["a.rs", "b.rs", "c.rs"],
            "comma AND OR split"
        );
        assert_eq!(query.date_start.as_deref(), Some("2020-12-25"));
        assert_eq!(query.date_end, None);
        assert!(query.executed_terms().date_bounded);

        values[SearchField::DateTo.index()] = "2020-13-01".into();
        assert!(
            SearchQuery::parse(&values).is_err(),
            "impossible date rejected"
        );
    }

    #[test]
    fn search_query_matches_is_pure_and_anded() {
        use super::SearchQuery;
        use std::collections::HashSet;
        let commit = crate::git::CommitInfo {
            hash: "abcdef1234567890".into(),
            message: "feat: add endpoints".into(),
            full_message: None,
            author_name: "Alice".into(),
            author_email: String::new(),
            timestamp: 1608910260, // 2020-12-25 15:31 UTC
            offset_minutes: 0,
            parent_indices: Some(vec![]),
            is_merge: false,
            graph_prefix: String::new(),
            graph_prefix_lanes: Vec::new(),
        };
        let query = SearchQuery {
            message: vec!["feat".into()],
            author: vec!["alice".into()],
            date_start: Some("2020-12-25".into()),
            date_end: Some("2020-12-25".into()),
            ..Default::default()
        };
        assert!(query.matches(&commit, None, None, None));
        // One failing dimension kills the match (AND).
        let mut wrong_author = query.clone();
        wrong_author.author = vec!["bob".into()];
        assert!(!wrong_author.matches(&commit, None, None, None));
        // A hash set restricts to membership.
        let empty: HashSet<String> = HashSet::new();
        assert!(!query.matches(&commit, Some(&empty), None, None));
        let with: HashSet<String> = [commit.hash.clone()].into_iter().collect();
        assert!(query.matches(&commit, Some(&with), None, None));
    }

    #[test]
    fn parallel_filter_indices_keeps_order() {
        use super::parallel_filter_indices;
        let items: Vec<usize> = (0..1000).collect();
        let picked = parallel_filter_indices(&items, |n| *n % 7 == 0);
        let expected: Vec<usize> = (0..1000).filter(|n| n % 7 == 0).collect();
        assert_eq!(picked, expected);
        assert!(parallel_filter_indices(&[] as &[usize], |_| true).is_empty());
    }

    #[test]
    fn file_search_metric_names_cover_modes() {
        use super::FileAction;
        use crate::app::App;
        assert_eq!(
            App::file_search_metric(true, FileAction::All),
            "search.files full+All"
        );
        assert_eq!(
            App::file_search_metric(false, FileAction::All),
            "search.files fast+All"
        );
        assert_eq!(
            App::file_search_metric(true, FileAction::Deleted),
            "search.files full+Deleted"
        );
        assert_eq!(
            App::file_search_metric(false, FileAction::Renamed),
            "search.files fast+Renamed"
        );
    }

    #[test]
    fn fuzzy_rank_multi_term_any_order() {
        let files = ["src/git/history.rs", "src/app/state.rs", "README.md"];
        let ranked = fuzzy_rank(files.iter().copied(), "hist git", 8);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].0, "src/git/history.rs");
        // Highlight covers both terms' chars.
        assert!(!ranked[0].1.is_empty());
    }

    #[test]
    fn fuzzy_rank_prefers_contiguous_and_shorter() {
        let files = ["src/git/graph_engine.rs", "src/git/graph.rs"];
        let ranked = fuzzy_rank(files.iter().copied(), "graph", 8);
        assert_eq!(
            ranked[0].0, "src/git/graph.rs",
            "shorter contiguous match first"
        );
        assert_eq!(ranked.len(), 2);
    }

    #[test]
    fn fuzzy_rank_subsequence_fallback_and_rejection() {
        let files = ["src/ui/screens/commit_list.rs"];
        // "uicl" is no substring but is an in-order subsequence.
        assert_eq!(fuzzy_rank(files.iter().copied(), "uicl", 8).len(), 1);
        assert!(fuzzy_rank(files.iter().copied(), "zzz", 8).is_empty());
    }

    #[test]
    fn fuzzy_rank_empty_query_lists_head() {
        let files = ["a.rs", "b.rs", "c.rs"];
        let ranked = fuzzy_rank(files.iter().copied(), "", 2);
        assert_eq!(ranked.len(), 2);
        assert!(ranked.iter().all(|(_, positions)| positions.is_empty()));
    }

    #[test]
    fn matches_contains_and_wildcards() {
        assert!(wildcard_or_contains("Fix Login Bug", "login"));
        assert!(wildcard_or_contains("Fix Login Bug", "F*B?g"));
        assert!(!wildcard_or_contains("Fix Login Bug", "logout"));
    }

    use super::{date_digits_plausible, format_date_digits};

    #[test]
    fn date_mask_formats_and_filters() {
        assert_eq!(format_date_digits("2024"), "2024");
        assert_eq!(format_date_digits("20240"), "2024-0");
        assert_eq!(format_date_digits("20240115"), "2024-01-15");
        assert_eq!(format_date_digits("2024x01!15abc"), "2024-01-15");
        assert_eq!(format_date_digits("202401159999"), "2024-01-15"); // capped
    }

    #[test]
    fn date_mask_rejects_impossible_dates() {
        assert!(date_digits_plausible("2024"));
        assert!(date_digits_plausible("20241")); // month 1x could be 10-12
        assert!(!date_digits_plausible("20242")); // month 2x impossible
        assert!(!date_digits_plausible("202400")); // month 00
        assert!(!date_digits_plausible("202413")); // month 13
        assert!(date_digits_plausible("2024013")); // day 3x could be 30/31
        assert!(!date_digits_plausible("2024014")); // day 4x impossible
        assert!(!date_digits_plausible("20240230")); // Feb 30
        assert!(date_digits_plausible("20240229")); // 2024 leap
        assert!(!date_digits_plausible("20230229")); // 2023 not leap
    }

    /// Focusing an empty "Date to" copies "Date from"; END shows only when
    /// both are empty.
    #[test]
    fn date_to_prefills_from_date_from_on_focus() {
        let mut search = SearchState::default();
        search.set_field(SearchField::DateTo);
        assert_eq!(search.value(SearchField::DateTo), "", "no From, no prefill");
        search.set_field(SearchField::DateFrom);
        *search.value_mut() = "2024-01-15".into();
        search.set_field(SearchField::DateTo);
        assert_eq!(search.value(SearchField::DateTo), "2024-01-15");
        // Non-empty To is never overwritten.
        *search.value_mut() = "2024-02-01".into();
        search.set_field(SearchField::DateFrom);
        search.set_field(SearchField::DateTo);
        assert_eq!(search.value(SearchField::DateTo), "2024-02-01");
    }

    #[test]
    fn or_segments_parse_and_remove() {
        use super::OrSegments;
        let value = "alice OR bob OR carol";
        let chars: Vec<char> = value.chars().collect();
        let parsed = OrSegments::parse(&chars);
        assert_eq!(parsed.segments.len(), 3);
        assert_eq!(&value[0..5], "alice");
        assert!(parsed.segments[0].joiner.is_none());
        assert!(parsed.segments[1].joiner.is_some());

        // Handles: removing a middle/last segment takes its LEADING joiner;
        // removing the first takes the joiner AFTER it.
        assert_eq!(parsed.remove_segment(value, 1), "alice OR carol");
        assert_eq!(parsed.remove_segment(value, 2), "alice OR bob");
        assert_eq!(parsed.remove_segment(value, 0), "bob OR carol");
        // Single segment: removal empties the value.
        let single: Vec<char> = "alice".chars().collect();
        assert_eq!(OrSegments::parse(&single).remove_segment("alice", 0), "");
    }

    #[test]
    fn or_separator_flags_mark_runs_only() {
        use super::or_separator_flags;
        let chars: Vec<char> = "ab OR cd".chars().collect();
        let flags = or_separator_flags(&chars);
        assert_eq!(
            flags,
            vec![false, false, true, true, true, true, false, false]
        );
        // Cursor positions 3,4,5 split the joiner (a| OR, O|R, R| ) — the
        // mouse handler skips exactly the positions where both neighbors are
        // flagged; boundaries 2 and 6 stay clickable.
        for pos in [3usize, 4, 5] {
            assert!(flags[pos] && flags[pos - 1], "pos {pos} must be inside");
        }
        assert!(!(flags[2] && flags[1]), "leading boundary clickable");
        assert!(
            !(flags.get(6).copied().unwrap_or(false) && flags[5]),
            "trailing boundary clickable"
        );
        // No false positives without the spaces.
        let plain: Vec<char> = "TORN".chars().collect();
        assert!(or_separator_flags(&plain).iter().all(|f| !f));
    }

    #[test]
    fn file_field_matches_the_segment_after_the_last_comma() {
        let mut search = SearchState::default();
        search.set_field(SearchField::File);
        *search.value_mut() = "src/main.rs, src/li".into();
        assert_eq!(search.active_prefix(), "src/li");
        // Non-File fields keep whole-value matching.
        search.set_field(SearchField::Author);
        *search.value_mut() = "a, b".into();
        assert_eq!(search.active_prefix(), "a, b");
    }
}

// ── search execution and autocomplete (Mode::Search) ────────────────────────

use crate::app::App;
use crate::app::models::Mode;
use crate::git;
use chrono::NaiveDate;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Can this digit prefix still become a valid date? Month must be 01-12,
/// day starts 0-3, and a complete date must actually exist (no Feb 30).
fn date_digits_plausible(digits: &str) -> bool {
    let digits: Vec<u32> = digits.chars().filter_map(|c| c.to_digit(10)).collect();
    let n = digits.len();
    if n > 8 {
        return false;
    }
    if n >= 5 && digits[4] > 1 {
        return false; // month first digit 0 or 1
    }
    if n >= 6 {
        let month = digits[4] * 10 + digits[5];
        if month == 0 || month > 12 {
            return false;
        }
    }
    if n >= 7 && digits[6] > 3 {
        return false; // day first digit 0-3
    }
    if n == 8 {
        let text = format!(
            "{}{}{}{}-{}{}-{}{}",
            digits[0], digits[1], digits[2], digits[3], digits[4], digits[5], digits[6], digits[7]
        );
        return NaiveDate::parse_from_str(&text, "%Y-%m-%d").is_ok();
    }
    true
}

/// Input mask for the date fields: digits only, dashes inserted after YYYY
/// and MM automatically, capped at 8 digits (YYYY-MM-DD).
fn format_date_digits(digits: &str) -> String {
    let digits: String = digits
        .chars()
        .filter(char::is_ascii_digit)
        .take(8)
        .collect();
    let mut out = String::with_capacity(10);
    for (i, ch) in digits.chars().enumerate() {
        if i == 4 || i == 6 {
            out.push('-');
        }
        out.push(ch);
    }
    out
}

fn validate_date(input: &str) -> Result<Option<String>, String> {
    let value = input.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if NaiveDate::parse_from_str(value, "%Y-%m-%d").is_err() {
        return Err(format!("Invalid date '{value}', use YYYY-MM-DD."));
    }
    Ok(Some(value.to_string()))
}

/// How many ranked suggestions to fetch — enough for a few "..." pages.
const SUGGESTION_FETCH: usize = 30;

impl App {
    pub(crate) fn open_search(&mut self) {
        // Opens during progressive loading too — inputs can be typed while
        // history streams in; execute_search tells the user if it's too early.
        self.search.set_field(SearchField::Quick);
        self.search.button_focus = None;
        self.search.notice = None;
        self.search.open_backup = Some(self.search.values.clone());
        if self.search_file_candidates.is_empty()
            && let Ok(files) = git::list_repo_files(&self.repo_path)
        {
            self.search_file_candidates = files;
        }
        self.nav.push(Mode::Search);
    }

    /// Fuzzy-ranked suggestions for the active field, with matched char
    /// positions for highlighting. Branch/Tag/File rank over their small
    /// candidate lists; Hash/Message/Author scan the whole commit list in
    /// parallel chunks, gated on a minimum query length and cached per
    /// (field, query, list length).
    pub(crate) fn search_suggestions(&mut self) -> Vec<(String, Vec<usize>)> {
        let field = self.search.field;
        let query = self.search.active_prefix().trim().to_string();
        let min_query = match field {
            SearchField::Hash => 3,
            SearchField::Message | SearchField::Quick => 2,
            SearchField::Author => 0,
            SearchField::File | SearchField::Branch | SearchField::Tag => 0,
            SearchField::DateFrom | SearchField::DateTo => return Vec::new(),
        };
        if query.chars().count() < min_query {
            return Vec::new();
        }
        if let Some((f, q, len, cached)) = &self.search.suggestion_cache
            && *f == field
            && *q == query
            && *len == self.commits.len()
        {
            return cached.clone();
        }
        let results = match field {
            SearchField::File => fuzzy_rank(
                self.search_file_candidates.iter().map(String::as_str),
                &query,
                SUGGESTION_FETCH,
            ),
            SearchField::Branch | SearchField::Tag => {
                let names = self.ref_names_for_field(field);
                fuzzy_rank(names.iter().map(String::as_str), &query, SUGGESTION_FETCH)
            }
            // Hashes: substring matches only — scattered hex is meaningless.
            SearchField::Hash => self.fuzzy_rank_commits(|c| c.hash.as_str(), &query, true),
            // Quick's suggestion corpus: commit messages (its dominant use).
            SearchField::Message | SearchField::Quick => {
                self.fuzzy_rank_commits(|c| c.message.as_str(), &query, false)
            }
            SearchField::Author => {
                self.fuzzy_rank_commits(|c| c.author_name.as_str(), &query, false)
            }
            SearchField::DateFrom | SearchField::DateTo => Vec::new(),
        };
        self.search.suggestion_page_start = 0;
        self.search.suggestion_cache = Some((field, query, self.commits.len(), results.clone()));
        results
    }

    /// Chunked-parallel fuzzy ranking over a per-commit string field; the
    /// per-chunk top-8s are merged, deduped (messages and authors repeat)
    /// and re-ranked.
    fn fuzzy_rank_commits<F>(
        &self,
        extract: F,
        query: &str,
        substring_only: bool,
    ) -> Vec<(String, Vec<usize>)>
    where
        F: Fn(&git::CommitInfo) -> &str + Sync,
    {
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(1, 8);
        let chunk_size = self.commits.len().div_ceil(workers).max(1);
        let per_chunk: Vec<Vec<(i64, String, Vec<usize>)>> = std::thread::scope(|scope| {
            let extract = &extract;
            let handles: Vec<_> = self
                .commits
                .chunks(chunk_size)
                .map(|chunk| {
                    scope.spawn(move || {
                        fuzzy_rank_scored(
                            chunk.iter().map(extract),
                            query,
                            SUGGESTION_FETCH,
                            substring_only,
                        )
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("suggestion worker panicked"))
                .collect()
        });
        let mut merged: Vec<(i64, String, Vec<usize>)> = per_chunk.into_iter().flatten().collect();
        merged.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let mut seen = HashSet::new();
        merged
            .into_iter()
            .filter(|(_, candidate, _)| seen.insert(candidate.clone()))
            .take(SUGGESTION_FETCH)
            .map(|(_, candidate, positions)| (candidate, positions))
            .collect()
    }

    /// Hashes of the commits that matching branch/tag refs POINT AT. n/p then
    /// hops ref to ref and `f` filters to the labeled commits — matching what
    /// a person means by "search tags". (The old semantics — every commit
    /// REACHABLE from the ref — made a tag near HEAD match the entire history:
    /// n/p crawled commit-by-commit and the filter appeared to do nothing.)
    fn ref_target_hashes(
        &self,
        values: &[String; 9],
        field: SearchField,
    ) -> Result<Option<HashSet<String>>, String> {
        let query = values[field.index()].trim();
        if query.is_empty() {
            return Ok(None);
        }
        let want_tag = field == SearchField::Tag;
        let parts: Vec<&str> = query
            .split(" OR ")
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect();
        let targets: Vec<String> = self
            .ref_labels
            .iter()
            .filter(|(_, labels)| {
                labels.iter().any(|label| {
                    (label.kind == git::RefKind::Tag) == want_tag
                        && parts
                            .iter()
                            .any(|part| wildcard_or_contains(&label.name, part))
                })
            })
            .map(|(hash, _)| hash.clone())
            .collect();
        if targets.is_empty() {
            return Err(format!(
                "No {} matches '{query}'.",
                field.label().to_lowercase()
            ));
        }
        // The walk is HEAD-only, so an unmerged branch (or a remote ahead of
        // local) points at a commit that is NOT in the loaded list. Resolve
        // such refs to their nearest ancestor that IS loaded — the branch
        // point — instead of silently matching nothing.
        let mut hashes: HashSet<String> = HashSet::new();
        let mut missing: Vec<String> = Vec::new();
        for hash in targets {
            if self.commit_index.contains_key(&hash) {
                hashes.insert(hash);
            } else {
                missing.push(hash);
            }
        }
        if !missing.is_empty()
            && let Ok(repo) = git2::Repository::discover(&self.repo_path)
        {
            const MAX_REFS: usize = 20;
            const MAX_WALK: usize = 100_000;
            for tip in missing.into_iter().take(MAX_REFS) {
                let Ok(oid) = git2::Oid::from_str(&tip) else {
                    continue;
                };
                let mut stack = vec![oid];
                let mut seen: HashSet<git2::Oid> = HashSet::new();
                let mut steps = 0usize;
                while let Some(oid) = stack.pop() {
                    if !seen.insert(oid) {
                        continue;
                    }
                    steps += 1;
                    if steps > MAX_WALK {
                        break;
                    }
                    let hex = oid.to_string();
                    if self.commit_index.contains_key(&hex) {
                        hashes.insert(hex);
                        break;
                    }
                    if let Ok(commit) = repo.find_commit(oid) {
                        stack.extend(commit.parent_ids());
                    }
                }
            }
        }
        if hashes.is_empty() {
            return Err(format!(
                "'{query}' matches refs, but none of their commits are in the loaded history."
            ));
        }
        Ok(Some(hashes))
    }

    /// Sorted, deduped ref names for the Branch (local + remote) or Tag field.
    fn ref_names_for_field(&self, field: SearchField) -> Vec<String> {
        let want_tag = field == SearchField::Tag;
        let mut names: Vec<String> = self
            .ref_labels
            .values()
            .flatten()
            .filter(|label| (label.kind == git::RefKind::Tag) == want_tag)
            .map(|label| label.name.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Apply the first candidate that differs from the current input.
    /// Returns `false` when there was nothing to complete.
    pub(crate) fn autocomplete_search(&mut self) -> bool {
        // Nothing typed in the active segment: Tab means "next input", not
        // "take the first suggestion" (Author/File/Branch/Tag list
        // suggestions even for an empty query).
        if self.search.active_prefix().trim().is_empty() {
            return false;
        }
        let candidates: Vec<String> = self
            .search_suggestions()
            .into_iter()
            .map(|(candidate, _)| candidate)
            .collect();
        let current = self.search.active_prefix().to_lowercase();
        if let Some(candidate) = candidates
            .into_iter()
            .find(|candidate| candidate.to_lowercase() != current)
        {
            self.apply_completion(&candidate);
            true
        } else {
            false
        }
    }

    /// Put a chosen suggestion into the active field (Tab or a click on the
    /// Suggestions line). File keeps the segments before the last comma,
    /// completing only the tail; other fields are replaced whole.
    pub(crate) fn apply_completion(&mut self, candidate: &str) {
        let value = self.search.value(self.search.field);
        // Last separator wins; File also treats commas as separators.
        let mut cut: Option<usize> = SearchState::multi_value_separator(self.search.field)
            .and_then(|sep| value.rfind(sep).map(|pos| pos + sep.len()));
        if self.search.field == SearchField::File
            && let Some(comma) = value.rfind(',')
        {
            cut = Some(cut.map_or(comma + 1, |c| c.max(comma + 1)));
        }
        let kept = cut.map(|c| &value[..c]).unwrap_or("");
        let completed = if kept.is_empty() {
            candidate.to_string()
        } else if kept.ends_with(',') {
            format!("{kept} {candidate}")
        } else {
            format!("{kept}{candidate}") // " OR " already carries spacing
        };
        *self.search.value_mut() = completed;
        self.search.cursor = self.search.value(self.search.field).chars().count();
        self.search.reset_backup = None;
    }

    /// --stats metric name for the file-search git call: full/fast history
    /// crossed with the action filter (static strings for push_perf_metric).
    fn file_search_metric(full_history: bool, action: FileAction) -> &'static str {
        use FileAction as A;
        match (full_history, action) {
            (true, A::All) => "search.files full+All",
            (true, A::Added) => "search.files full+Added",
            (true, A::Modified) => "search.files full+Modified",
            (true, A::Deleted) => "search.files full+Deleted",
            (true, A::Renamed) => "search.files full+Renamed",
            (false, A::All) => "search.files fast+All",
            (false, A::Added) => "search.files fast+Added",
            (false, A::Modified) => "search.files fast+Modified",
            (false, A::Deleted) => "search.files fast+Deleted",
            (false, A::Renamed) => "search.files fast+Renamed",
        }
    }

    pub(crate) fn execute_search(&mut self) {
        if self.history_loading {
            self.search.notice = Some(format!(
                "History still loading ({} commits so far) — try again in a moment.",
                self.commits.len()
            ));
            return;
        }
        let query = match SearchQuery::parse(&self.search.values) {
            Ok(query) => query,
            Err(err) => {
                self.search.notice = Some(err);
                return;
            }
        };
        self.search.notice = None;

        // Hash sets that need the repo: file pathspec log + branch/tag refs.
        let search_started = std::time::Instant::now();
        let mut file_search_elapsed = None;
        let file_hashes = if query.file_patterns.is_empty() {
            None
        } else {
            let patterns: Vec<&str> = query.file_patterns.iter().map(String::as_str).collect();
            let file_started = std::time::Instant::now();
            match git::matching_commit_hashes_for_file(
                &self.repo_path,
                &patterns,
                self.search.file_full_history,
                self.search.file_action.diff_filter(),
            ) {
                Ok(hashes) => {
                    file_search_elapsed = Some(file_started.elapsed());
                    Some(hashes)
                }
                Err(err) => {
                    self.search.notice = Some(format!("Search error: {err}"));
                    return;
                }
            }
        };
        let values = self.search.values.clone();
        let branch_hashes = match self.ref_target_hashes(&values, SearchField::Branch) {
            Ok(hashes) => hashes,
            Err(err) => {
                self.search.notice = Some(err);
                return;
            }
        };
        let tag_hashes = match self.ref_target_hashes(&values, SearchField::Tag) {
            Ok(hashes) => hashes,
            Err(err) => {
                self.search.notice = Some(err);
                return;
            }
        };

        self.search.clear_matches();
        self.search.executed_terms = Some(query.executed_terms());
        self.search.matches = parallel_filter_indices(&self.commits, |commit| {
            query.matches(
                commit,
                file_hashes.as_ref(),
                branch_hashes.as_ref(),
                tag_hashes.as_ref(),
            )
        });
        self.search.match_pos = 0;
        self.search.active_file_patterns = query.file_patterns.clone();
        self.file_status_cache.clear();
        self.report_search_timing(search_started.elapsed(), file_search_elapsed);
    }

    /// Timing metrics + user-facing result messages, then close-or-stay.
    fn report_search_timing(
        &mut self,
        total_elapsed: std::time::Duration,
        file_search_elapsed: Option<std::time::Duration>,
    ) {
        // Timing: total search + the file-search git call, split by
        // full/fast history and action filter (--stats picks these up; the
        // messages below let the user compare modes interactively).
        self.push_perf_metric("search.execute", total_elapsed);
        if let Some(elapsed) = file_search_elapsed {
            self.push_perf_metric(
                Self::file_search_metric(self.search.file_full_history, self.search.file_action),
                elapsed,
            );
        }
        let timing_suffix = match file_search_elapsed {
            Some(elapsed) => format!(
                "{} ms (files {} ms, {} history, action {:?})",
                total_elapsed.as_millis(),
                elapsed.as_millis(),
                if self.search.file_full_history {
                    "full"
                } else {
                    "fast"
                },
                self.search.file_action,
            ),
            None => format!("{} ms", total_elapsed.as_millis()),
        };
        if self.search.matches.is_empty() {
            // Stay in the popup so the inputs can be adjusted; the status
            // line is hidden behind it, so tell the user right here.
            self.search.notice = Some(format!(
                "No matches in {timing_suffix}. Adjust the search or Esc to close."
            ));
            if self.search.filter_active {
                self.search.filter_active = false;
                self.rebuild_visible();
            }
            return;
        }
        tracing::debug!(
            "search: {} matches in {timing_suffix}",
            self.search.matches.len()
        );
        self.status_message = Some(format!(
            "Search: {} matches in {timing_suffix}",
            self.search.matches.len()
        ));
        self.search.open_backup = None; // search accepted: keep the edits
        self.nav.pop();
        if self.search.filter_active {
            // A live filter follows the new result set.
            self.rebuild_visible();
        }
        if let Some(&commit_idx) = self.search.matches.first() {
            self.select_commit_idx(commit_idx);
            self.commit_diff_scroll = 0;
            self.commit_diff_info_scroll = 0;
            self.schedule_selected_stats_load();
            // Match info in the status line is DERIVED from search state at
            // render time (status_line_content); the timing message set
            // above is a one-shot transient, which is what status_message
            // is for.
        }
    }

    /// Clear the whole search from the list (key: F): matches, filter, done.
    /// Popup inputs stay put — Undo Reset in the popup brings them back.
    pub(crate) fn clear_search_results(&mut self) {
        let had_anything = !self.search.matches.is_empty() || self.search.filter_active;
        self.search.clear_matches();
        if self.search.filter_active {
            self.search.filter_active = false;
            self.rebuild_visible();
        }
        self.status_message = Some(if had_anything {
            "Search cleared.".into()
        } else {
            "No active search.".into()
        });
    }

    /// Toggle showing only search matches in the commit list (key: f).
    pub(crate) fn toggle_search_filter(&mut self) {
        if !self.search.filter_active && self.search.matches.is_empty() {
            self.status_message = Some("No search matches to filter. /: search first.".into());
            return;
        }
        let keep = self.selected_commit().map(|c| c.hash.clone());
        self.search.filter_active = !self.search.filter_active;
        self.rebuild_visible();
        let keep_idx = keep.as_deref().and_then(|h| self.commit_position(h));
        if self.search.filter_active {
            let target = keep_idx
                .filter(|idx| self.search.matches.binary_search(idx).is_ok())
                .or_else(|| self.search.matches.first().copied());
            if let Some(idx) = target {
                self.select_commit_idx(idx);
            }
            self.status_message = Some(format!(
                "Filter: showing {} matched commit{}. f: remove filter.",
                self.search.matches.len(),
                if self.search.matches.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ));
        } else {
            if let Some(idx) = keep_idx {
                self.select_commit_idx(idx);
            }
            self.status_message = Some("Filter removed.".into());
        }
        self.schedule_selected_stats_load();
    }

    pub(crate) fn move_search_match(&mut self, forward: bool) {
        let Some(commit_idx) = self.search.next_match(forward) else {
            self.status_message = Some("Search: no matches.".into());
            return;
        };
        self.select_commit_idx(commit_idx);
        self.commit_diff_scroll = 0;
        self.commit_diff_info_scroll = 0;
        self.schedule_selected_stats_load();
        self.status_message = None; // derived segment shows the live position
    }

    pub(crate) fn paste_search_text(&mut self, text: &str) {
        let mut chars: Vec<char> = self.search.value(self.search.field).chars().collect();
        let insert: Vec<char> = text.replace('\n', " ").chars().collect();
        let cursor = self.search.cursor.min(chars.len());
        chars.splice(cursor..cursor, insert.iter().copied());
        self.search.cursor = cursor + insert.len();
        *self.search.value_mut() = chars.into_iter().collect();
        if !insert.is_empty() {
            self.search.reset_backup = None;
        }
    }

    /// Bottom slots: 3 buttons, plus the full-history checkbox while the
    /// file field has text.
    pub(crate) fn search_button_count(&self) -> usize {
        if self.search.value(SearchField::File).trim().is_empty() {
            4
        } else {
            6
        }
    }

    /// Move focus through the popup ring: 9 fields then the bottom slots.
    fn search_focus_step(&mut self, forward: bool) {
        let fields = SearchField::ALL.len();
        let ring = fields + self.search_button_count();
        let pos = match self.search.button_focus {
            Some(button) => fields + button,
            None => self.search.field.index(),
        };
        let next = if forward {
            (pos + 1) % ring
        } else {
            (pos + ring - 1) % ring
        };
        if next < fields {
            self.search.button_focus = None;
            self.search.set_field(SearchField::ALL[next]);
        } else {
            self.search.button_focus = Some(next - fields);
        }
    }

    /// Semantics of a click inside the search popup. The mouse handler
    /// resolves pixels first (hit target, char position, double-click) so
    /// this is deterministic and testable without a terminal.
    pub(crate) fn apply_search_popup_hit(
        &mut self,
        hit: crate::app::layout::SearchPopupHit,
        clicked_char_pos: Option<usize>,
        double_click: bool,
        popup_inner_width: u16,
    ) {
        use crate::app::layout::SearchPopupHit;
        self.search.notice = None;
        match hit {
            SearchPopupHit::Field(index) => {
                let field = SearchField::ALL[index];
                self.search.set_field(field);
                self.search.button_focus = None;
                if let Some(pos) = clicked_char_pos {
                    let chars: Vec<char> = self.search.value(field).chars().collect();
                    // A click INSIDE an " OR " joiner would split it ("O|R") —
                    // those clicks open the remove-block confirm instead.
                    let flags = or_separator_flags(&chars);
                    let inside_separator = pos > 0
                        && pos < chars.len()
                        && flags.get(pos).copied().unwrap_or(false)
                        && flags.get(pos - 1).copied().unwrap_or(false);
                    if inside_separator {
                        let parsed = OrSegments::parse(&chars);
                        if let Some(seg_idx) = parsed.segments.iter().position(|seg| {
                            seg.joiner
                                .as_ref()
                                .is_some_and(|j| pos > j.start && pos < j.end)
                        }) {
                            let text: String = chars[parsed.segments[seg_idx].text.clone()]
                                .iter()
                                .collect();
                            let mut preview = format!("OR {}", text.trim());
                            if preview.chars().count() > 40 {
                                preview = preview.chars().take(37).collect::<String>() + "...";
                            }
                            self.nav.push(Mode::Confirm(
                                crate::app::models::ConfirmAction::RemoveOrSegment {
                                    field_index: index,
                                    segment_index: seg_idx,
                                    preview,
                                },
                                true,
                            ));
                        }
                    } else {
                        self.search.cursor = pos.min(chars.len());
                    }
                }
                if double_click && !self.search.value(field).is_empty() {
                    self.search.select_all = true;
                }
            }
            SearchPopupHit::ClearField(index) => {
                let field = SearchField::ALL[index];
                self.search.button_focus = None;
                if field == SearchField::DateFrom {
                    // Date row [ clear ] is staged: To first, then From.
                    self.clear_date_row_staged();
                } else {
                    // Multi-value fields clear one OR segment per click,
                    // from the right; others clear whole.
                    self.clear_field_staged(field);
                }
            }
            SearchPopupHit::Button(button) => {
                self.search.button_focus = Some(button);
                self.activate_search_button(button);
            }
            SearchPopupHit::FullHistoryToggle => {
                self.search.file_full_history = !self.search.file_full_history;
            }
            SearchPopupHit::FileAction(i) => {
                self.search.file_action = FileAction::ALL[i];
            }
            SearchPopupHit::ValueScrollLeft(index) | SearchPopupHit::ValueScrollRight(index) => {
                let field = SearchField::ALL[index];
                // set_field RESETS the cursor to the end — calling it on the
                // already-focused field made every left-click page from the
                // end again (stuck one page back).
                if self.search.field != field {
                    self.search.set_field(field);
                }
                self.search.button_focus = None;
                self.search.select_all = false;
                // Page the cursor; the window follows it (derived). Repeated
                // clicks walk all the way to either end.
                let page = usize::from(popup_inner_width / 2).max(1);
                let len = self.search.value(field).chars().count();
                self.search.cursor = if matches!(hit, SearchPopupHit::ValueScrollLeft(_)) {
                    self.search.cursor.saturating_sub(page)
                } else {
                    (self.search.cursor + page).min(len)
                };
            }
            SearchPopupHit::OrAppend(index) => {
                self.append_or_separator(SearchField::ALL[index]);
            }
            SearchPopupHit::SuggestionsPrev => {
                if let Some((_, _, _, list)) = self.search.suggestion_cache.as_ref() {
                    let widths: Vec<u16> =
                        list.iter().map(|(c, _)| c.chars().count() as u16).collect();
                    self.search.suggestion_page_start = crate::app::layout::previous_page_start(
                        &widths,
                        popup_inner_width,
                        self.search.suggestion_page_start,
                    );
                }
            }
            SearchPopupHit::SuggestionsMore => {
                // Next page starts after the last shown candidate.
                let shown_through = self
                    .search
                    .suggestion_cache
                    .as_ref()
                    .map(|(_, _, _, list)| {
                        let widths: Vec<u16> =
                            list.iter().map(|(c, _)| c.chars().count() as u16).collect();
                        let (placed, _) = crate::app::layout::layout_suggestions(
                            &widths,
                            self.search.suggestion_page_start,
                            popup_inner_width,
                        );
                        placed.last().map(|p| p.index + 1).unwrap_or(0)
                    })
                    .unwrap_or(0);
                self.advance_suggestion_page(shown_through);
            }
            SearchPopupHit::Suggestion(index) => {
                let candidate = self
                    .search
                    .suggestion_cache
                    .as_ref()
                    .and_then(|(_, _, _, list)| list.get(index))
                    .map(|(candidate, _)| candidate.clone());
                if let Some(candidate) = candidate {
                    self.apply_completion(&candidate);
                }
            }
        }
    }

    pub(crate) fn activate_search_button(&mut self, button: usize) {
        match button {
            0 => self.execute_search(),
            1 => self.cancel_search(),
            // '?' types into the inputs, so Search mode blocks the help key
            // (can_open_help) — the [ Help ] button is the way in here.
            3 => self.show_help = true,
            4 => self.search.file_full_history = !self.search.file_full_history,
            5 => self.search.file_action = self.search.file_action.next(),
            _ => {
                if let Some(values) = self.search.reset_backup.take() {
                    // Undo Reset: bring the previous inputs back. Baseline
                    // moves with it — Esc afterwards keeps the restored state.
                    self.search.values = values.clone();
                    self.search.cursor = 0;
                    self.search.open_backup = Some(values);
                } else {
                    // Reset: clear inputs, drop matches (n/p go dead) and any
                    // active filter.
                    self.search.reset_backup = Some(self.search.values.clone());
                    self.search.values = Default::default();
                    self.search.cursor = 0;
                    self.search.clear_matches();
                    if self.search.filter_active {
                        self.search.filter_active = false;
                        self.rebuild_visible();
                    }
                    self.search.notice =
                        Some("Inputs and results cleared. Undo Reset restores inputs.".into());
                    // Reset takes effect immediately: Esc afterwards keeps the
                    // cleared state instead of resurrecting the old inputs.
                    self.search.open_backup = Some(self.search.values.clone());
                }
            }
        }
    }

    /// Cancel the popup (Esc, Cancel button, click outside): edits made in
    /// this popup session are discarded — the inputs return to what they were
    /// when the popup opened. A successful Search keeps them instead.
    pub(crate) fn cancel_search(&mut self) {
        if let Some(values) = self.search.open_backup.take() {
            self.search.values = values;
            self.search.cursor = 0;
        }
        self.search.notice = None;
        self.search.button_focus = None;
        self.nav.pop();
    }

    /// "..." pager: advance the suggestions page, wrapping to the start.
    pub(crate) fn advance_suggestion_page(&mut self, shown_through: usize) {
        let total = self
            .search
            .suggestion_cache
            .as_ref()
            .map(|(_, _, _, list)| list.len())
            .unwrap_or(0);
        self.search.suggestion_page_start = if shown_through >= total {
            0
        } else {
            shown_through
        };
    }

    /// Confirmed removal of an OR segment (the common confirm dialog's yes).
    pub(crate) fn remove_or_segment_confirmed(&mut self, field_index: usize, segment_index: usize) {
        let field = SearchField::ALL[field_index];
        self.search.set_field(field);
        let value = self.search.value(field).to_string();
        let chars: Vec<char> = value.chars().collect();
        let updated = OrSegments::parse(&chars).remove_segment(&value, segment_index);
        *self.search.value_mut() = updated;
        self.search.cursor = self.search.value(field).chars().count();
        self.search.notice = None;
        self.search.select_all = false;
    }

    /// [OR] button on a multi-value row: appends `` OR `` and keeps the
    /// field focused so the next value can be typed/picked from suggestions.
    pub(crate) fn append_or_separator(&mut self, field: SearchField) {
        if SearchState::multi_value_separator(field).is_none() {
            return;
        }
        self.search.set_field(field);
        let value = self.search.value(field).trim_end().to_string();
        if !value.is_empty() && !value.ends_with(" OR") {
            *self.search.value_mut() = format!("{value} OR ");
        }
        self.search.cursor = self.search.value(field).chars().count();
        self.search.select_all = false;
    }

    /// After a deletion in a multi-value field: if the active OR block just
    /// became empty (and others exist), ask — via the common confirm dialog —
    /// whether to drop the block entirely (joiner included).
    fn maybe_confirm_empty_or_block(&mut self) {
        let field = self.search.field;
        if SearchState::multi_value_separator(field).is_none() {
            return;
        }
        let chars: Vec<char> = self.search.value(field).chars().collect();
        let parsed = OrSegments::parse(&chars);
        if parsed.segments.len() < 2 {
            return;
        }
        let c = self.search.cursor;
        let Some(idx) = parsed
            .segments
            .iter()
            .position(|seg| c >= seg.text.start && c <= seg.text.end)
        else {
            return;
        };
        if parsed.segments[idx].text.is_empty() {
            self.nav.push(Mode::Confirm(
                crate::app::models::ConfirmAction::RemoveOrSegment {
                    field_index: field.index(),
                    segment_index: idx,
                    preview: "this empty OR block".into(),
                },
                true,
            ));
        }
    }

    /// [ clear ] on a multi-value field clears from the RIGHT, one OR
    /// segment per click ("a OR b OR c" → "a OR b" → "a" → ""). Plain
    /// fields (and the keyboard chords) clear whole.
    pub(crate) fn clear_field_staged(&mut self, field: SearchField) {
        self.search.set_field(field);
        if SearchState::multi_value_separator(field).is_some() {
            let value = self.search.value(field).to_string();
            let chars: Vec<char> = value.chars().collect();
            let parsed = OrSegments::parse(&chars);
            if parsed.segments.len() > 1 {
                let last = parsed.segments.len() - 1;
                *self.search.value_mut() = parsed.remove_segment(&value, last);
                self.search.cursor = self.search.value(field).chars().count();
                self.search.notice = None;
                self.search.select_all = false;
                return;
            }
        }
        self.clear_active_search_field();
    }

    /// The date row's [ clear ] works in stages: first click clears To (if
    /// set), second click clears From — trimming the range from the near end.
    pub(crate) fn clear_date_row_staged(&mut self) {
        let target = if !self.search.value(SearchField::DateTo).is_empty() {
            SearchField::DateTo
        } else {
            SearchField::DateFrom
        };
        self.search.set_field(target);
        self.clear_active_search_field();
    }

    /// Clear the active field (Ctrl+U, Shift+Backspace, or the ✕ click).
    pub(crate) fn clear_active_search_field(&mut self) {
        self.search.value_mut().clear();
        self.search.cursor = 0;
        self.search.reset_backup = None;
        self.search.notice = None;
    }

    pub(crate) fn handle_search(&mut self, key: KeyEvent) -> bool {
        // Any edit invalidates the "no matches" notice.
        if !matches!(key.code, KeyCode::Enter) {
            self.search.notice = None;
        }
        if let Some(button) = self.search.button_focus {
            match key.code {
                KeyCode::Esc => {
                    self.cancel_search();
                }
                KeyCode::Enter => self.activate_search_button(button),
                KeyCode::Tab | KeyCode::Down => self.search_focus_step(true),
                KeyCode::BackTab | KeyCode::Up => self.search_focus_step(false),
                KeyCode::Left => {
                    self.search.button_focus = Some(button.saturating_sub(1));
                }
                KeyCode::Right => {
                    self.search.button_focus =
                        Some((button + 1).min(self.search_button_count() - 1));
                }
                KeyCode::Char(' ') => self.activate_search_button(button),
                _ => {}
            }
            return false;
        }
        // Double-click selection: destructive keys clear, typing replaces,
        // navigation deselects. Esc/Enter keep their popup meaning.
        if self.search.select_all {
            match key.code {
                KeyCode::Backspace | KeyCode::Delete => {
                    self.search.select_all = false;
                    self.clear_active_search_field();
                    return false;
                }
                KeyCode::Char(ch)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.search.select_all = false;
                    *self.search.value_mut() = ch.to_string();
                    self.search.cursor = 1;
                    self.search.reset_backup = None;
                    return false;
                }
                KeyCode::Esc | KeyCode::Enter => {}
                _ => self.search.select_all = false,
            }
        }
        match key.code {
            KeyCode::Esc => {
                self.cancel_search();
            }
            KeyCode::Enter => self.execute_search(),
            KeyCode::Tab => {
                // Tab completes when the field has suggestions, otherwise it
                // moves to the next field (single candidate scan, not two).
                if self.autocomplete_search() {
                    self.search.reset_backup = None;
                } else {
                    self.search_focus_step(true);
                }
            }
            KeyCode::BackTab | KeyCode::Up => self.search_focus_step(false),
            KeyCode::Down => self.search_focus_step(true),
            KeyCode::Left => {
                self.search.cursor = input_edit::step_cursor(
                    self.search.value(self.search.field),
                    self.search.cursor,
                    false,
                );
            }
            KeyCode::Right => {
                self.search.cursor = input_edit::step_cursor(
                    self.search.value(self.search.field),
                    self.search.cursor,
                    true,
                );
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.clear_active_search_field();
            }
            // Alt+Backspace is the reliable clear chord: terminals encode it
            // distinctly (ESC DEL), unlike Ctrl/Shift+Backspace which many
            // terminals collapse into a plain Backspace byte.
            KeyCode::Backspace
                if key.modifiers.intersects(
                    KeyModifiers::ALT | KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                ) =>
            {
                self.clear_active_search_field();
            }
            KeyCode::Home => self.search.cursor = 0,
            KeyCode::End => {
                self.search.cursor = self.search.value(self.search.field).chars().count();
            }
            // Date fields are masked inputs: digits append (dashes appear on
            // their own), everything else is ignored, Backspace/Delete drop
            // the last digit. Cursor stays pinned at the end.
            KeyCode::Char(ch)
                if matches!(
                    self.search.field,
                    SearchField::DateFrom | SearchField::DateTo
                ) && !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                // Masked input: invalid digits and non-digits are swallowed.
                if let Some(next) = input_edit::date_input(self.search.value(self.search.field), ch)
                {
                    *self.search.value_mut() = next;
                    self.search.cursor = self.search.value(self.search.field).chars().count();
                    self.search.reset_backup = None;
                }
            }
            KeyCode::Backspace | KeyCode::Delete
                if matches!(
                    self.search.field,
                    SearchField::DateFrom | SearchField::DateTo
                ) =>
            {
                *self.search.value_mut() =
                    input_edit::date_backspace(self.search.value(self.search.field));
                self.search.cursor = self.search.value(self.search.field).chars().count();
                self.search.reset_backup = None;
            }
            KeyCode::Backspace => {
                if self.search.cursor > 0 {
                    // " OR " deletes atomically: one Backspace removes the
                    // whole separator instead of leaving " OR"/" O" litter.
                    let atomic_or = SearchState::multi_value_separator(self.search.field).is_some();
                    let (next, cursor) = input_edit::backspace(
                        self.search.value(self.search.field),
                        self.search.cursor,
                        atomic_or,
                    );
                    *self.search.value_mut() = next;
                    self.search.cursor = cursor;
                    self.search.reset_backup = None;
                    self.maybe_confirm_empty_or_block();
                }
            }
            KeyCode::Delete => {
                let value = self.search.value(self.search.field);
                if self.search.cursor < value.chars().count() {
                    let (next, cursor) = input_edit::delete(value, self.search.cursor);
                    *self.search.value_mut() = next;
                    self.search.cursor = cursor;
                    self.search.reset_backup = None;
                    self.maybe_confirm_empty_or_block();
                }
            }
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let (next, cursor) = input_edit::insert(
                    self.search.value(self.search.field),
                    self.search.cursor,
                    ch,
                );
                *self.search.value_mut() = next;
                self.search.cursor = cursor;
                self.search.reset_backup = None;
            }
            _ => {}
        }
        false
    }
}

#[cfg(test)]
mod ops_tests {
    use super::validate_date;

    #[test]
    fn empty_date_means_no_bound() {
        assert_eq!(validate_date("  "), Ok(None));
    }

    #[test]
    fn valid_date_is_a_bound() {
        assert_eq!(validate_date(" 2024-01-15 "), Ok(Some("2024-01-15".into())));
    }

    #[test]
    fn invalid_date_is_rejected_with_hint() {
        let err = validate_date("2024-13-99").unwrap_err();
        assert!(err.contains("YYYY-MM-DD"), "{err}");
    }
}
