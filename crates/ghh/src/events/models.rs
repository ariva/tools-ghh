use chrono::Local;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;

/// Timestamp carrying both a monotonic `Instant` (for elapsed-ms math) and a
/// formatted local wall-clock string (for human-readable display).
#[derive(Debug, Clone)]
pub struct EventTime {
    #[allow(dead_code)]
    pub instant: Instant,
    /// Local wall-clock time formatted as `"YYYY-MM-DD HH:MM:SS"`.
    pub local: String,
}

impl EventTime {
    pub fn now() -> Self {
        Self {
            instant: Instant::now(),
            local: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        }
    }
}

/// Human-readable identifier for each logical screen/mode.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub enum ScreenId {
    CommitList,
    CommitEdit,
    CommitEditMessage,
    Staging,
    Stash,
    StagingMessageEditor,
    SquashPicker,
    SquashMessageEditor,
    Confirm,
    Debugger,
}

impl ScreenId {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScreenId::CommitList => "commit-list",
            ScreenId::CommitEdit => "commit-edit",
            ScreenId::CommitEditMessage => "commit-edit-message",
            ScreenId::Staging => "staging",
            ScreenId::Stash => "stash",
            ScreenId::StagingMessageEditor => "staging-msg-editor",
            ScreenId::SquashPicker => "squash-picker",
            ScreenId::SquashMessageEditor => "squash-msg-editor",
            ScreenId::Confirm => "confirm",
            ScreenId::Debugger => "debugger",
        }
    }
}

impl std::fmt::Display for ScreenId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Convert a crossterm `KeyEvent` to a short human-readable string (e.g. `"Ctrl+S"`, `"j"`).
pub fn format_key_event(key: KeyEvent) -> String {
    let mut mods = Vec::new();
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        mods.push("Ctrl");
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        mods.push("Alt");
    }
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        mods.push("Shift");
    }
    let name = match key.code {
        KeyCode::Char(c) => format!("{c}"),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Esc => "Esc".into(),
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Delete => "Delete".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::BackTab => "BackTab".into(),
        KeyCode::Up => "↑".into(),
        KeyCode::Down => "↓".into(),
        KeyCode::Left => "←".into(),
        KeyCode::Right => "→".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::PageUp => "PageUp".into(),
        KeyCode::PageDown => "PageDown".into(),
        KeyCode::F(n) => format!("F{n}"),
        _ => "?".into(),
    };
    if mods.is_empty() {
        name
    } else {
        format!("{}+{name}", mods.join("+"))
    }
}

/// High-level application actions (emitted after execution).
#[derive(Debug, Clone)]
pub enum ActionKind {
    CommitModified,
    CommitSquashed { count: usize },
    CommitCreated,
    BranchCollapsed { hash: String },
    BranchExpanded { hash: String },
}

impl ActionKind {
    pub fn as_str(&self) -> String {
        match self {
            ActionKind::CommitModified => "commit-modified".into(),
            ActionKind::CommitSquashed { count } => format!("commit-squashed({})", count),
            ActionKind::CommitCreated => "commit-created".into(),
            ActionKind::BranchCollapsed { hash } => {
                format!("branch-collapsed({})", &hash[..hash.len().min(7)])
            }
            ActionKind::BranchExpanded { hash } => {
                format!("branch-expanded({})", &hash[..hash.len().min(7)])
            }
        }
    }
}

