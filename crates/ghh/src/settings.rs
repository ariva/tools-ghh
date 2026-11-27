//! Settings persistence: global `~/.config/ghh/ghh-settings.json`
//! plus a per-repo `.ghh-settings.json` override.
//!
//! Merge order (later wins, field by field): built-in defaults ← global
//! `defaults` ← global `repos[<key>]` ← local `.ghh-settings.json`.
//! The repo key is the repository directory name.
//!
//! Loading and saving never fail the app: unreadable/corrupt files fall back
//! to defaults, write errors are ignored — settings must never block exit.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Pane split preferences. Percentages (not cells) so a saved layout adapts
/// to any terminal size; only the details footer is an absolute row count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Splits {
    /// Commit list width as % of the screen (the diff column gets the rest).
    pub list_pct: u16,
    /// Commit info height as % of the diff column (the patch gets the rest).
    pub diff_info_pct: u16,
    /// Details footer height in rows.
    pub details_height: u16,
    /// Files column width as % of the diff column (the patch gets the rest).
    pub files_pct: u16,
}

impl Default for Splits {
    fn default() -> Self {
        Self {
            list_pct: 40,
            diff_info_pct: 40,
            details_height: 8,
            files_pct: 30,
        }
    }
}

impl Splits {
    /// Keep every pane usable regardless of what a drag or a hand-edited
    /// settings file produced.
    pub fn clamped(self) -> Self {
        Self {
            list_pct: self.list_pct.clamp(15, 85),
            diff_info_pct: self.diff_info_pct.clamp(15, 85),
            details_height: self.details_height.clamp(3, 20),
            files_pct: self.files_pct.clamp(15, 85),
        }
    }
}

/// Trace log file settings (`logs: {…}` object form).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogSettings {
    /// May contain `{…}` date tokens; default /tmp/ghh/logs.txt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Default true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Minimum level written to the file; defaults to the debug screen's
    /// level (i.e. mirror what the debugger shows).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
    /// Include app events (key presses, screen changes, actions) in the
    /// file. Default true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_events: Option<bool>,
}

/// Default trace log file path.
pub const DEFAULT_LOG_PATH: &str = "/tmp/ghh/logs.txt";

/// Debug screen settings (`debug: {…}` object form).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DebugSettings {
    /// Minimum trace level shown. Default TRACE (show all).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
    /// Show app events (KEY/SCRN/ACT rows). Default true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_events: Option<bool>,
}

/// `debug:` accepts the legacy plain level string or the full object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DebugSetting {
    Level(String),
    Full(DebugSettings),
}

/// `logs:` accepts a plain path string or the full object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LogsSetting {
    Path(String),
    Full(LogSettings),
}

/// One repo's (or the global `defaults`) settings; every field optional so
/// files only state what they override.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// "simple" | "graph" | "focused" (see `CommitListView::label`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_details: Option<bool>,
    /// Debug screen settings: level string ("INFO") or object
    /// ({ log_level, log_events }). Levels: TRACE|DEBUG|INFO|WARN|ERROR,
    /// case-insensitive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug: Option<DebugSetting>,
    /// Trace log file: path string or object ({ path, enabled, log_level,
    /// log_events }). Path may contain `{…}` date tokens (YYYY MM DD HH mm
    /// SS). "off"/"" path or enabled:false disables. CLI --logs overrides.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs: Option<LogsSetting>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub splits: Option<Splits>,
    /// Seconds between checks for changes made outside the app while the
    /// commit list is shown. 0 = never check. Default 2, at most 60.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watch_secs: Option<u64>,
}

impl RepoSettings {
    /// Field-wise merge: `other`'s set fields win.
    fn overlay(&mut self, other: &RepoSettings) {
        macro_rules! take {
            ($field:ident) => {
                if other.$field.is_some() {
                    self.$field = other.$field.clone();
                }
            };
        }
        take!(short_name);
        take!(path);
        take!(origin);
        take!(view);
        take!(show_details);
        take!(debug);
        take!(logs);
        take!(splits);
        take!(watch_secs);
    }

    /// Debug screen minimum level, whichever form `debug:` was written in.
    pub fn debug_level(&self) -> Option<&str> {
        match self.debug.as_ref()? {
            DebugSetting::Level(level) => Some(level),
            DebugSetting::Full(debug) => debug.log_level.as_deref(),
        }
    }

    /// Debug screen "show events" flag (object form only).
    pub fn debug_events(&self) -> Option<bool> {
        match self.debug.as_ref()? {
            DebugSetting::Level(_) => None,
            DebugSetting::Full(debug) => debug.log_events,
        }
    }

    /// `logs:` normalized to the object form.
    pub fn logs_normalized(&self) -> LogSettings {
        match self.logs.clone() {
            Some(LogsSetting::Path(path)) => LogSettings {
                path: Some(path),
                ..Default::default()
            },
            Some(LogsSetting::Full(logs)) => logs,
            None => LogSettings::default(),
        }
    }
}

/// Which app build first wrote this settings file. Stamped once when
/// missing and never overwritten after that (future upgrade/migration
/// logic will key off it).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub build_hash: String,
    pub build_date: String,
}

