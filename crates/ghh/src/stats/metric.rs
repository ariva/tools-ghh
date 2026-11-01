//! Single duration metric: call count, running total and max.

use std::time::Duration;

#[derive(Clone, Default)]
pub(super) struct Metric {
    pub(super) count: u64,
    pub(super) total: Duration,
    pub(super) max: Duration,
}

impl Metric {
    pub(super) fn record(&mut self, duration: Duration) {
        self.count += 1;
        self.total += duration;
        self.max = self.max.max(duration);
    }

    pub(super) fn avg_ms(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.total.as_secs_f64() * 1000.0 / self.count as f64
        }
    }

    pub(super) fn total_ms(&self) -> f64 {
        self.total.as_secs_f64() * 1000.0
    }

    pub(super) fn max_ms(&self) -> f64 {
        self.max.as_secs_f64() * 1000.0
    }
}