/// Variant order is severity order (Trace < Debug < Info < Warn < Error),
/// so the derived `Ord` gives min-level filtering for free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[allow(dead_code)]
pub enum TraceLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl TraceLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            TraceLevel::Trace => "TRC",
            TraceLevel::Debug => "DBG",
            TraceLevel::Info => "INF",
            TraceLevel::Warn => "WRN",
            TraceLevel::Error => "ERR",
        }
    }

    /// Parse the settings `debug` value ("TRACE"/"DEBUG"/"INFO"/"WARN"/
    /// "ERROR", case-insensitive). None for anything else.
    pub fn from_setting(value: &str) -> Option<Self> {
        match value.to_ascii_uppercase().as_str() {
            "TRACE" => Some(TraceLevel::Trace),
            "DEBUG" => Some(TraceLevel::Debug),
            "INFO" => Some(TraceLevel::Info),
            "WARN" => Some(TraceLevel::Warn),
            "ERROR" => Some(TraceLevel::Error),
            _ => None,
        }
    }

    /// The canonical settings-file spelling of this level.
    pub fn setting_str(self) -> &'static str {
        match self {
            TraceLevel::Trace => "TRACE",
            TraceLevel::Debug => "DEBUG",
            TraceLevel::Info => "INFO",
            TraceLevel::Warn => "WARN",
            TraceLevel::Error => "ERROR",
        }
    }
}

/// All events emitted by the application.
#[derive(Debug, Clone)]
pub enum AppEvent {
    KeyPress {
        display: String, // "Ctrl+S", "j", "Enter", …
        screen: ScreenId,
        at: EventTime,
    },
    ScreenChange {
        from: ScreenId,
        to: ScreenId,
        at: EventTime,
    },
    Action {
        kind: ActionKind,
        screen: ScreenId,
        at: EventTime,
    },
    #[allow(dead_code)]
    Trace {
        level: TraceLevel,
        msg: String,
        at: EventTime,
    },
}

impl AppEvent {
    pub fn at(&self) -> &EventTime {
        match self {
            AppEvent::KeyPress { at, .. } => at,
            AppEvent::ScreenChange { at, .. } => at,
            AppEvent::Action { at, .. } => at,
            AppEvent::Trace { at, .. } => at,
        }
    }

    /// Short uppercase tag for display.
    pub fn type_tag(&self) -> &'static str {
        match self {
            AppEvent::KeyPress { .. } => "KEY ",
            AppEvent::ScreenChange { .. } => "SCRN",
            AppEvent::Action { .. } => "ACT ",
            AppEvent::Trace { .. } => "TRC ",
        }
    }

    /// Human-readable description (without timestamp / tag).
    pub fn description(&self) -> String {
        match self {
            AppEvent::KeyPress {
                display, screen, ..
            } => format!("{:<22} {}", screen.as_str(), display),
            AppEvent::ScreenChange { from, to, .. } => {
                format!("{} \u{2192} {}", from.as_str(), to.as_str())
            }
            AppEvent::Action { kind, screen, .. } => {
                format!("{:<22} {}", screen.as_str(), kind.as_str())
            }
            AppEvent::Trace { level, msg, .. } => format!("[{}] {}", level.as_str(), msg),
        }
    }
}

#[cfg(test)]
mod trace_level_tests {
    use super::TraceLevel;

    #[test]
    fn parses_settings_values_case_insensitive() {
        assert_eq!(TraceLevel::from_setting("INFO"), Some(TraceLevel::Info));
        assert_eq!(TraceLevel::from_setting("warn"), Some(TraceLevel::Warn));
        assert_eq!(TraceLevel::from_setting("Debug"), Some(TraceLevel::Debug));
        assert_eq!(TraceLevel::from_setting("error"), Some(TraceLevel::Error));
        assert_eq!(TraceLevel::from_setting("trace"), Some(TraceLevel::Trace));
        assert_eq!(TraceLevel::from_setting("verbose"), None);
    }

    #[test]
    fn severity_order_drives_min_level_filtering() {
        // "debug: INFO" must show Info/Warn/Error and hide Debug.
        let min = TraceLevel::Info;
        assert!(TraceLevel::Info >= min);
        assert!(TraceLevel::Error >= min);
        assert!(TraceLevel::Debug < min);
        assert!(TraceLevel::Trace < TraceLevel::Debug);
        // Round-trip: what save_settings writes, from_setting reads back.
        for level in [
            TraceLevel::Trace,
            TraceLevel::Debug,
            TraceLevel::Info,
            TraceLevel::Warn,
            TraceLevel::Error,
        ] {
            assert_eq!(TraceLevel::from_setting(level.setting_str()), Some(level));
        }
    }
}
