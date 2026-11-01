use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::str::FromStr;

/// All keys that can appear in an automation script.
///
/// Supported name strings (used by `FromStr`)
/// -------------------------------------------
/// Navigation : ARROW_UP  ARROW_DOWN  ARROW_LEFT  ARROW_RIGHT
///              HOME  END  PAGE_UP  PAGE_DOWN
/// Control    : ENTER  ESC / ESCAPE  BACKSPACE  DELETE / DEL  TAB  SPACE
/// Symbols    : SLASH (/)  DOT (.)  QUESTION (?)  — search, context menu, help
/// Function   : F1 … F12
/// Letters    : UPPER_A … UPPER_Z   (Shift + letter)
///              LOWER_A … LOWER_Z   (plain letter)
/// Digits     : DIGIT_0 … DIGIT_9
/// Ctrl combos: CTRL_C  CTRL_D  CTRL_Q  CTRL_S  CTRL_Z
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    // ── navigation ─────────────────────────────────────────────────────────
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    // ── control ────────────────────────────────────────────────────────────
    Enter,
    Esc,
    Backspace,
    Delete,
    Tab,
    Space,
    // ── symbols used by app shortcuts ──────────────────────────────────────
    Slash,    // opens search
    Dot,      // opens context menu
    Question, // opens help
    // ── function keys ──────────────────────────────────────────────────────
    F(u8), // 1–12
    // ── letters & digits ───────────────────────────────────────────────────
    Upper(char), // A–Z  →  Shift + uppercase
    Lower(char), // a–z  →  plain lowercase
    Digit(char), // 0–9
    // ── ctrl combos ────────────────────────────────────────────────────────
    CtrlC,
    CtrlD,
    CtrlQ,
    CtrlS,
    CtrlZ,
}

impl Key {
    /// Convert to the [`KeyEvent`] that crossterm would produce for this key.
    pub fn to_key_event(self) -> KeyEvent {
        match self {
            Key::ArrowUp => KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            Key::ArrowDown => KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            Key::ArrowLeft => KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            Key::ArrowRight => KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            Key::Home => KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
            Key::End => KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            Key::PageUp => KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
            Key::PageDown => KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            Key::Enter => KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            Key::Esc => KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            Key::Backspace => KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            Key::Delete => KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
            Key::Tab => KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            Key::Space => KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            Key::Slash => KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            Key::Dot => KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE),
            Key::Question => KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
            Key::F(n) => KeyEvent::new(KeyCode::F(n), KeyModifiers::NONE),
            Key::Upper(c) => {
                KeyEvent::new(KeyCode::Char(c.to_ascii_uppercase()), KeyModifiers::SHIFT)
            }
            Key::Lower(c) => {
                KeyEvent::new(KeyCode::Char(c.to_ascii_lowercase()), KeyModifiers::NONE)
            }
            Key::Digit(c) => KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            Key::CtrlC => KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Key::CtrlD => KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
            Key::CtrlQ => KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
            Key::CtrlS => KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            Key::CtrlZ => KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL),
        }
    }
}

impl FromStr for Key {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();

