//! Right-click / `.` context menu: building actions, executing them and
//! copying commit data to the clipboard.

use crate::app::App;
use crate::app::models::{
    CommitDiffFile, CommitListFocus, ContextMenuAction, ContextMenuState, parse_diff_files,
};
use crate::git;

impl App {
    pub(crate) fn is_commit_context_position(&self, column: u16, row: u16) -> bool {
        self.selected_commit().is_some()
            && (self.commit_diff_regions().is_some_and(|(info, diff)| {
                Self::rect_contains(info, column, row) || Self::rect_contains(diff, column, row)
            }) || self
                .commit_details_region()
                .is_some_and(|details| Self::rect_contains(details, column, row)))
    }

    pub(crate) fn open_context_menu(
        &mut self,
        x: u16,
        y: u16,
        mut actions: Vec<ContextMenuAction>,
    ) {
        if self
            .text_selection
            .as_ref()
            .is_some_and(|selection| !selection.text.trim().is_empty())
        {
            actions.insert(0, ContextMenuAction::CopySelection);
        }
        if actions.is_empty() {
            return;
        }
        self.context_menu = Some(ContextMenuState {
            x,
            y,
            actions,
            selected: 0,
            clipboard_available: Self::read_clipboard_text().is_some(),
        });
    }

    pub(crate) fn commit_copy_context_actions() -> Vec<ContextMenuAction> {
        vec![
            ContextMenuAction::CopyHash,
            ContextMenuAction::CopyAuthor,
            ContextMenuAction::CopyCommitMessage,
            ContextMenuAction::CopyChangedFiles,
            ContextMenuAction::CopyAllInfo,
            ContextMenuAction::CopyDiffPatch,
        ]
    }

    pub(crate) fn commit_list_context_actions(&self) -> Vec<ContextMenuAction> {
        let mut actions = Self::commit_copy_context_actions();
        actions.push(ContextMenuAction::GotoMergeCommit);
        actions.push(ContextMenuAction::DrillIntoMerge);
        actions.push(if self.show_commit_diff {
            ContextMenuAction::HideDetails
        } else {
            ContextMenuAction::ViewDetails
        });
        // Destructive entries go last; the renderer fences them off.
        actions.push(ContextMenuAction::EditCommit);
        actions.push(ContextMenuAction::SquashFromHere);
        actions
    }

    pub(crate) fn open_context_menu_for_active_pane(&mut self) {
        if self.selected_commit().is_none() {
            self.status_message = Some("No commit selected.".into());
            return;
        }

        let pane_and_actions = match self.commit_list_focus {
            CommitListFocus::List => self
                .commit_list_regions()
                .map(|(list, _)| (list, self.commit_list_context_actions())),
            CommitListFocus::DiffInfo | CommitListFocus::Diff if self.show_commit_diff => {
                self.commit_diff_regions().map(|(info, diff)| {
                    let pane = if self.commit_list_focus == CommitListFocus::DiffInfo {
                        info
                    } else {
                        diff
                    };
                    (pane, Self::commit_copy_context_actions())
                })
            }
            CommitListFocus::DiffInfo
            | CommitListFocus::Files
            | CommitListFocus::Diff
            | CommitListFocus::Details => None,
        };

        if let Some((pane, actions)) = pane_and_actions {
            self.open_context_menu(pane.x.saturating_add(1), pane.y.saturating_add(1), actions);
        }
    }

    pub(crate) fn execute_context_menu_action(&mut self) {
        let Some(menu) = self.context_menu.take() else {
            return;
        };
        let Some(action) = menu.actions.get(menu.selected).copied() else {
            return;
        };
        match action {
            ContextMenuAction::CopySelection => self.copy_selected_text(),
            ContextMenuAction::ViewDetails | ContextMenuAction::HideDetails => {
                self.toggle_commit_diff();
            }
            ContextMenuAction::CopyHash
            | ContextMenuAction::CopyAuthor
            | ContextMenuAction::CopyCommitMessage
            | ContextMenuAction::CopyChangedFiles
            | ContextMenuAction::CopyAllInfo
            | ContextMenuAction::CopyDiffPatch => self.copy_commit_context(action),
            ContextMenuAction::GotoMergeCommit => self.jump_to_merge_point(),
            ContextMenuAction::DrillIntoMerge => self.drill_into_merge_of_selected(),
            ContextMenuAction::PasteClipboard
                if menu.clipboard_available && self.can_paste_here() =>
            {
                self.paste_from_clipboard();
            }
            ContextMenuAction::PasteClipboard => {}
            ContextMenuAction::EditCommit => {
                if !self.full_history_required("editing commits") {
                    self.enter_edit();
                }
            }
            ContextMenuAction::SquashFromHere => {
                if !self.full_history_required("starting squash") {
                    self.enter_squash();
                }
            }
        }
    }