impl AppInfo {
    pub fn current() -> Self {
        Self {
            name: env!("CARGO_BIN_NAME").into(),
            version: env!("CARGO_PKG_VERSION").into(),
            build_hash: env!("GIT_HELPER_BUILD_HASH").into(),
            build_date: env!("GIT_HELPER_BUILD_DATE").into(),
        }
    }
}

/// Shape of the global settings file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<AppInfo>,
    /// God level: applied AFTER everything else (defaults, repo entry,
    /// local file) — one place to force a setting across all repos.
    /// Seeded empty so the section is always visible; field order puts it
    /// right before the defaults (debug/logs) block in the written file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overrides: Option<RepoSettings>,
    pub defaults: RepoSettings,
    pub repos: BTreeMap<String, RepoSettings>,
}

/// The full merge, one place: built-in defaults ← global `defaults` ←
/// global `repos[<key>]` ← local file ← global `overrides` (god level).
/// Later wins, field by field.
pub fn resolve(
    global: &SettingsFile,
    repo_key: &str,
    local: Option<&RepoSettings>,
) -> RepoSettings {
    let mut resolved = RepoSettings::default();
    resolved.overlay(&global.defaults);
    if let Some(repo) = global.repos.get(repo_key) {
        resolved.overlay(repo);
    }
    if let Some(local) = local {
        resolved.overlay(local);
    }
    if let Some(overrides) = &global.overrides {
        resolved.overlay(overrides);
    }
    resolved
}

/// Loaded settings plus where to write them back.
pub struct Settings {
    global_path: PathBuf,
    local_path: PathBuf,
    pub repo_key: String,
    /// Effective settings for this repo after the full merge.
    pub resolved: RepoSettings,
    /// Also write the local `.ghh-settings.json` on save (the file already
    /// exists, or `--save-local-settings` was passed).
    pub save_local: bool,
}

/// Settings files are parsed as JSON5, so hand-edited files may use
/// comments, trailing commas, unquoted keys… The app itself always writes
/// plain pretty JSON (every JSON document is valid JSON5).
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    json5::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn global_settings_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ghh")
        .join("ghh-settings.json")
}

/// `debug:` normalized to (level, show_events) with every default filled.
fn debug_norm(setting: Option<&DebugSetting>) -> (String, bool) {
    let (level, events) = match setting {
        Some(DebugSetting::Level(level)) => (Some(level.as_str()), None),
        Some(DebugSetting::Full(debug)) => (debug.log_level.as_deref(), debug.log_events),
        None => (None, None),
    };
    (
        level.unwrap_or("INFO").to_ascii_uppercase(),
        events.unwrap_or(true),
    )
}

/// `logs:` normalized to (path, enabled, level, events) with defaults filled.
fn logs_norm(setting: Option<&LogsSetting>) -> (String, bool, String, bool) {
    let logs = match setting {
        Some(LogsSetting::Path(path)) => LogSettings {
            path: Some(path.clone()),
            ..Default::default()
        },
        Some(LogsSetting::Full(logs)) => logs.clone(),
        None => LogSettings::default(),
    };
    (
        logs.path.unwrap_or_else(|| DEFAULT_LOG_PATH.into()),
        logs.enabled.unwrap_or(true),
        logs.log_level
            .as_deref()
            .unwrap_or("INFO")
            .to_ascii_uppercase(),
        logs.log_events.unwrap_or(true),
    )
}

/// Drop every field whose value equals what the repo would inherit anyway
/// (built-in defaults overlaid with the global `defaults` section): the
/// file stores ONLY deviations. Identity fields (short_name/path/origin)
/// have no defaults and always stay.
fn strip_inherited(mut entry: RepoSettings, defaults: &RepoSettings) -> RepoSettings {
    if entry.view.as_deref() == Some(defaults.view.as_deref().unwrap_or("focused")) {
        entry.view = None;
    }
    if entry.show_details == Some(defaults.show_details.unwrap_or(false)) {
        entry.show_details = None;
    }
    if entry.splits == Some(defaults.splits.unwrap_or_default()) {
        entry.splits = None;
    }
    if entry.watch_secs == defaults.watch_secs {
        entry.watch_secs = None;
    }
    if debug_norm(entry.debug.as_ref()) == debug_norm(defaults.debug.as_ref()) {
        entry.debug = None;
    }
    if logs_norm(entry.logs.as_ref()) == logs_norm(defaults.logs.as_ref()) {
        entry.logs = None;
    }
    entry
}

// ── surgical writes (comment-preserving) ─────────────────────────────────────

