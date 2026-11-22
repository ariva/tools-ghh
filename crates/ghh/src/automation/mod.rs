mod commands;
pub mod keys;
mod loader;

pub use commands::AutomationCommand;
pub use loader::{Error, load_automation_commands};
