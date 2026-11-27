//! Follows the repository while the commit list or the staging screen is
//! shown: every few seconds one background reading (`git::repo_snapshot`) is
//! compared with what is on screen. Working tree and label changes are
//! applied; a moved HEAD only raises a notice, the list is reloaded when the
//! user asks (`r`).

use crate::app::App;
use crate::app::features::working_tree::WORKING_TREE_KEY;
use crate::app::models::Mode;
use crate::git;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

/// Seconds between readings when the settings say nothing.
const DEFAULT_WATCH_SECS: u64 = 2;
/// A reading may use at most a tenth of one core: the wait before the next
/// one is at least this many times as long as the reading took.
const SCAN_BACKOFF: u32 = 10;

#[derive(Default)]
pub struct RepoWatchState {
    /// A reading in flight; `None` inside = it failed.
    rx: Option<Receiver<Option<git::RepoSnapshot>>>,
    /// Time between readings; `None` = watching is off (`watch_secs: 0`).
    interval: Option<Duration>,
    /// Earliest time of the next timed reading; `None` = now.
    next_due: Option<Instant>,
    /// `RepoSnapshot::stamp` of the reading the row 0 diff belongs to.
    stamp: u64,
    /// Branch of the previous reading, to tell a switch from a new commit.
    branch: Option<Option<String>>,
    /// HEAD is not the commit the list was loaded for. Stays on the status
    /// line until the list is reloaded or HEAD is back.
    pub notice: Option<String>,
}

impl RepoWatchState {
    /// Forget a reading in flight; the next timed one starts as planned.
    pub(crate) fn discard_pending(&mut self) {
        self.rx = None;
    }

    /// `watch_secs` from the settings: missing = default, 0 = off.
    pub(crate) fn new(watch_secs: Option<u64>) -> Self {
        let secs = watch_secs.unwrap_or(DEFAULT_WATCH_SECS);
        Self {
            interval: (secs > 0).then(|| Duration::from_secs(secs.min(60))),
            ..Default::default()
        }
    }
}

/// Wait before the next reading: the interval, longer after a slow reading.
fn next_wait(interval: Duration, took: Duration) -> Duration {
    interval.max(took.saturating_mul(SCAN_BACKOFF))
}

