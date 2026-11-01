use std::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

// ── Forwarded event ───────────────────────────────────────────────────────────

/// A tracing event forwarded to the EventBus for display in the debugger screen.
pub struct ForwardedTrace {
    pub level: String,  // "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR"
    pub target: String, // module path, e.g. "git_helper::app::edit"
    pub message: String,
}

// ── Custom Layer ──────────────────────────────────────────────────────────────

/// Tracing `Layer` that forwards events to the EventBus via an mpsc channel.
pub struct EventBusLayer {
    tx: mpsc::SyncSender<ForwardedTrace>,
}

impl EventBusLayer {
    pub fn new(tx: mpsc::SyncSender<ForwardedTrace>) -> Self {
        Self { tx }
    }
}

struct MsgVisitor {
    message: String,
}

impl Visit for MsgVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            // `fmt::Arguments` implements Debug via Display, so no extra quotes.
            self.message = format!("{value:?}");
        }
    }
}

impl<S: Subscriber> Layer<S> for EventBusLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        // Skip events emitted by EventBus itself (target "events::bus") to
        // prevent the recursion cycle: emit() → tracing → EventBusLayer → emit().
        if meta.target() == "events::bus" {
            return;
        }
        let mut v = MsgVisitor {
            message: String::new(),
        };
        event.record(&mut v);
        let _ = self.tx.try_send(ForwardedTrace {
            level: meta.level().to_string(),
            target: meta.target().to_string(),
            message: v.message,
        });
    }
}

// ── Subscriber setup ──────────────────────────────────────────────────────────

/// Initialise the global tracing subscriber.
///
/// - If `log_path` is `Some(path)`, events are written to that file (non-blocking)
///   and can be tailed with `tail -f <path>` in a second terminal.
/// - If `log_path` is `None`, file logging is skipped.
///
/// Returns:
/// - `Option<WorkerGuard>` — present only when a log file is active; keep it
///   alive for the duration of the program so the file writer isn't dropped.
/// - `Receiver<ForwardedTrace>` — drain each event-loop iteration to push
///   tracing events into the `EventBus` (visible in the debugger screen).
///
/// What the log file gets: a minimum level for app traces plus an
/// independent switch for app events (target "events::bus").
pub struct FileLogConfig {
    pub level: tracing_subscriber::filter::LevelFilter,
    pub log_events: bool,
}

/// The file layer's per-event decision — pure so tests can pin it.
/// Event lines (target "events::bus") ride the `log_events` switch alone;
/// everything else passes the level gate.
pub(crate) fn file_log_allows(
    target: &str,
    level: &tracing::Level,
    config_level: tracing_subscriber::filter::LevelFilter,
    log_events: bool,
) -> bool {
    if target == "events::bus" {
        return log_events;
    }
    config_level >= *level
}

/// Global capture: RUST_LOG when set, otherwise everything — the debugger
/// and file layers apply their own configured gates downstream.
pub fn setup_tracing(
    log_path: Option<&str>,
    file_config: FileLogConfig,
) -> (
    Option<WorkerGuard>,
    std::sync::mpsc::Receiver<ForwardedTrace>,
) {
    use tracing_subscriber::{
        EnvFilter, filter::filter_fn, layer::SubscriberExt, util::SubscriberInitExt,
    };

    let (tx, rx) = std::sync::mpsc::sync_channel::<ForwardedTrace>(1000);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("trace"));

    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(EventBusLayer::new(tx));

    if let Some(path) = log_path {
        let path = std::path::Path::new(path);
        let dir = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => std::path::Path::new("."),
        };
        let _ = std::fs::create_dir_all(dir);
        // Each start begins a fresh log file (matters for static names like
        // the default logs.txt; dated patterns rotate by name anyway).
        let _ = std::fs::write(path, "");
        let file_name = path
            .file_name()
            .map(|name| name.to_owned())
            .unwrap_or_else(|| "logs.txt".into());
        let file_appender = tracing_appender::rolling::never(dir, file_name);
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        let FileLogConfig { level, log_events } = file_config;
        registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(non_blocking)
                    .with_ansi(false)
                    .with_target(true)
                    .with_level(true)
                    .with_filter(filter_fn(move |meta| {
                        file_log_allows(meta.target(), meta.level(), level, log_events)
                    })),
            )
            .init();
        (Some(guard), rx)
    } else {
        registry.init();
        (None, rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::{EnvFilter, layer::SubscriberExt};

    /// The whole capture side of the pipeline: every tracing level passes an
    /// all-permissive filter and arrives on the channel with the level string
    /// `drain_tracing_events` expects. Uses a scoped subscriber, not the
    /// global one, so tests stay independent.
    #[test]
    fn all_five_levels_forward_through_the_layer() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<ForwardedTrace>(100);
        // Uppercase directive: RUST_LOG=TRACE must work like trace.
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("TRACE"))
            .with(EventBusLayer::new(tx));
        tracing::subscriber::with_default(subscriber, || {
            tracing::error!("e");
            tracing::warn!("w");
            tracing::info!("i");
            tracing::debug!("d");
            tracing::trace!("t");
        });
        let got: Vec<(String, String)> =
            rx.try_iter().map(|t| (t.level, t.message)).collect();
        assert_eq!(
            got,
            vec![
                ("ERROR".into(), "e".into()),
                ("WARN".into(), "w".into()),
                ("INFO".into(), "i".into()),
                ("DEBUG".into(), "d".into()),
                ("TRACE".into(), "t".into()),
            ]
        );
    }

    /// The default filter ("info") drops debug/trace at the capture gate —
    /// this is why RUST_LOG=debug/trace is needed to see DBG/TRC.
    #[test]
    fn default_info_filter_drops_debug_and_trace() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<ForwardedTrace>(100);
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("info"))
            .with(EventBusLayer::new(tx));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("i");
            tracing::debug!("d");
            tracing::trace!("t");
        });
        let levels: Vec<String> = rx.try_iter().map(|t| t.level).collect();
        assert_eq!(levels, vec!["INFO".to_string()]);
    }

    /// File gate: events ride the log_events switch regardless of level;
    /// app traces pass the configured level only.
    #[test]
    fn file_gate_splits_events_from_levels() {
        use super::file_log_allows;
        use tracing::Level;
        use tracing_subscriber::filter::LevelFilter;
        // Events: only the switch matters (bus logs them at DEBUG).
        assert!(file_log_allows("events::bus", &Level::DEBUG, LevelFilter::ERROR, true));
        assert!(!file_log_allows("events::bus", &Level::ERROR, LevelFilter::TRACE, false));
        // App traces: level gate, independent of the events switch.
        assert!(file_log_allows("app", &Level::INFO, LevelFilter::INFO, false));
        assert!(!file_log_allows("app", &Level::DEBUG, LevelFilter::INFO, true));
        assert!(file_log_allows("app", &Level::TRACE, LevelFilter::TRACE, false));
    }

    /// events::bus target must be skipped — it is the recursion guard.
    #[test]
    fn event_bus_target_is_not_forwarded() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<ForwardedTrace>(100);
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("trace"))
            .with(EventBusLayer::new(tx));
        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!(target: "events::bus", "loop");
            tracing::debug!("kept");
        });
        let messages: Vec<String> = rx.try_iter().map(|t| t.message).collect();
        assert_eq!(messages, vec!["kept".to_string()]);
    }
}
