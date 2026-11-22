use super::commands::AutomationCommand;
use super::keys;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Cannot read automation-keys file '{path}': {source}")]
    ReadFile {
        path: String,
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Load automation commands from either a text file or a comma-separated string.
///
/// File is chosen when `spec` is a path that exists on disk or ends with
/// ".txt"; otherwise the spec is treated as comma-separated.
pub fn load_automation_commands(spec: &str) -> Result<Vec<AutomationCommand>> {
    let path = std::path::Path::new(spec);
    if path.exists() || spec.ends_with(".txt") {
        parse_automation_file(spec)
    } else {
        Ok(parse_automation_csv(spec))
    }
}

/// Parse a comma-separated string. Each token is either:
///   - `SLEEP <ms>` — explicit sleep
///   - `<KEY_NAME>`  — key press
fn parse_automation_csv(input: &str) -> Vec<AutomationCommand> {
    input
        .split(',')
        .filter_map(|token| parse_automation_token(token.trim()))
        .collect()
}

/// Parse a text file where each non-blank, non-comment line is one command:
///   `KEY_PRESS <KEY_NAME>`, `SLEEP <ms>`, or `# comment`
fn parse_automation_file(path: &str) -> Result<Vec<AutomationCommand>> {
    let content = std::fs::read_to_string(path).map_err(|source| Error::ReadFile {
        path: path.to_string(),
        source,
    })?;
    let mut cmds = Vec::new();
    for (lineno, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("KEY_PRESS ") {
            match rest.trim().parse::<keys::Key>() {
                Ok(k) => cmds.push(AutomationCommand::KeyPress(k.to_key_event())),
                Err(e) => eprintln!("[automation] line {}: {}", lineno + 1, e),
            }
        } else if let Some(rest) = line.strip_prefix("SLEEP ") {
            match rest.trim().parse::<u64>() {
                Ok(ms) => cmds.push(AutomationCommand::Sleep(ms)),
                Err(_) => eprintln!(
                    "[automation] line {}: invalid SLEEP value '{}'",
                    lineno + 1,
                    rest.trim()
                ),
            }
        } else if let Some(rest) = line.strip_prefix("KEY_PRESS_DELAY ") {
            match rest.trim().parse::<u64>() {
                Ok(ms) => cmds.push(AutomationCommand::KeyPressDelay(ms)),
                Err(_) => eprintln!(
                    "[automation] line {}: invalid KEY_PRESS_DELAY value '{}'",
                    lineno + 1,
                    rest.trim()
                ),
            }
        } else {
            eprintln!(
                "[automation] line {}: unrecognised command '{}'",
                lineno + 1,
                line
            );
        }
    }
    Ok(cmds)
}

/// Parse one token from the CSV format (no KEY_PRESS prefix needed).
fn parse_automation_token(token: &str) -> Option<AutomationCommand> {
    if token.is_empty() {
        return None;
    }
    if let Some(rest) = token.strip_prefix("SLEEP ") {
        if let Ok(ms) = rest.trim().parse::<u64>() {
            return Some(AutomationCommand::Sleep(ms));
        }
        eprintln!("[automation] invalid SLEEP value '{}'", rest.trim());
        return None;
    }
    if let Some(rest) = token.strip_prefix("KEY_PRESS_DELAY ") {
        if let Ok(ms) = rest.trim().parse::<u64>() {
            return Some(AutomationCommand::KeyPressDelay(ms));
        }
        eprintln!(
            "[automation] invalid KEY_PRESS_DELAY value '{}'",
            rest.trim()
        );
        return None;
    }
    match token.parse::<keys::Key>() {
        Ok(k) => Some(AutomationCommand::KeyPress(k.to_key_event())),
        Err(e) => {
            eprintln!("[automation] {}", e);
            None
        }
    }
}