impl App {
    /// Read the repository again, off the UI thread. A newer call replaces
    /// a pending one, so a reading started before an in-app change never
    /// lands after it.
    pub(crate) fn refresh_working_tree(&mut self) {
        let repo_path = self.repo_path.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let snapshot = git::repo_snapshot(&repo_path)
                .inspect_err(|e| tracing::debug!("repository reading failed: {e}"))
                .ok();
            let _ = tx.send(snapshot);
        });
        self.repo_watch.rx = Some(rx);
    }

    /// Once per main loop turn: take a finished reading, start the next one
    /// when it is due. Costs one channel poll and one clock read when
    /// nothing happened.
    pub(crate) fn maybe_poll_repo(&mut self) {
        // The commit list and staging follow the repository. Not the
        // others: squash and move hold row indices, and nothing may change
        // under a popup or the message editor.
        if !matches!(self.mode(), Mode::Normal | Mode::Staging) {
            return;
        }
        if let Some(rx) = self.repo_watch.rx.take() {
            match rx.try_recv() {
                Ok(Some(snapshot)) => self.apply_repo_snapshot(snapshot),
                Ok(None) | Err(TryRecvError::Disconnected) => self.schedule_next_scan(None),
                Err(TryRecvError::Empty) => self.repo_watch.rx = Some(rx),
            }
            return;
        }
        let due = self
            .repo_watch
            .next_due
            .is_none_or(|due| Instant::now() >= due);
        if self.repo_watch.interval.is_some() && due && !self.history_loading {
            self.refresh_working_tree();
        }
    }

    fn schedule_next_scan(&mut self, took: Option<Duration>) {
        let interval = self.repo_watch.interval.unwrap_or_default();
        self.repo_watch.next_due =
            Some(Instant::now() + next_wait(interval, took.unwrap_or_default()));
    }

    /// Apply what differs from the screen, and nothing when nothing does.
    pub(crate) fn apply_repo_snapshot(&mut self, snapshot: git::RepoSnapshot) {
        let started = Instant::now();
        self.push_perf_metric("watch.scan", snapshot.took);
        self.schedule_next_scan(Some(snapshot.took));

        let content_changed = snapshot.stamp != self.repo_watch.stamp;
        self.repo_watch.stamp = snapshot.stamp;
        if content_changed {
            // The row 0 patch is stale whichever screen is open.
            self.commit_diff_cache.remove(WORKING_TREE_KEY);
            self.commit_diff_file_cache.remove(WORKING_TREE_KEY);
        }
        if matches!(self.mode(), Mode::Staging) {
            // Row 0 of the commit list is read again on the way back.
            let repo_path = self.repo_path.clone();
            if let Some(staging) = self.staging.as_mut() {
                staging.apply_outside_change(&repo_path, snapshot.files, content_changed);
            }
        } else if snapshot.files != self.working_tree.files {
            self.set_working_tree(snapshot.files);
        } else if content_changed && self.working_tree_selected() {
            // Same files, other content: the row and the scroll positions
            // stay where they are, only the patch loads again.
            self.schedule_selected_stats_load();
        }

        let switched = self
            .repo_watch
            .branch
            .as_ref()
            .is_some_and(|before| *before != snapshot.branch);
        if snapshot.head != self.head_hash {
            // The list shows another history. Labels stay as loaded too:
            // list, HEAD mark and labels are one consistent picture until
            // the reload.
            self.repo_watch.notice = Some(match (&snapshot.branch, switched) {
                (Some(branch), true) => {
                    format!("Switched to branch {branch} outside ghh. r: reload")
                }
                _ => "Repository changed outside ghh. r: reload".into(),
            });
        } else {
            self.repo_watch.notice = None;
            if snapshot.ref_labels != self.ref_labels {
                self.ref_labels = snapshot.ref_labels;
                if let (Some(branch), true) = (&snapshot.branch, switched) {
                    self.status_message = Some(format!("Now on branch {branch}."));
                }
            }
        }
        self.repo_watch.branch = Some(snapshot.branch);
        self.push_perf_metric("watch.apply", started.elapsed());
    }

    /// `r` in the commit list: load the history again, in the background.
    pub(crate) fn reload_repo(&mut self) {
        if self.full_history_required("reloading") {
            return;
        }
        self.reload_history("Reloading history...".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{FileStatus, StageState};
    use std::collections::HashMap;

    fn app(name: &str) -> App {
        let dir = std::env::temp_dir().join(format!("ghh-watch-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir, true);
        app.initial_history_loader_rx = None;
        app.history_loading = false;
        app.head_hash = Some("head1".into());
        app
    }

    fn snapshot(files: Vec<FileStatus>, stamp: u64, head: &str, branch: &str) -> git::RepoSnapshot {
        git::RepoSnapshot {
            files,
            stamp,
            head: Some(head.into()),
            branch: Some(branch.into()),
            ref_labels: HashMap::new(),
            took: Duration::from_millis(1),
        }
    }

    fn modified(path: &str) -> FileStatus {
        FileStatus {
            path: path.into(),
            stage_state: StageState::Unstaged,
            status_char: 'M',
        }
    }

    #[test]
    fn unchanged_reading_touches_nothing() {
        let mut app = app("same");
        app.apply_repo_snapshot(snapshot(vec![modified("a")], 7, "head1", "main"));
        app.working_tree.details_scroll = 3;
        app.commit_diff_cache
            .insert(WORKING_TREE_KEY.into(), vec!["patch".into()]);
        app.status_message = None;

        app.apply_repo_snapshot(snapshot(vec![modified("a")], 7, "head1", "main"));
        assert_eq!(app.working_tree.details_scroll, 3, "scroll kept");
        assert!(app.commit_diff_cache.contains_key(WORKING_TREE_KEY));
        assert!(app.repo_watch.notice.is_none() && app.status_message.is_none());
    }

    #[test]
    fn content_change_drops_the_patch_and_keeps_the_scroll() {
        let mut app = app("content");
        app.apply_repo_snapshot(snapshot(vec![modified("a")], 7, "head1", "main"));
        app.working_tree.details_scroll = 3;
        app.commit_diff_cache
            .insert(WORKING_TREE_KEY.into(), vec!["patch".into()]);

        app.apply_repo_snapshot(snapshot(vec![modified("a")], 8, "head1", "main"));
        assert!(!app.commit_diff_cache.contains_key(WORKING_TREE_KEY));
        assert_eq!(app.working_tree.details_scroll, 3);
    }

    #[test]
    fn file_list_change_updates_row_0() {
        let mut app = app("files");
        app.apply_repo_snapshot(snapshot(vec![modified("a")], 1, "head1", "main"));
        assert!(app.working_tree_row_shown());
        app.apply_repo_snapshot(snapshot(
            vec![modified("a"), modified("b")],
            2,
            "head1",
            "main",
        ));
        assert_eq!(app.working_tree.files.len(), 2);
        app.apply_repo_snapshot(snapshot(Vec::new(), 3, "head1", "main"));
        assert!(!app.working_tree_row_shown(), "clean tree: no row");
    }

    #[test]
    fn moved_head_raises_a_notice_until_head_is_back() {
        let mut app = app("head");
        app.apply_repo_snapshot(snapshot(Vec::new(), 1, "head1", "main"));
        assert!(app.repo_watch.notice.is_none());

        app.apply_repo_snapshot(snapshot(Vec::new(), 1, "head2", "main"));
        let notice = app.repo_watch.notice.clone().unwrap();
        assert!(notice.contains("changed outside") && notice.contains("r: reload"));
        assert_eq!(app.head_hash.as_deref(), Some("head1"), "list untouched");

        app.apply_repo_snapshot(snapshot(Vec::new(), 1, "head3", "topic"));
        assert!(
            app.repo_watch
                .notice
                .as_deref()
                .unwrap()
                .contains("branch topic")
        );

        app.apply_repo_snapshot(snapshot(Vec::new(), 1, "head1", "main"));
        assert!(app.repo_watch.notice.is_none(), "HEAD back: notice gone");
    }

    #[test]
    fn labels_follow_when_head_did_not_move() {
        let mut app = app("labels");
        app.apply_repo_snapshot(snapshot(Vec::new(), 1, "head1", "main"));
        let mut moved = snapshot(Vec::new(), 1, "head1", "other");
        moved.ref_labels.insert(
            "head1".into(),
            vec![git::RefLabel {
                name: "other".into(),
                kind: git::RefKind::Branch,
                is_head: true,
            }],
        );
        app.apply_repo_snapshot(moved);
        assert_eq!(app.ref_labels["head1"][0].name, "other");
        assert!(app.repo_watch.notice.is_none());
        assert_eq!(app.status_message.as_deref(), Some("Now on branch other."));
    }

    #[test]
    fn slow_reading_stretches_the_wait() {
        let two = Duration::from_secs(2);
        assert_eq!(next_wait(two, Duration::from_millis(5)), two);
        assert_eq!(
            next_wait(two, Duration::from_millis(500)),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn watch_secs_zero_turns_watching_off() {
        assert!(RepoWatchState::new(Some(0)).interval.is_none());
        assert_eq!(
            RepoWatchState::new(None).interval,
            Some(Duration::from_secs(DEFAULT_WATCH_SECS))
        );
        assert_eq!(
            RepoWatchState::new(Some(999)).interval,
            Some(Duration::from_secs(60))
        );
    }
}
