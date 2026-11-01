/// Generic vim-like multi-line text editor state, shared by squash message
/// and commit message editors.
pub struct MultilineEditor {
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_col: usize,
    /// True = insert mode; false = view-only command mode.
    pub insert_mode: bool,
    /// Pending `d` press — waiting for second `d` to form `dd`.
    pub pending_d: bool,
}

impl MultilineEditor {
    /// Create from a plain text string; newlines become separate lines.
    pub fn new(text: &str) -> Self {
        let lines: Vec<String> = text
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
            .collect();
        Self::from_lines(lines)
    }

    /// Create from an already-split list of lines.
    pub fn from_lines(lines: Vec<String>) -> Self {
        let lines = if lines.is_empty() {
            vec![String::new()]
        } else {
            lines
        };
        let cursor_col = lines[0].chars().count();
        Self {
            lines,
            cursor_line: 0,
            cursor_col,
            insert_mode: false,
            pending_d: false,
        }
    }

    /// Return all lines joined with newlines, trimmed. Does **not** strip `#` comments.
    pub fn text(&self) -> String {
        self.lines.join("\n").trim().to_string()
    }

    pub fn line_len(&self) -> usize {
        self.lines
            .get(self.cursor_line)
            .map(|l| l.chars().count())
            .unwrap_or(0)
    }

    pub fn clamp_col(&mut self) {
        let len = self.line_len();
        let max = if self.insert_mode || len == 0 {
            len
        } else {
            len - 1
        };
        self.cursor_col = self.cursor_col.min(max);
    }

    /// Leave insert mode vim-style: cursor steps one column left and clamps
    /// onto the last character of the line.
    pub fn exit_insert_mode(&mut self) {
        self.insert_mode = false;
        self.pending_d = false;
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        }
        self.clamp_col();
    }

    pub fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor_col < self.line_len() {
            self.cursor_col += 1;
        }
    }

    /// Vim `o`: open an empty line below the cursor and enter insert mode.
    pub fn open_line_below(&mut self) {
        self.cursor_line += 1;
        self.lines.insert(self.cursor_line, String::new());
        self.cursor_col = 0;
        self.insert_mode = true;
    }

    /// Vim `O`: open an empty line above the cursor and enter insert mode.
    pub fn open_line_above(&mut self) {
        self.lines.insert(self.cursor_line, String::new());
        self.cursor_col = 0;
        self.insert_mode = true;
    }

    pub fn move_up(&mut self) {
        if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.clamp_col();
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.clamp_col();
        }
    }

    pub fn move_line_start(&mut self) {
        self.cursor_col = 0;
    }

    pub fn move_line_end(&mut self) {
        self.cursor_col = self.line_len();
        self.clamp_col();
    }

    pub fn move_doc_start(&mut self) {
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.clamp_col();
    }

    pub fn move_doc_end(&mut self) {
        self.cursor_line = self.lines.len().saturating_sub(1);
        self.cursor_col = self.line_len();
        self.clamp_col();
    }

    pub fn insert_char(&mut self, c: char) {
        if self.cursor_line >= self.lines.len() {
            return;
        }
        let line = &mut self.lines[self.cursor_line];
        let byte_pos = line
            .char_indices()
            .nth(self.cursor_col)
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        line.insert(byte_pos, c);
        self.cursor_col += 1;
    }

    pub fn insert_text(&mut self, text: &str) {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        for ch in normalized.chars() {
            match ch {
                '\n' => self.split_line(),
                c => self.insert_char(c),
            }
        }
    }

    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            let cursor = self.cursor_col;
            let line = &mut self.lines[self.cursor_line];
            let byte_pos = line
                .char_indices()
                .nth(cursor - 1)
                .map(|(i, _)| i)
                .unwrap_or(0);
            line.remove(byte_pos);
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            let rest = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            let prev_len = self.lines[self.cursor_line].chars().count();
            self.lines[self.cursor_line].push_str(&rest);
            self.cursor_col = prev_len;
        }
    }

    pub fn delete_at(&mut self) {
        let len = self.line_len();
        if self.cursor_col < len {
            let cursor = self.cursor_col;
            let line = &mut self.lines[self.cursor_line];
            let byte_pos = line
                .char_indices()
                .nth(cursor)
                .map(|(i, _)| i)
                .unwrap_or(line.len());
            line.remove(byte_pos);
        } else if self.cursor_line + 1 < self.lines.len() {
            let next = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next);
        }
    }

    pub fn split_line(&mut self) {
        if self.cursor_line >= self.lines.len() {
            self.lines.push(String::new());
            self.cursor_line += 1;
            self.cursor_col = 0;
            return;
        }
        let cursor = self.cursor_col;
        let line = &self.lines[self.cursor_line];
        let byte_pos = line
            .char_indices()
            .nth(cursor)
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        let remainder = line[byte_pos..].to_string();
        self.lines[self.cursor_line].truncate(byte_pos);
        self.cursor_line += 1;
        self.lines.insert(self.cursor_line, remainder);
        self.cursor_col = 0;
    }

    pub fn delete_line(&mut self) {
        if self.lines.len() > 1 {
            self.lines.remove(self.cursor_line);
            if self.cursor_line >= self.lines.len() {
                self.cursor_line = self.lines.len() - 1;
            }
        } else {
            self.lines[0].clear();
            self.cursor_col = 0;
            return;
        }
        self.clamp_col();
    }
}
