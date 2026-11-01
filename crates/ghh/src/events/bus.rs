use crate::events::models::{AppEvent, EventTime, TraceLevel};
use std::collections::VecDeque;
use std::time::Instant;

/// Side-effect handler — implement this to write events to a file, network, etc.
#[allow(dead_code)]
pub trait EventHandler {
    fn handle(&mut self, event: &AppEvent);
}

/// Central event bus: dispatches to handlers and keeps an in-memory ring-buffer log.
pub struct EventBus {
    log: VecDeque<AppEvent>,
    capacity: usize,
    handlers: Vec<Box<dyn EventHandler>>,
    #[allow(dead_code)]
    started_at: Instant,
    /// First visible row index (used by the debugger screen).
    pub scroll: usize,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        Self {
            log: VecDeque::with_capacity(capacity),
            capacity,
            handlers: Vec::new(),
            started_at: Instant::now(),
            scroll: 0,
        }
    }

    /// Register a side-effect handler (file logger, etc.).
    #[allow(dead_code)]
    pub fn add_handler(&mut self, handler: Box<dyn EventHandler>) {
        self.handlers.push(handler);
    }

    /// Emit an event: log to tracing (→ log file), fan out to handlers, append to ring buffer.
    ///
    /// Uses `target: "events::bus"` for tracing calls so that `EventBusLayer`
    /// can filter them out and break the recursion cycle.
    pub fn emit(&mut self, event: AppEvent) {
        // Log every event via tracing so it appears in the log file with a
        // consistent format alongside other tracing output.
        match &event {
            AppEvent::KeyPress {
                display: key_str,
                screen,
                ..
            } => tracing::debug!(target: "events::bus",
                    screen = screen.as_str(), key = key_str.as_str(), "event: key-press"),
            AppEvent::ScreenChange { from, to, .. } => tracing::info!(target: "events::bus",
                    from = from.as_str(), to = to.as_str(), "event: screen-change"),
            AppEvent::Action { kind, screen, .. } => tracing::info!(target: "events::bus",
                    screen = screen.as_str(), action = %kind.as_str(), "event: action"),
            AppEvent::Trace { level, msg, .. } => tracing::debug!(target: "events::bus",
                    level = level.as_str(), message = msg.as_str(), "event: trace"),
        }

        for h in &mut self.handlers {
            h.handle(&event);
        }
        if self.log.len() == self.capacity {
            self.log.pop_front();
            self.scroll = self.scroll.saturating_sub(1);
        }
        self.log.push_back(event);
    }

    /// Convenience: emit a Trace event at Debug level.
    #[allow(dead_code)]
    pub fn trace(&mut self, msg: impl Into<String>) {
        self.emit(AppEvent::Trace {
            level: TraceLevel::Debug,
            msg: msg.into(),
            at: EventTime::now(),
        });
    }

    #[allow(dead_code)]
    pub fn trace_level(&mut self, level: TraceLevel, msg: impl Into<String>) {
        self.emit(AppEvent::Trace {
            level,
            msg: msg.into(),
            at: EventTime::now(),
        });
    }

    pub fn events(&self) -> &VecDeque<AppEvent> {
        &self.log
    }

    /// Milliseconds elapsed since the bus was created.
    #[allow(dead_code)]
    pub fn elapsed_ms(&self, at: &EventTime) -> u64 {
        at.instant.duration_since(self.started_at).as_millis() as u64
    }

    pub fn clear(&mut self) {
        self.log.clear();
        self.scroll = 0;
    }

    // ── scroll helpers ────────────────────────────────────────────────────
    // `scroll` = rows back from the bottom of the log.
    // 0  → newest events visible at the bottom.
    // N  → N rows scrolled back toward older events.
    // Renderer computes: start = total.saturating_sub(list_height + scroll)

    pub fn scroll_down(&mut self) {
        // Move toward newer events.
        self.scroll = self.scroll.saturating_sub(1);
    }

    pub fn scroll_up(&mut self) {
        // Move toward older events (no hard cap — renderer clamps via saturating_sub).
        self.scroll = self.scroll.saturating_add(1);
    }

    pub fn scroll_to_start(&mut self) {
        // Jump to oldest events (top). Use a large sentinel; renderer clamps.
        self.scroll = usize::MAX / 2;
    }

    /// Jump to the newest events (bottom).
    pub fn jump_to_end(&mut self) {
        self.scroll = 0;
    }
}