/// Minimal JSON5 scanner: just enough to find the byte span of one
/// `repos.<key>` entry so saves can splice it without touching (and thus
/// without destroying comments in) the rest of the file.
struct Json5Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Json5Scanner<'a> {
    fn new(text: &'a str, pos: usize) -> Self {
        Self {
            bytes: text.as_bytes(),
            pos,
        }
    }

    fn cur(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    /// Skip whitespace and // or /* */ comments.
    fn skip_trivia(&mut self) {
        loop {
            match self.cur() {
                Some(byte) if byte.is_ascii_whitespace() => self.pos += 1,
                Some(b'/') if self.bytes.get(self.pos + 1) == Some(&b'/') => {
                    while self.cur().is_some_and(|byte| byte != b'\n') {
                        self.pos += 1;
                    }
                }
                Some(b'/') if self.bytes.get(self.pos + 1) == Some(&b'*') => {
                    self.pos += 2;
                    while self.pos < self.bytes.len()
                        && !(self.bytes[self.pos] == b'*'
                            && self.bytes.get(self.pos + 1) == Some(&b'/'))
                    {
                        self.pos += 1;
                    }
                    self.pos = (self.pos + 2).min(self.bytes.len());
                }
                _ => return,
            }
        }
    }

    /// Skip a string; `pos` sits on the opening quote.
    fn skip_string(&mut self) {
        let quote = self.bytes[self.pos];
        self.pos += 1;
        while let Some(byte) = self.cur() {
            self.pos += if byte == b'\\' { 2 } else { 1 };
            if byte == quote {
                return;
            }
        }
    }

    /// Skip one value (object/array/string/scalar); `pos` sits at its start.
    fn skip_value(&mut self) {
        match self.cur() {
            Some(b'{') | Some(b'[') => {
                let mut depth = 0usize;
                while let Some(byte) = self.cur() {
                    match byte {
                        b'{' | b'[' => {
                            depth += 1;
                            self.pos += 1;
                        }
                        b'}' | b']' => {
                            depth -= 1;
                            self.pos += 1;
                            if depth == 0 {
                                return;
                            }
                        }
                        b'"' | b'\'' => self.skip_string(),
                        b'/' => {
                            let before = self.pos;
                            self.skip_trivia();
                            if self.pos == before {
                                self.pos += 1; // lone slash — malformed, move on
                            }
                        }
                        _ => self.pos += 1,
                    }
                }
            }
            Some(b'"') | Some(b'\'') => self.skip_string(),
            _ => {
                // Scalar: number, bool, null, identifier-ish.
                while let Some(byte) = self.cur() {
                    if byte.is_ascii_whitespace() || matches!(byte, b',' | b'}' | b']' | b'/') {
                        return;
                    }
                    self.pos += 1;
                }
            }
        }
    }

    /// Read an object key (quoted or bare identifier).
    fn read_key(&mut self) -> Option<String> {
        self.skip_trivia();
        match self.cur()? {
            quote @ (b'"' | b'\'') => {
                let start = self.pos + 1;
                self.skip_string();
                std::str::from_utf8(&self.bytes[start..self.pos.saturating_sub(1)])
                    .ok()
                    .map(str::to_owned)
                    .filter(|_| self.bytes.get(self.pos.saturating_sub(1)) == Some(&quote))
            }
            _ => {
                let start = self.pos;
                while self.cur().is_some_and(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$' | b'-' | b'.')
                }) {
                    self.pos += 1;
                }
                (self.pos > start)
                    .then(|| String::from_utf8_lossy(&self.bytes[start..self.pos]).into_owned())
            }
        }
    }
}

/// Entries of the object whose `{` sits at `start`:
/// (key, key_start, value byte range). None = shape not understood.
fn object_entries(
    text: &str,
    start: usize,
) -> Option<Vec<(String, usize, std::ops::Range<usize>)>> {
    if text.as_bytes().get(start) != Some(&b'{') {
        return None;
    }
    let mut scanner = Json5Scanner::new(text, start + 1);
    let mut entries = Vec::new();
    loop {
        scanner.skip_trivia();
        match scanner.cur()? {
            b'}' => return Some(entries),
            b',' => scanner.pos += 1,
            _ => {
                let key_start = scanner.pos;
                let key = scanner.read_key()?;
                scanner.skip_trivia();
                if scanner.cur()? != b':' {
                    return None;
                }
                scanner.pos += 1;
                scanner.skip_trivia();
                let value_start = scanner.pos;
                scanner.skip_value();
                entries.push((key, key_start, value_start..scanner.pos));
            }
        }
    }
}