    pub(crate) fn copy_commit_context(&mut self, action: ContextMenuAction) {
        let Some(hash) = self.selected_commit().map(|commit| commit.hash.clone()) else {
            self.status_message = Some("No commit selected.".into());
            return;
        };

        if let Err(error) = self.load_commit_copy_data(&hash, action) {
            self.status_message = Some(format!("Unable to load commit data: {error}"));
            return;
        }

        let Some(commit) = self.selected_commit() else {
            self.status_message = Some("No commit selected.".into());
            return;
        };
        let hash = commit.hash.clone();
        let author = if commit.author_email.is_empty() {
            commit.author_name.clone()
        } else {
            format!("{} <{}>", commit.author_name, commit.author_email)
        };
        let message = commit
            .full_message
            .clone()
            .unwrap_or_else(|| commit.message.clone());
        let files = self
            .commit_diff_file_cache
            .get(&hash)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let stats = self.stats_cache.get(&hash);
        let text = match action {
            ContextMenuAction::CopyHash => hash,
            ContextMenuAction::CopyAuthor => author,
            ContextMenuAction::CopyCommitMessage => message,
            ContextMenuAction::CopyChangedFiles => Self::format_changed_files_info(stats, files),
            ContextMenuAction::CopyDiffPatch => match git::get_commit_patch(&self.repo_path, &hash)
            {
                Ok(patch) => patch,
                Err(error) => {
                    self.status_message = Some(format!("Unable to load patch: {error}"));
                    return;
                }
            },
            ContextMenuAction::CopyAllInfo => format!(
                "Hash: {}\nAuthor: {}\nDate: {}\n{}\n\nMessage:\n{}\n\nFiles:\n{}",
                commit.hash,
                author,
                commit.date_display_text(),
                Self::format_change_summary(stats),
                message,
                Self::format_file_entries(files),
            ),
            ContextMenuAction::CopySelection
            | ContextMenuAction::ViewDetails
            | ContextMenuAction::HideDetails
            | ContextMenuAction::GotoMergeCommit
            | ContextMenuAction::DrillIntoMerge
            | ContextMenuAction::PasteClipboard
            | ContextMenuAction::EditCommit
            | ContextMenuAction::SquashFromHere => String::new(),
        };
        if text.is_empty() {
            self.status_message = Some("Requested commit data is not loaded yet.".into());
        } else if self.write_clipboard_text(&text) {
            self.status_message = Some(format!(
                "Copied {} characters to clipboard.",
                text.chars().count()
            ));
        } else {
            self.status_message = Some("Clipboard copy unavailable.".into());
        }
    }

    pub(crate) fn load_commit_copy_data(
        &mut self,
        hash: &str,
        action: ContextMenuAction,
    ) -> std::result::Result<(), String> {
        let needs_author = matches!(
            action,
            ContextMenuAction::CopyAuthor | ContextMenuAction::CopyAllInfo
        );
        let needs_message = matches!(
            action,
            ContextMenuAction::CopyCommitMessage | ContextMenuAction::CopyAllInfo
        );
        let needs_changed_files = matches!(
            action,
            ContextMenuAction::CopyChangedFiles | ContextMenuAction::CopyAllInfo
        );

        if needs_changed_files && !self.stats_cache.contains_key(hash) {
            let stats =
                git::get_commit_stats(&self.repo_path, hash).map_err(|error| error.to_string())?;
            self.stats_cache.insert(hash.to_string(), stats);
        }
        let commit_idx = self.commit_position(hash);
        if needs_author
            && let Some(idx) = commit_idx
            && self.commits[idx].author_email.is_empty()
        {
            let email = git::get_commit_author_email(&self.repo_path, hash)
                .map_err(|error| error.to_string())?;
            self.commits[idx].author_email = email;
        }
        if needs_message
            && let Some(idx) = commit_idx
            && self.commits[idx].full_message.is_none()
        {
            let message = git::get_commit_full_message(&self.repo_path, hash)
                .map_err(|error| error.to_string())?;
            self.commits[idx].full_message = Some(message);
        }
        if needs_changed_files && !self.commit_diff_file_cache.contains_key(hash) {
            let diff =
                git::get_commit_diff(&self.repo_path, hash).map_err(|error| error.to_string())?;
            let lines: Vec<String> = diff.lines().map(String::from).collect();
            self.commit_diff_file_cache
                .insert(hash.to_string(), parse_diff_files(&lines));
            self.commit_diff_cache
                .entry(hash.to_string())
                .or_insert(lines);
        }
        Ok(())
    }

    pub(crate) fn format_changed_files_info(
        stats: Option<&git::CommitStats>,
        files: &[CommitDiffFile],
    ) -> String {
        let stats = stats.cloned().unwrap_or(git::CommitStats {
            files_added: 0,
            files_modified: 0,
            files_deleted: 0,
            lines_inserted: 0,
            lines_deleted: 0,
        });
        let file_count = stats.files_added + stats.files_modified + stats.files_deleted;
        let entries = Self::format_file_entries(files);
        let header = format!(
            "Files: +{} ~{} -{}\n\nChanges: +{} -{} ({} files):",
            stats.files_added,
            stats.files_modified,
            stats.files_deleted,
            stats.lines_inserted,
            stats.lines_deleted,
            file_count
        );
        if entries.is_empty() {
            header
        } else {
            format!("{header}\n{entries}")
        }
    }

    pub(crate) fn format_change_summary(stats: Option<&git::CommitStats>) -> String {
        let stats = stats.cloned().unwrap_or(git::CommitStats {
            files_added: 0,
            files_modified: 0,
            files_deleted: 0,
            lines_inserted: 0,
            lines_deleted: 0,
        });
        let file_count = stats.files_added + stats.files_modified + stats.files_deleted;
        [
            format!(
                "Changes: +{} -{} ({} files)",
                stats.lines_inserted, stats.lines_deleted, file_count
            ),
            format!(
                "Files: +{} ~{} -{}",
                stats.files_added, stats.files_modified, stats.files_deleted
            ),
        ]
        .join("\n")
    }

    pub(crate) fn format_file_entries(files: &[CommitDiffFile]) -> String {
        files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                format!(
                    "{}. +{} -{}  {}",
                    index + 1,
                    file.additions,
                    file.deletions,
                    file.path
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