        // ── named keys ─────────────────────────────────────────────────────
        match s {
            "ARROW_UP" => return Ok(Key::ArrowUp),
            "ARROW_DOWN" => return Ok(Key::ArrowDown),
            "ARROW_LEFT" => return Ok(Key::ArrowLeft),
            "ARROW_RIGHT" => return Ok(Key::ArrowRight),
            "HOME" => return Ok(Key::Home),
            "END" => return Ok(Key::End),
            "PAGE_UP" => return Ok(Key::PageUp),
            "PAGE_DOWN" => return Ok(Key::PageDown),
            "ENTER" => return Ok(Key::Enter),
            "ESC" | "ESCAPE" => return Ok(Key::Esc),
            "BACKSPACE" => return Ok(Key::Backspace),
            "DELETE" | "DEL" => return Ok(Key::Delete),
            "TAB" => return Ok(Key::Tab),
            "SPACE" => return Ok(Key::Space),
            "SLASH" => return Ok(Key::Slash),
            "DOT" => return Ok(Key::Dot),
            "QUESTION" => return Ok(Key::Question),
            "F1" => return Ok(Key::F(1)),
            "F2" => return Ok(Key::F(2)),
            "F3" => return Ok(Key::F(3)),
            "F4" => return Ok(Key::F(4)),
            "F5" => return Ok(Key::F(5)),
            "F6" => return Ok(Key::F(6)),
            "F7" => return Ok(Key::F(7)),
            "F8" => return Ok(Key::F(8)),
            "F9" => return Ok(Key::F(9)),
            "F10" => return Ok(Key::F(10)),
            "F11" => return Ok(Key::F(11)),
            "F12" => return Ok(Key::F(12)),
            "CTRL_C" => return Ok(Key::CtrlC),
            "CTRL_D" => return Ok(Key::CtrlD),
            "CTRL_Q" => return Ok(Key::CtrlQ),
            "CTRL_S" => return Ok(Key::CtrlS),
            "CTRL_Z" => return Ok(Key::CtrlZ),
            _ => {}
        }

        // ── UPPER_A … UPPER_Z ──────────────────────────────────────────────
        if let Some(rest) = s.strip_prefix("UPPER_")
            && let Some(c) = single_ascii_alpha(rest)
        {
            return Ok(Key::Upper(c));
        }

        // ── LOWER_A … LOWER_Z ──────────────────────────────────────────────
        if let Some(rest) = s.strip_prefix("LOWER_")
            && let Some(c) = single_ascii_alpha(rest)
        {
            return Ok(Key::Lower(c));
        }

        // ── DIGIT_0 … DIGIT_9 ──────────────────────────────────────────────
        if let Some(rest) = s.strip_prefix("DIGIT_")
            && let Some(c) = single_ascii_digit(rest)
        {
            return Ok(Key::Digit(c));
        }

        Err(format!("unknown key '{s}'"))
    }
}

fn single_ascii_alpha(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let c = chars.next()?;
    if chars.next().is_none() && c.is_ascii_alphabetic() {
        Some(c)
    } else {
        None
    }
}

fn single_ascii_digit(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let c = chars.next()?;
    if chars.next().is_none() && c.is_ascii_digit() {
        Some(c)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    #[test]
    fn arrow_keys() {
        let up: Key = "ARROW_UP".parse().unwrap();
        assert_eq!(up.to_key_event().code, KeyCode::Up);
        let down: Key = "ARROW_DOWN".parse().unwrap();
        assert_eq!(down.to_key_event().code, KeyCode::Down);
    }

    #[test]
    fn upper_lower_letters() {
        let up: Key = "UPPER_G".parse().unwrap();
        let ev = up.to_key_event();
        assert_eq!(ev.code, KeyCode::Char('G'));
        assert!(ev.modifiers.contains(KeyModifiers::SHIFT));

        let lo: Key = "LOWER_J".parse().unwrap();
        let ev = lo.to_key_event();
        assert_eq!(ev.code, KeyCode::Char('j'));
        assert!(!ev.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn digits() {
        let d: Key = "DIGIT_5".parse().unwrap();
        assert_eq!(d.to_key_event().code, KeyCode::Char('5'));
    }

    #[test]
    fn symbol_keys() {
        assert_eq!(
            "SLASH".parse::<Key>().unwrap().to_key_event().code,
            KeyCode::Char('/')
        );
        assert_eq!(
            "DOT".parse::<Key>().unwrap().to_key_event().code,
            KeyCode::Char('.')
        );
        assert_eq!(
            "QUESTION".parse::<Key>().unwrap().to_key_event().code,
            KeyCode::Char('?')
        );
    }

    #[test]
    fn unknown_returns_err() {
        assert!("NOT_A_KEY".parse::<Key>().is_err());
    }
}