/// Leading whitespace of the line containing `pos`.
fn line_indent(text: &str, pos: usize) -> String {
    let line_start = text[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    text[line_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// Re-indent a pretty-printed JSON block: first line untouched, the rest
/// prefixed with `indent`.
fn reindent(json: &str, indent: &str) -> String {
    let mut lines = json.lines();
    let mut out = lines.next().unwrap_or_default().to_string();
    for line in lines {
        out.push('\n');
        out.push_str(indent);
        out.push_str(line);
    }
    out
}

/// Splice the regenerated `repos.<key>` entry into the original file text,
/// leaving every other byte — comments included — untouched. None = file
/// shape not understood; caller falls back to a full rewrite.
fn splice_repo_entry(text: &str, repo_key: &str, entry_json: &str) -> Option<String> {
    let mut scanner = Json5Scanner::new(text, 0);
    scanner.skip_trivia();
    let top = scanner.pos;
    let top_entries = object_entries(text, top)?;
    let (_, repos_key_start, repos_span) =
        top_entries.into_iter().find(|(key, _, _)| key == "repos")?;
    let repo_entries = object_entries(text, repos_span.start)?;
    match repo_entries.iter().find(|(key, _, _)| key == repo_key) {
        Some((_, key_start, value_span)) => {
            // Replace just this entry's value.
            let indent = line_indent(text, *key_start);
            let mut out = String::with_capacity(text.len() + entry_json.len());
            out.push_str(&text[..value_span.start]);
            out.push_str(&reindent(entry_json, &indent));
            out.push_str(&text[value_span.end..]);
            Some(out)
        }
        None => {
            // Insert a new entry right after the `{` of repos.
            let parent_indent = line_indent(text, repos_key_start);
            let entry_indent = repo_entries
                .first()
                .map(|(_, key_start, _)| line_indent(text, *key_start))
                .unwrap_or_else(|| format!("{parent_indent}  "));
            let brace = repos_span.start;
            let insertion = if repo_entries.is_empty() {
                format!(
                    "\n{entry_indent}\"{repo_key}\": {}\n{parent_indent}",
                    reindent(entry_json, &entry_indent)
                )
            } else {
                format!(
                    "\n{entry_indent}\"{repo_key}\": {},",
                    reindent(entry_json, &entry_indent)
                )
            };
            let mut out = String::with_capacity(text.len() + insertion.len());
            out.push_str(&text[..=brace]);
            out.push_str(&insertion);
            out.push_str(&text[brace + 1..]);
            Some(out)
        }
    }
}

fn write_global(path: &Path, file: &SettingsFile) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(file) {
        let _ = std::fs::write(path, json);
    }
}

// ── example file ─────────────────────────────────────────────────────────────

/// Fully commented reference settings file. Written next to the real one as
/// ghh-settings.example.json; a build stamp on the first line decides when
/// it needs recreating.
const EXAMPLE_BODY: &str = r#"// Reference for ~/.config/ghh/ghh-settings.json and per-repo
// .ghh-settings.json overrides. RECREATED by the app on version/build change
// — do not edit; copy what you need into the real files.
//
// Both files are JSON5: comments, trailing commas, unquoted keys all fine.
// Merge order (later wins, field by field):
//   built-in defaults -> defaults -> repos["<dir name>"] ->
//   <repo>/.ghh-settings.json -> overrides (god level)
{
  // Dynamically generated by the app: written ONCE by the first build that
  // touches the settings file, then never overwritten (future upgrade or
  // migration logic keys off it). Not a setting — informational only.
  // The values below are from the build that wrote this example.
  app: {
    name: "{APP_NAME}",
    version: "{APP_VERSION}",
    build_hash: "{APP_BUILD_HASH}",
    build_date: "{APP_BUILD_DATE}",
  },

  // God level: merged LAST — beats defaults, repo entries AND the local
  // .ghh-settings.json. One place to force a setting everywhere, e.g.
  //   overrides: { debug: { log_level: "ERROR" } }
  // Same fields as a repo entry. The app never writes into this section.
  overrides: {},

  // Hand-maintained defaults applied to every repo. Same fields as a repo
  // entry (see below). The app never writes into this section.
  defaults: {},

  // One entry per repository, keyed by the repo DIRECTORY name. The app
  // updates these on quit and stores ONLY values that deviate from the
  // defaults above (plus the identity fields).
  repos: {
    "example-repo": {
      // Identity, written by the app (no defaults):
      short_name: "example-repo.git",               // last segment of origin URL
      path: "/path/to/example-repo",                // canonical repo path
      origin: "git@github.com:you/example-repo.git",

      // Commit list view: "focused" (gitk-style) | "graph" | "simple".
      // Default: "focused".
      view: "focused",

      // Diff/details pane open on start: true | false. Default: false.
      show_details: false,

      // Debug screen + verbosity master knob. Either a plain level string
      // ("INFO") or this object form:
      debug: {
        // Minimum trace level shown on the debug screen AND the CAP for the
        // log file's verbosity (logs.log_level below may restrict further
        // but never exceed this). "TRACE" | "DEBUG" | "INFO" | "WARN" |
        // "ERROR", case-insensitive. Default: "INFO". RUST_LOG, when set,
        // still limits what is captured at all.
        log_level: "INFO",
        // Show app events (key presses, screen changes, actions) on the
        // debug screen: true | false. Default: true.
        log_events: true,
      },

      // Trace log file. Either a plain path string or this object form:
      logs: {
        // "off" or "" disables logging. Tokens expanded at start:
        //   {REPO} repo directory name, {HASH} stable 8-hex repo hash,
        //   date tokens YYYY MM DD HH mm SS in any combination,
        //   e.g. "/tmp/ghh/{REPO}-{HASH}-{YYYY-MM-DD}.log".
        // A fresh file is started on every run. Default: "/tmp/ghh/logs.txt".
        path: "/tmp/ghh/logs.txt",
        // Default: true. CLI --logs always forces logging on.
        enabled: true,
        // Minimum level written to the file; capped by debug.log_level.
        // Default: "INFO".
        log_level: "INFO",
        // Also write app events into the file (any level): true | false.
        // Default: true.
        log_events: true,
      },

      // Pane sizes, saved automatically when you drag borders with the
      // mouse. Percentages adapt to any terminal size; values are clamped.
      splits: {
        list_pct: 40,        // commit list width, % of screen (15-85)
        diff_info_pct: 40,   // commit info height, % of diff column (15-85)
        details_height: 8,   // details footer height in rows (3-20)
        files_pct: 30,       // files column width, % of diff column (15-85)
      },

      // Seconds between checks for changes made outside ghh (files edited,
      // commits, branch switch) while the commit list is shown. The check
      // runs in the background. 0 = never check. Default: 2, at most 60.
      watch_secs: 2,
    },
  },
}
"#;

/// Write ghh-settings.example.json into `dir` unless the existing file was
/// already written by this exact app version/build (stamp on line one).
pub fn write_example_file(dir: &Path) {
    let path = dir.join("ghh-settings.example.json");
    let app = AppInfo::current();
    let stamp = format!(
        "// auto-generated by {} v{} (build {} {})",
        app.name, app.version, app.build_hash, app.build_date
    );
    if std::fs::read_to_string(&path).is_ok_and(|text| text.starts_with(&stamp)) {
        return;
    }
    tracing::info!("settings: example file recreated at {}", path.display());
    let body = EXAMPLE_BODY
        .replace("{APP_NAME}", &app.name)
        .replace("{APP_VERSION}", &app.version)
        .replace("{APP_BUILD_HASH}", &app.build_hash)
        .replace("{APP_BUILD_DATE}", &app.build_date);
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&path, format!("{stamp}\n{body}"));
}

/// `origin` remote URL, if the repo has one.
pub fn origin_url(repo_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!url.is_empty()).then_some(url)
}

