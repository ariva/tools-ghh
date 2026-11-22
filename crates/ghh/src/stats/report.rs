//! Text report formatting for [`StatsCollector`].

use super::collector::StatsCollector;
use super::metric::Metric;
use std::fmt::Write as _;

impl StatsCollector {
    pub fn report(&self) -> String {
        let mut out = String::new();
        let total_runtime = self.started_at.elapsed();
        let _ = writeln!(out, "=========  STATS ==========");
        let _ = writeln!(
            out,
            "runtime.total_ms: {:.3}",
            total_runtime.as_secs_f64() * 1000.0
        );
        if let Some(startup_total) = self.startup_total {
            let _ = writeln!(
                out,
                "startup.total_ms: {:.3}",
                startup_total.as_secs_f64() * 1000.0
            );
        }
        if let Some(app_init) = self.app_init {
            let _ = writeln!(
                out,
                "startup.app_init_ms: {:.3}",
                app_init.as_secs_f64() * 1000.0
            );
        }
        if let Some(terminal_init) = self.terminal_init {
            let _ = writeln!(
                out,
                "startup.terminal_init_ms: {:.3}",
                terminal_init.as_secs_f64() * 1000.0
            );
        }
        if let Some(terminal_restore) = self.terminal_restore {
            let _ = writeln!(
                out,
                "exit.terminal_restore_ms: {:.3}",
                terminal_restore.as_secs_f64() * 1000.0
            );
        }
        if let Some(total_count) = self.commit_total_count {
            let _ = writeln!(out, "commits.total_count: {}", total_count);
        }
        if let Some(visible_count) = self.commit_visible_count {
            let _ = writeln!(out, "commits.visible_count: {}", visible_count);
        }

        self.write_metric(&mut out, "render.all", &self.draw_total);
        for (screen, metric) in &self.draw_by_screen {
            self.write_metric(&mut out, &format!("render.screen.{screen}"), metric);
        }
        self.write_metric(&mut out, "events.poll", &self.poll_total);
        self.write_metric(&mut out, "events.read", &self.read_total);
        self.write_metric(&mut out, "events.drain_traces", &self.drain_traces_total);
        self.write_metric(&mut out, "handle.key", &self.key_handle_total);
        self.write_metric(&mut out, "handle.mouse", &self.mouse_handle_total);
        self.write_metric(&mut out, "handle.paste", &self.paste_handle_total);
        self.write_metric(&mut out, "automation.step", &self.automation_step_total);
        self.write_metric(&mut out, "sleep", &self.sleep_total);
        for (name, metric) in &self.custom_metrics {
            self.write_metric(&mut out, name, metric);
        }
        self.write_problem_areas(&mut out);
        let _ = writeln!(out, "loop.iterations: {}", self.loop_iterations);
        out
    }

    /// Work metrics whose worst case blocked the UI thread noticeably.
    /// Idle waits (poll/read/sleep) are excluded — long maxima there are by
    /// design, not stalls.
    fn write_problem_areas(&self, out: &mut String) {
        const STALL_MS: f64 = 100.0;
        let mut candidates: Vec<(String, &Metric)> = vec![
            ("render.all".into(), &self.draw_total),
            ("events.drain_traces".into(), &self.drain_traces_total),
            ("handle.key".into(), &self.key_handle_total),
            ("handle.mouse".into(), &self.mouse_handle_total),
            ("handle.paste".into(), &self.paste_handle_total),
            ("automation.step".into(), &self.automation_step_total),
        ];
        for (screen, metric) in &self.draw_by_screen {
            candidates.push((format!("render.screen.{screen}"), metric));
        }
        for (name, metric) in &self.custom_metrics {
            candidates.push((name.clone(), metric));
        }
        let mut problems: Vec<(String, &Metric)> = candidates
            .into_iter()
            .filter(|(_, m)| m.max_ms() >= STALL_MS)
            .collect();
        problems.sort_by(|a, b| b.1.max_ms().total_cmp(&a.1.max_ms()));
        let _ = writeln!(
            out,
            "=========  PROBLEM AREAS (max >= {STALL_MS:.0}ms) =========="
        );
        if problems.is_empty() {
            let _ = writeln!(out, "none");
        } else {
            for (name, metric) in problems {
                self.write_metric(out, &name, metric);
            }
        }
    }

    fn write_metric(&self, out: &mut String, name: &str, metric: &Metric) {
        let _ = writeln!(
            out,
            "{name}: count={} avg_ms={:.3} max_ms={:.3} total_ms={:.3}",
            metric.count,
            metric.avg_ms(),
            metric.max_ms(),
            metric.total_ms()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::StatsCollector;
    use std::time::{Duration, Instant};

    #[test]
    fn problem_areas_list_stalls_and_skip_fast_metrics() {
        let mut stats = StatsCollector::new(Instant::now());
        stats.record_custom("drill.slow", Duration::from_millis(150));
        stats.record_custom("drill.fast", Duration::from_millis(5));
        let report = stats.report();
        let problems = report.split("PROBLEM AREAS").nth(1).unwrap();
        assert!(problems.contains("drill.slow"));
        assert!(!problems.contains("drill.fast"));
    }

    #[test]
    fn problem_areas_report_none_when_all_fast() {
        let mut stats = StatsCollector::new(Instant::now());
        stats.record_custom("quick", Duration::from_millis(2));
        let report = stats.report();
        assert!(
            report
                .split("PROBLEM AREAS")
                .nth(1)
                .unwrap()
                .contains("none")
        );
    }
}
