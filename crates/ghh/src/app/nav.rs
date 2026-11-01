//! Stack-based navigation between screens and popups.
//!
//! The app is always in exactly one [`Mode`]. Modes form a stack: the bottom
//! entry is the root screen, and every entry above it is a screen or popup
//! opened from the one below (e.g. `Staging` → `EditCommitMessage` →
//! `Confirm`). Closing the top entry returns to the entry underneath it, so
//! call sites never need to remember *where to go back to* — the stack does.
//!
//! Rules of thumb for call sites:
//! - Opening a screen/popup on top of the current one: [`Navigator::push`].
//! - Closing the current screen/popup ("Esc", "Cancel"): [`Navigator::pop`].
//! - Finishing a whole flow (commit created, squash done): [`Navigator::reset`].

use crate::app::models::Mode;

pub struct Navigator {
    stack: Vec<Mode>,
}

impl Default for Navigator {
    fn default() -> Self {
        Self::new()
    }
}

impl Navigator {
    /// Start at the root screen (`Mode::Normal`, the commit list).
    pub fn new() -> Self {
        Self {
            stack: vec![Mode::Normal],
        }
    }

    /// The active mode (top of the stack).
    pub fn mode(&self) -> &Mode {
        self.stack.last().expect("navigation stack is never empty")
    }

    /// Mutable access to the active mode (e.g. toggling the Yes/No selection
    /// inside `Mode::Confirm`).
    pub fn mode_mut(&mut self) -> &mut Mode {
        self.stack
            .last_mut()
            .expect("navigation stack is never empty")
    }

    /// Open `mode` on top of the current screen/popup.
    pub fn push(&mut self, mode: Mode) {
        self.stack.push(mode);
    }

    /// Close the active screen/popup and return it; the mode underneath
    /// becomes active again. The root entry cannot be popped.
    pub fn pop(&mut self) -> Option<Mode> {
        if self.stack.len() > 1 {
            self.stack.pop()
        } else {
            None
        }
    }

    /// Drop the whole stack and make `mode` the new root. Used when a flow
    /// finishes and the app returns to a known base state.
    pub fn reset(&mut self, mode: Mode) {
        self.stack.clear();
        self.stack.push(mode);
    }

    /// Current stack depth (1 = only the root screen).
    #[cfg(test)]
    pub fn depth(&self) -> usize {
        self.stack.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::models::ConfirmAction;

    #[test]
    fn starts_at_normal_root() {
        let nav = Navigator::new();
        assert_eq!(*nav.mode(), Mode::Normal);
        assert_eq!(nav.depth(), 1);
    }

    #[test]
    fn push_and_pop_restore_previous_mode() {
        let mut nav = Navigator::new();
        nav.push(Mode::Staging);
        nav.push(Mode::EditCommitMessage);
        assert_eq!(*nav.mode(), Mode::EditCommitMessage);

        assert_eq!(nav.pop(), Some(Mode::EditCommitMessage));
        assert_eq!(*nav.mode(), Mode::Staging);
        assert_eq!(nav.pop(), Some(Mode::Staging));
        assert_eq!(*nav.mode(), Mode::Normal);
    }

    #[test]
    fn root_cannot_be_popped() {
        let mut nav = Navigator::new();
        assert_eq!(nav.pop(), None);
        assert_eq!(*nav.mode(), Mode::Normal);
    }

    #[test]
    fn confirm_cancel_returns_to_opener() {
        // Staging → message editor → confirm-discard; cancelling the confirm
        // must land back in the message editor, not in staging.
        let mut nav = Navigator::new();
        nav.push(Mode::Staging);
        nav.push(Mode::EditCommitMessage);
        nav.push(Mode::Confirm(
            ConfirmAction::DiscardStagingMessageEditor,
            false,
        ));

        let popped = nav.pop();
        assert!(matches!(popped, Some(Mode::Confirm(..))));
        assert_eq!(*nav.mode(), Mode::EditCommitMessage);
    }

    #[test]
    fn reset_clears_overlays() {
        let mut nav = Navigator::new();
        nav.push(Mode::Squash);
        nav.push(Mode::SquashMessage);
        nav.reset(Mode::Normal);
        assert_eq!(*nav.mode(), Mode::Normal);
        assert_eq!(nav.depth(), 1);
    }

    #[test]
    fn mode_mut_edits_top_entry() {
        let mut nav = Navigator::new();
        nav.push(Mode::Confirm(ConfirmAction::SaveEdit, true));
        if let Mode::Confirm(_, yes) = nav.mode_mut() {
            *yes = false;
        }
        assert!(matches!(nav.mode(), Mode::Confirm(_, false)));
    }
}