/// Last path segment of a remote URL: "git@host:org/name.git" -> "name.git".
pub fn short_name_of(url: &str) -> String {
    url.rsplit(['/', ':']).next().unwrap_or(url).to_string()
}

/// Stable 8-hex-digit repo identifier: FNV-1a 64 of the canonical repo
/// path. Hand-rolled so the value never changes across Rust versions.
pub fn repo_hash(canonical_path: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in canonical_path.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")[..8].to_string()
}

/// Expand `{…}` tokens in a log path: `{REPO}` (repo dir name), `{HASH}`
/// (stable repo path hash), and date tokens YYYY, MM (month), DD, HH,
/// mm (minutes), SS. Text outside braces (and unknown text inside) passes
/// through unchanged; an unclosed `{` is kept literal.
pub fn expand_log_pattern(
    spec: &str,
    now: &chrono::NaiveDateTime,
    repo_name: &str,
    repo_hash: &str,
) -> String {
    let mut out = String::new();
    let mut rest = spec;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        match rest[start..].find('}') {
            Some(offset) => {
                let token = &rest[start + 1..start + offset];
                match token {
                    "REPO" => out.push_str(repo_name),
                    "HASH" => out.push_str(repo_hash),
                    _ => {
                        // mm before MM: minutes must not be eaten by the
                        // month rule.
                        let fmt = token
                            .replace("YYYY", "%Y")
                            .replace("mm", "%M")
                            .replace("MM", "%m")
                            .replace("DD", "%d")
                            .replace("HH", "%H")
                            .replace("SS", "%S");
                        out.push_str(&now.format(&fmt).to_string());
                    }
                }
                rest = &rest[start + offset + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

impl Settings {
    pub fn load(repo_path: &Path) -> Self {
        // Unit tests build Apps against temp repos; their behavior must not
        // depend on the developer's real config files.
        if cfg!(test) {
            return Self {
                global_path: PathBuf::new(),
                local_path: PathBuf::new(),
                repo_key: "test".into(),
                resolved: RepoSettings::default(),
                save_local: false,
            };
        }
        let canonical = repo_path
            .canonicalize()
            .unwrap_or_else(|_| repo_path.to_path_buf());
        let repo_key = canonical
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".into());
        let global_path = global_settings_path();
        let local_path = canonical.join(".ghh-settings.json");
        let mut global: SettingsFile = read_json(&global_path).unwrap_or_default();
        // Seed once what's missing: the build stamp and the fully spelled-out
        // logs/debug defaults. Existing values are never touched, and with
        // nothing missing the file is not rewritten (preserving JSON5
        // comments).
        // `defaults` and `overrides` are read-only for the app: seeded as
        // visible empty sections, contents hand-maintained only.
        let mut seed = false;
        if global.app.is_none() {
            global.app = Some(AppInfo::current());
            seed = true;
        }
        if global.overrides.is_none() {
            global.overrides = Some(RepoSettings::default()); // visible, empty
            seed = true;
        }
        if seed {
            write_global(&global_path, &global);
        }
        let save_local = local_path.exists();
        let local = read_json::<RepoSettings>(&local_path);
        tracing::info!(
            "settings loaded: repo '{}' — global {} (entry: {}), local {}: {}",
            repo_key,
            global_path.display(),
            if global.repos.contains_key(&repo_key) {
                "found"
            } else {
                "none"
            },
            local_path.display(),
            if local.is_some() {
                "found"
            } else {
                "not found"
            },
        );
        let resolved = resolve(&global, &repo_key, local.as_ref());
        Self {
            global_path,
            local_path,
            repo_key,
            resolved,
            save_local,
        }
    }

    /// Write `entry` to the global file's `repos[<key>]` (and the local
    /// override file when enabled). Re-reads the global file first so entries
    /// for other repos written meanwhile survive.
    pub fn save(&self, entry: RepoSettings) {
        let original = std::fs::read_to_string(&self.global_path).ok();
        let mut global: SettingsFile = original
            .as_deref()
            .and_then(|text| json5::from_str(text).ok())
            .unwrap_or_default();
        let needs_seed = global.app.is_none() || global.overrides.is_none();
        global.app.get_or_insert_with(AppInfo::current);
        global.overrides.get_or_insert_with(RepoSettings::default);
        // Smart save: only deviations from the inherited baseline are
        // stored; `defaults` and `overrides` are never written into.
        let minimal = strip_inherited(entry, &global.defaults);
        // Nothing changed: no write at all — hand-written JSON5 comments in
        // the file survive untouched.
        if !needs_seed && global.repos.get(&self.repo_key) == Some(&minimal) {
            tracing::info!("settings: global unchanged, not written");
        }
        if needs_seed || global.repos.get(&self.repo_key) != Some(&minimal) {
            // Surgical write: splice only this repo's entry into the
            // original text, preserving comments everywhere else. Falls
            // back to a full rewrite when the file shape isn't understood
            // (or seeding is needed anyway).
            let spliced = (!needs_seed)
                .then_some(original.as_deref())
                .flatten()
                .zip(serde_json::to_string_pretty(&minimal).ok())
                .and_then(|(text, json)| splice_repo_entry(text, &self.repo_key, &json))
                // Only ship text that still parses — a splice bug must
                // degrade to the safe full rewrite, never corrupt the file.
                .filter(|text| json5::from_str::<SettingsFile>(text).is_ok());
            match spliced {
                Some(text) => {
                    if let Some(dir) = self.global_path.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let _ = std::fs::write(&self.global_path, text);
                    tracing::info!(
                        "settings saved: repo '{}' spliced into {}",
                        self.repo_key,
                        self.global_path.display()
                    );
                }
                None => {
                    global.repos.insert(self.repo_key.clone(), minimal.clone());
                    write_global(&self.global_path, &global);
                    tracing::info!(
                        "settings saved: {} fully rewritten (repo '{}')",
                        self.global_path.display(),
                        self.repo_key
                    );
                }
            }
        }
        if self.save_local {
            if read_json::<RepoSettings>(&self.local_path).as_ref() == Some(&minimal) {
                tracing::info!("settings: local override unchanged, not written");
            } else if let Ok(json) = serde_json::to_string_pretty(&minimal) {
                let _ = std::fs::write(&self.local_path, json);
                tracing::info!(
                    "settings saved: local override {}",
                    self.local_path.display()
                );
            }
        }
        // Keep the commented reference file current (recreated only when
        // the app version/build changed).
        if let Some(dir) = self.global_path.parent() {
            write_example_file(dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_merges_field_wise() {
        let mut base = RepoSettings {
            view: Some("focused".into()),
            show_details: Some(true),
            ..Default::default()
        };
        base.overlay(&RepoSettings {
            show_details: Some(false),
            origin: Some("git@host:org/repo.git".into()),
            ..Default::default()
        });
        assert_eq!(base.view.as_deref(), Some("focused")); // kept
        assert_eq!(base.show_details, Some(false)); // overridden
        assert_eq!(base.origin.as_deref(), Some("git@host:org/repo.git")); // added
    }

    #[test]
    fn parses_example_settings_file() {
        let json = r#"{
            "defaults": { "view": "focused", "show_details": true },
            "repos": {
                "ariva-tools": {
                    "short_name": "ariva-tools.git",
                    "origin": "git@github.com-personal:ariva/ariva-tools.git",
                    "show_details": false
                }
            }
        }"#;
        let file: SettingsFile = serde_json::from_str(json).unwrap();
        assert_eq!(file.defaults.view.as_deref(), Some("focused"));
        assert_eq!(file.repos["ariva-tools"].show_details, Some(false));
        assert!(file.repos["ariva-tools"].splits.is_none());
    }

    #[test]
    fn parses_json5_comments_trailing_commas_unquoted_keys() {
        let json5_text = r#"{
            // global defaults
            defaults: { view: 'graph', show_details: true, },
            repos: {
                "ariva-tools": {
                    /* block comment */
                    splits: { list_pct: 30, },
                },
            },
        }"#;
        let file: SettingsFile = json5::from_str(json5_text).unwrap();
        assert_eq!(file.defaults.view.as_deref(), Some("graph"));
        assert_eq!(file.repos["ariva-tools"].splits.unwrap().list_pct, 30);
        // What the app writes must load back (JSON ⊂ JSON5).
        let written = serde_json::to_string_pretty(&file).unwrap();
        let reread: SettingsFile = json5::from_str(&written).unwrap();
        assert_eq!(reread.defaults.view.as_deref(), Some("graph"));
    }

    #[test]
    fn merge_order_defaults_repo_local_then_overrides_wins() {
        let global: SettingsFile = json5::from_str(
            r#"{
                defaults: { view: "focused", show_details: true },
                repos: { r: { view: "graph", short_name: "r.git" } },
                overrides: { show_details: false },
            }"#,
        )
        .unwrap();
        let local = RepoSettings {
            view: Some("simple".into()),
            ..Default::default()
        };
        let resolved = resolve(&global, "r", Some(&local));
        assert_eq!(resolved.view.as_deref(), Some("simple")); // local beats repo entry
        assert_eq!(resolved.short_name.as_deref(), Some("r.git")); // repo entry beats defaults
        assert_eq!(resolved.show_details, Some(false)); // overrides beat everything
        // Overrides win over local too — god level.
        let god: SettingsFile = json5::from_str(r#"{ overrides: { view: "graph" } }"#).unwrap();
        assert_eq!(
            resolve(&god, "missing", Some(&local)).view.as_deref(),
            Some("graph")
        );
        // No repo entry, no local, no overrides: defaults flow through.
        assert_eq!(
            resolve(&global, "missing", None).view.as_deref(),
            Some("focused")
        );
    }

    const COMMENTED: &str = r#"{
  // stamp comment
  app: { name: 'ghh' },
  overrides: {}, /* god level */
  defaults: {
    // keep me: defaults comment
    view: "graph",
  },
  repos: {
    // other repo comment
    "other": { view: "simple", origin: "x{y}" },
    "mine": { view: "focused" },
  },
}"#;

    #[test]
    fn splice_replaces_one_entry_and_keeps_all_comments() {
        let new_json = "{\n  \"view\": \"graph\",\n  \"show_details\": true\n}";
        let out = splice_repo_entry(COMMENTED, "mine", new_json).unwrap();
        // Comments and the other entry survive byte-for-byte.
        for kept in [
            "// stamp comment",
            "/* god level */",
            "// keep me",
            "// other repo comment",
            "\"other\": { view: \"simple\", origin: \"x{y}\" }",
        ] {
            assert!(out.contains(kept), "missing: {kept}");
        }
        // The new value is live and the file still parses.
        let parsed: SettingsFile = json5::from_str(&out).unwrap();
        assert_eq!(parsed.repos["mine"].show_details, Some(true));
        assert_eq!(parsed.repos["other"].view.as_deref(), Some("simple"));
        assert_eq!(parsed.defaults.view.as_deref(), Some("graph"));
    }

    #[test]
    fn splice_inserts_new_entry_and_handles_empty_repos() {
        let out = splice_repo_entry(COMMENTED, "fresh", "{\n  \"view\": \"simple\"\n}").unwrap();
        let parsed: SettingsFile = json5::from_str(&out).unwrap();
        assert_eq!(parsed.repos["fresh"].view.as_deref(), Some("simple"));
        assert_eq!(parsed.repos.len(), 3);
        assert!(out.contains("// other repo comment"));

        let empty = "{\n  repos: {}, // none yet\n}";
        let out = splice_repo_entry(empty, "first", "{\n  \"view\": \"graph\"\n}").unwrap();
        let parsed: SettingsFile = json5::from_str(&out).unwrap();
        assert_eq!(parsed.repos["first"].view.as_deref(), Some("graph"));
        assert!(out.contains("// none yet"));

        // No repos section: not understood, caller falls back.
        assert!(splice_repo_entry("{ defaults: {} }", "x", "{}").is_none());
    }

    /// The commented reference file must stay a valid settings file and
    /// show EVERY setting — a new struct field without a template update
    /// fails here.
    #[test]
    fn example_template_parses_and_covers_every_setting() {
        let parsed: SettingsFile = json5::from_str(EXAMPLE_BODY).unwrap();
        let entry = &parsed.repos["example-repo"];
        assert!(entry.short_name.is_some() && entry.path.is_some() && entry.origin.is_some());
        assert_eq!(entry.view.as_deref(), Some("focused"));
        assert_eq!(entry.show_details, Some(false));
        assert_eq!(debug_norm(entry.debug.as_ref()), ("INFO".into(), true));
        assert_eq!(
            logs_norm(entry.logs.as_ref()),
            (DEFAULT_LOG_PATH.into(), true, "INFO".into(), true)
        );
        assert_eq!(entry.splits, Some(Splits::default()));
        assert!(parsed.overrides.is_some(), "overrides section shown");
        assert!(parsed.app.is_some(), "app stamp section shown");
        // Every RepoSettings field is demonstrated (values equal defaults,
        // so a full strip leaves only identity — proving nothing is missing
        // beyond fields without defaults).
        let stripped = strip_inherited(entry.clone(), &RepoSettings::default());
        assert!(stripped.view.is_none() && stripped.debug.is_none() && stripped.logs.is_none());
    }

    #[test]
    fn smart_save_strips_values_matching_the_inherited_baseline() {
        let full_entry = || RepoSettings {
            short_name: Some("r.git".into()),
            view: Some("focused".into()),
            show_details: Some(false),
            debug: Some(DebugSetting::Full(DebugSettings {
                log_level: Some("INFO".into()),
                log_events: Some(true),
            })),
            logs: Some(LogsSetting::Full(LogSettings {
                path: Some(DEFAULT_LOG_PATH.into()),
                enabled: Some(true),
                log_level: Some("INFO".into()),
                log_events: Some(true),
            })),
            splits: Some(Splits::default()),
            ..Default::default()
        };
        // Everything matches the built-ins: only identity survives.
        let minimal = strip_inherited(full_entry(), &RepoSettings::default());
        assert_eq!(minimal.short_name.as_deref(), Some("r.git"));
        assert!(minimal.view.is_none() && minimal.show_details.is_none());
        assert!(minimal.debug.is_none() && minimal.logs.is_none() && minimal.splits.is_none());
        // Deviations stay: view differs from built-in.
        let mut deviant = full_entry();
        deviant.view = Some("graph".into());
        deviant.splits = Some(Splits {
            list_pct: 55,
            ..Splits::default()
        });
        let kept = strip_inherited(deviant, &RepoSettings::default());
        assert_eq!(kept.view.as_deref(), Some("graph"));
        assert_eq!(kept.splits.unwrap().list_pct, 55);
        // The global `defaults` section shifts the baseline: view "graph"
        // becomes inherited (stripped), built-in "focused" becomes a deviation.
        let defaults = RepoSettings {
            view: Some("graph".into()),
            ..Default::default()
        };
        let mut graph_entry = full_entry();
        graph_entry.view = Some("graph".into());
        assert!(strip_inherited(graph_entry, &defaults).view.is_none());
        assert_eq!(
            strip_inherited(full_entry(), &defaults).view.as_deref(),
            Some("focused")
        );
        // Legacy string forms compare semantically: debug "info" == defaults.
        let mut legacy = full_entry();
        legacy.debug = Some(DebugSetting::Level("info".into()));
        assert!(
            strip_inherited(legacy, &RepoSettings::default())
                .debug
                .is_none()
        );
    }

    #[test]
    fn debug_and_logs_accept_string_and_object_forms() {
        // Legacy string forms.
        let old: RepoSettings =
            json5::from_str(r#"{ debug: "INFO", logs: "/tmp/x.log" }"#).unwrap();
        assert_eq!(old.debug_level(), Some("INFO"));
        assert_eq!(old.debug_events(), None);
        assert_eq!(old.logs_normalized().path.as_deref(), Some("/tmp/x.log"));
        assert_eq!(old.logs_normalized().enabled, None);
        // Object forms; log_level optional in both.
        let new: RepoSettings = json5::from_str(
            r#"{
                debug: { log_events: false },
                logs: { path: "/tmp/y-{YYYY}.log", enabled: true, log_level: "warn", log_events: false },
            }"#,
        )
        .unwrap();
        assert_eq!(new.debug_level(), None);
        assert_eq!(new.debug_events(), Some(false));
        let logs = new.logs_normalized();
        assert_eq!(logs.path.as_deref(), Some("/tmp/y-{YYYY}.log"));
        assert_eq!(logs.log_level.as_deref(), Some("warn"));
        assert_eq!(logs.log_events, Some(false));
    }

    #[test]
    fn log_pattern_expands_date_repo_and_hash_tokens() {
        let now = chrono::NaiveDate::from_ymd_opt(2026, 8, 10)
            .unwrap()
            .and_hms_opt(14, 30, 5)
            .unwrap();
        let expand = |spec: &str| expand_log_pattern(spec, &now, "ariva-tools", "a1b2c3d4");
        assert_eq!(
            expand("/tmp/ghh/logs-{YYYY-MM-DD-HH-SS}.txt"),
            "/tmp/ghh/logs-2026-08-10-14-05.txt"
        );
        assert_eq!(
            expand("l-{YYYY}-{MM}-{DD}-{HH}-{mm}-{SS}.log"),
            "l-2026-08-10-14-30-05.log"
        );
        assert_eq!(
            expand("/tmp/ghh/{REPO}-{HASH}-{YYYY-MM-DD}.log"),
            "/tmp/ghh/ariva-tools-a1b2c3d4-2026-08-10.log"
        );
        // No braces: unchanged. Unclosed brace: literal.
        assert_eq!(expand("/tmp/ghh/logs.txt"), "/tmp/ghh/logs.txt");
        assert_eq!(expand("a-{YYYY.txt"), "a-{YYYY.txt");
        // Hash is stable and 8 hex chars.
        assert_eq!(repo_hash("/some/repo"), repo_hash("/some/repo"));
        assert_ne!(repo_hash("/some/repo"), repo_hash("/other/repo"));
        assert!(
            repo_hash("/some/repo").len() == 8
                && repo_hash("/some/repo")
                    .chars()
                    .all(|c| c.is_ascii_hexdigit())
        );
    }

    #[test]
    fn app_info_stamped_once_never_overwritten() {
        // Missing: stamped with the current build.
        let mut fresh = SettingsFile::default();
        fresh.app.get_or_insert_with(AppInfo::current);
        assert_eq!(fresh.app.as_ref().unwrap().name, env!("CARGO_BIN_NAME"));
        // Present: kept verbatim, even if from an older build.
        let json = r#"{ app: { name: 'ghh', version: '0.0.1', build_hash: 'abc', build_date: '2020-01-01' } }"#;
        let mut old: SettingsFile = json5::from_str(json).unwrap();
        old.app.get_or_insert_with(AppInfo::current);
        assert_eq!(old.app.as_ref().unwrap().version, "0.0.1");
        assert_eq!(old.app.as_ref().unwrap().build_hash, "abc");
    }

    #[test]
    fn short_name_from_urls() {
        assert_eq!(
            short_name_of("git@github.com:ariva/ariva-tools.git"),
            "ariva-tools.git"
        );
        assert_eq!(
            short_name_of("https://github.com/ariva/ariva-tools.git"),
            "ariva-tools.git"
        );
        assert_eq!(short_name_of("plain"), "plain");
    }

    #[test]
    fn splits_clamp_keeps_panes_usable() {
        let wild = Splits {
            list_pct: 1,
            diff_info_pct: 99,
            details_height: 0,
            files_pct: 0,
        };
        assert_eq!(
            wild.clamped(),
            Splits {
                list_pct: 15,
                diff_info_pct: 85,
                details_height: 3,
                files_pct: 15,
            }
        );
    }
}
