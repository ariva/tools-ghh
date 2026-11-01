use crossterm::event::KeyEvent;

/// A single step in an automation script.
#[derive(Debug, Clone)]
pub enum AutomationCommand {
    /// Inject a synthetic key-press into the app.
    KeyPress(KeyEvent),
    /// Pause for the given number of milliseconds.
    Sleep(u64),
    /// Override the inter-key delay for all subsequent KEY_PRESS commands.
    KeyPressDelay(u64),
}
