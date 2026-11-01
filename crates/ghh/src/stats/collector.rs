//! [`StatsCollector`]: gathers startup, event-loop and per-screen timing
//! metrics. Report formatting lives in [`super::report`].

use super::metric::Metric;
use crate::events::ScreenId;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub struct StatsCollector {
    pub(super) started_at: Instant,
    pub(super) startup_total: Option<Duration>,
    pub(super) app_init: Option<Duration>,
    pub(super) terminal_init: Option<Duration>,
    pub(super) terminal_restore: Option<Duration>,
    pub(super) commit_total_count: Option<usize>,
    pub(super) commit_visible_count: Option<usize>,
    pub(super) draw_total: Metric,
    pub(super) draw_by_screen: BTreeMap<&'static str, Metric>,
    pub(super) poll_total: Metric,
    pub(super) read_total: Metric,
    pub(super) drain_traces_total: Metric,
    pub(super) key_handle_total: Metric,
    pub(super) mouse_handle_total: Metric,
    pub(super) paste_handle_total: Metric,
    pub(super) automation_step_total: Metric,
    pub(super) sleep_total: Metric,
    pub(super) custom_metrics: BTreeMap<String, Metric>,
    pub(super) loop_iterations: u64,
}

impl StatsCollector {
    pub fn new(started_at: Instant) -> Self {
        Self {
            started_at,
            startup_total: None,
            app_init: None,
            terminal_init: None,
            terminal_restore: None,
            commit_total_count: None,
            commit_visible_count: None,
            draw_total: Metric::default(),
            draw_by_screen: BTreeMap::new(),
            poll_total: Metric::default(),
            read_total: Metric::default(),
            drain_traces_total: Metric::default(),
            key_handle_total: Metric::default(),
            mouse_handle_total: Metric::default(),
            paste_handle_total: Metric::default(),
            automation_step_total: Metric::default(),
            sleep_total: Metric::default(),
            custom_metrics: BTreeMap::new(),
            loop_iterations: 0,
        }
    }

    pub fn mark_app_init(&mut self, duration: Duration) {
        self.app_init = Some(duration);
    }

    pub fn mark_terminal_init(&mut self, duration: Duration) {
        self.terminal_init = Some(duration);
        self.startup_total = Some(self.started_at.elapsed());
    }

    pub fn mark_terminal_restore(&mut self, duration: Duration) {
        self.terminal_restore = Some(duration);
    }

    pub fn mark_commit_counts(&mut self, total_count: usize, visible_count: usize) {
        self.commit_total_count = Some(total_count);
        self.commit_visible_count = Some(visible_count);
    }

    pub fn record_draw(&mut self, screen: ScreenId, duration: Duration) {
        self.draw_total.record(duration);
        self.draw_by_screen
            .entry(screen.as_str())
            .or_default()
            .record(duration);
    }

    pub fn record_poll(&mut self, duration: Duration) {
        self.poll_total.record(duration);
    }

    pub fn record_read(&mut self, duration: Duration) {
        self.read_total.record(duration);
    }

    pub fn record_drain_traces(&mut self, duration: Duration) {
        self.drain_traces_total.record(duration);
    }

    pub fn record_key_handle(&mut self, duration: Duration) {
        self.key_handle_total.record(duration);
    }

    pub fn record_mouse_handle(&mut self, duration: Duration) {
        self.mouse_handle_total.record(duration);
    }

    pub fn record_paste_handle(&mut self, duration: Duration) {
        self.paste_handle_total.record(duration);
    }

    pub fn record_automation_step(&mut self, duration: Duration) {
        self.automation_step_total.record(duration);
    }

    pub fn record_sleep(&mut self, duration: Duration) {
        self.sleep_total.record(duration);
    }

    pub fn record_custom(&mut self, name: impl Into<String>, duration: Duration) {
        self.custom_metrics
            .entry(name.into())
            .or_default()
            .record(duration);
    }

    pub fn bump_loop_iteration(&mut self) {
        self.loop_iterations += 1;
    }
}
