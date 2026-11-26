mod app;
mod automation;
mod events;
mod faq;
mod git;
mod settings;
mod stats;
mod tracer;
mod ui;

use automation::{AutomationCommand, load_automation_commands};
use clap::Parser;
use crossterm::{
    cursor::Show,
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
use signal_hook::flag as signal_flag;
use std::cell::Cell;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Instant;

/// Maximum number of already-buffered input events handled between two redraws.
const MAX_EVENT_BATCH: usize = 64;

/// Buffered stdout: one large write per frame instead of thousands of tiny
/// ones. Unbuffered bursts can split escape sequences across writes, which
/// macOS Terminal.app has been seen to mis-render under load (stale
/// dim/bright cells).
type Tui = Terminal<CrosstermBackend<io::BufWriter<io::Stdout>>>;

/// Main-loop silence after which the hang watchdog considers the UI stuck.
const HANG_THRESHOLD_MS: u64 = 2_000;

/// Main-loop silence after which the watchdog exits on its own, without any
/// tty input. A detached/orphaned process (no terminal to type q into) used
/// to spin at 100% CPU forever — instances were found 17 h deep.
const HANG_SELF_EXIT_MS: u64 = 5 * 60 * 1_000;

const VERSION_INFO: &str = concat!(
    "— ",
    "GIT Handy Helper",
    "\n",
    env!("CARGO_PKG_DESCRIPTION"),
    "\n",
    "Author: Arunas Ivanauskas\n",
    "Version: ",
    env!("CARGO_PKG_VERSION"),
    "\n",
    "Build hash: ",
    env!("GIT_HELPER_BUILD_HASH"),
    "\nBuild date: ",
    env!("GIT_HELPER_BUILD_DATE"),
);
const HELP_INFO: &str = concat!(
    env!("CARGO_BIN_NAME"),
    " — ",
    "GIT Handy Helper",
    "\nAuthor: Arunas Ivanauskas\nVersion: ",
    env!("CARGO_PKG_VERSION"),
    "\nBuild hash: ",
    env!("GIT_HELPER_BUILD_HASH"),
    "\nBuild date: ",
    env!("GIT_HELPER_BUILD_DATE"),
);

#[derive(Parser)]
#[command(name = env!("CARGO_BIN_NAME"))]
#[command(version = VERSION_INFO)]
#[command(about = env!("CARGO_PKG_DESCRIPTION"))]
#[command(before_help = HELP_INFO)]
struct Cli {
    /// Commit hash to modify (enables non-interactive mode)
    #[arg(long)]
    hash: Option<String>,

    /// New commit message
    #[arg(long = "mod-commit")]
    mod_commit: Option<String>,

    /// New author name
    #[arg(long = "mod-author")]
    mod_author: Option<String>,

    /// New author email
    #[arg(long = "mod-email")]
    mod_email: Option<String>,

    /// New date: "YYYY-MM-DD", "YYYY-MM-DD HH:MM", "YYYY-MM-DD HH:MM:SS", or Unix timestamp
    #[arg(long = "mod-date")]
    mod_date: Option<String>,

    /// Path to a git repository (default: current directory)
    #[arg(long, visible_alias = "repo", default_value = ".")]
    path: PathBuf,

    /// Path to a git repository, positional form: `ghh ~/repo`
    #[arg(value_name = "REPO", conflicts_with = "path")]
    repo_positional: Option<PathBuf>,

    /// Pause this many milliseconds between emitted automation keys
    #[arg(long, default_value_t = 0)]
    auto_delay: u64,

    /// Keys/commands to replay for automation.
    /// Either a comma-separated list (e.g. "ARROW_DOWN,ARROW_DOWN,ENTER,SLEEP 500")
    /// or a path to a .txt file where each line is one command:
    ///   KEY_PRESS <KEY_NAME>  — emit a key
    ///   SLEEP <ms>            — sleep N milliseconds
    ///   # ...                 — comment (ignored)
    #[arg(long)]
    auto: Option<String>,

    /// Auto-confirm destructive actions (skip confirmation dialogs).
    #[arg(short = 'y', long)]
    yes: bool,

    /// Draw the commit graph with plain ASCII glyphs (also: GHH_ASCII=1).
    /// For terminals/fonts that render the Unicode box glyphs poorly.
    #[arg(long)]
    ascii: bool,

    /// Write trace/debug log to this file (default: /tmp/ghh/logs.txt, or
    /// the settings `logs` value; "off" disables). Supports {} date tokens,
    /// e.g. logs-{YYYY-MM-DD-HH-mm-SS}.txt. A fresh file is started on each
    /// run. Tail it in a second terminal: tail -f <file>
    /// Log level is controlled by RUST_LOG (default: info).
    #[arg(long = "logs")]
    logs: Option<String>,

    /// Print runtime and application statistics on exit.
    #[arg(long)]
    stats: bool,

    /// Write a .ghh-settings.json override into the repo on exit (afterwards
    /// the existing file keeps auto-updating without this flag).
    #[arg(long = "save-local-settings")]
    save_local_settings: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Print the FAQ: questions and the key or option that answers each.
    /// Words narrow the list: `ghh faq stash`, `ghh faq move conflict`,
    /// `ghh faq 3` (a section), `ghh faq 3.7` (one row).
    Faq {
        /// Section number, row number, or words that must all appear
        words: Vec<String>,
        /// Only rows whose answer names this key, case kept: `-k E`
        #[arg(short = 'k', long = "key", value_name = "KEY")]
        key: Vec<String>,
        /// Print the rows as a JSON array
        #[arg(long)]
        json: bool,
    },
}

/// Everything that can end the program: each layer's error, plus terminal I/O.
#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Git(#[from] git::Error),
    #[error(transparent)]
    Automation(#[from] automation::Error),
}

type Result<T> = std::result::Result<T, Error>;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let process_start = Instant::now();
    // libgit2's global object cache defaults to 256 MB; walking 568k commits
    // fills it with parsed commit objects we never re-read. Cap it small
    // before any Repository opens.
    unsafe {
        let _ = libgit2_sys::git_libgit2_opts(
            libgit2_sys::GIT_OPT_SET_CACHE_MAX_SIZE as std::ffi::c_int,
            (32 * 1024 * 1024) as libc::ssize_t,
        );
    }
    let cli = Cli::parse();
    // Needs no repository and no terminal setup.
    if let Some(Command::Faq { words, key, json }) = &cli.command {
        faq::run(words, key, *json);
        return Ok(());
    }
    if cli.ascii || std::env::var_os("GHH_ASCII").is_some_and(|v| v != "0") {
        ui::helpers::ASCII_SYMBOLS.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let mut perf = cli.stats.then(|| stats::StatsCollector::new(process_start));
    let repo_path = cli.repo_positional.unwrap_or(cli.path);

    if let Some(hash) = &cli.hash {
        // Non-interactive CLI mode
        if cli.mod_commit.is_none()
            && cli.mod_author.is_none()
            && cli.mod_email.is_none()
            && cli.mod_date.is_none()
        {
            eprintln!(
                "When using --hash, provide at least one modification flag:\n  \
                 --mod-commit, --mod-author, --mod-email, --mod-date"
            );
            std::process::exit(1);
        }
        // Confirm unless -y was passed
        if !cli.yes {
            let short = &hash[..hash.len().min(7)];
            print!("Modify commit {short}? [y/N]: ");
            io::stdout().flush()?;
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            if !input.trim().eq_ignore_ascii_case("y") {
                println!("Aborted.");
                return Ok(());
            }
        }
        git::modify_commit(
            &repo_path,
            hash,
            cli.mod_commit.as_deref(),
            cli.mod_author.as_deref(),
            cli.mod_email.as_deref(),
            cli.mod_date.as_deref(),
        )?;
        let short = &hash[..hash.len().min(7)];
        println!("Commit {short} modified successfully.");
        if let Some(perf) = perf.as_ref() {
            print!("{}", perf.report());
        }
        return Ok(());
    }

    // TUI mode
    let automation_cmds = match &cli.auto {
        Some(spec) => load_automation_commands(spec)?,
        None => Vec::new(),
    };
    run_tui(
        repo_path,
        cli.auto_delay,
        automation_cmds,
        cli.yes,
        cli.save_local_settings,
        cli.logs.as_deref(),
        perf.as_mut(),
    )
}

// ─────────────────────────────────────────────────────────────────────────────

fn run_tui(
    repo_path: PathBuf,
    automation_delay: u64,
    automation_cmds: Vec<AutomationCommand>,
    yes: bool,
    save_local_settings: bool,
    log_path: Option<&str>,
    mut perf: Option<&mut stats::StatsCollector>,
) -> Result<()> {
    // Set up tracing before entering raw mode so any startup errors still print.
    // Keep the guard alive: dropping it flushes/closes the log file.
    // Log file: CLI --logs > settings `logs` > default /tmp/ghh/logs.txt;
    // "off"/"" path or enabled:false disables. File level defaults to the
    // debug screen's level; log_events adds/removes app events.
    let loaded = settings::Settings::load(&repo_path);
    let resolved = &loaded.resolved;
    let logs_config = resolved.logs_normalized();
    let debug_level = resolved
        .debug_level()
        .and_then(events::TraceLevel::from_setting);
    let file_level = file_trace_level(
        debug_level,
        logs_config
            .log_level
            .as_deref()
            .and_then(events::TraceLevel::from_setting),
    );
    let log_spec = log_path
        .map(str::to_string)
        .or(logs_config.path)
        .unwrap_or_else(|| settings::DEFAULT_LOG_PATH.into());
    let enabled = log_path.is_some() || logs_config.enabled.unwrap_or(true);
    let log_file = (enabled && !log_spec.is_empty() && log_spec != "off").then(|| {
        let canonical = repo_path
            .canonicalize()
            .unwrap_or_else(|_| repo_path.clone());
        settings::expand_log_pattern(
            &log_spec,
            &chrono::Local::now().naive_local(),
            &loaded.repo_key,
            &settings::repo_hash(&canonical.to_string_lossy()),
        )
    });
    let (tracing_guard, trace_rx) = tracer::setup_tracing(
        log_file.as_deref(),
        tracer::FileLogConfig {
            level: level_filter_of(file_level),
            log_events: logs_config.log_events.unwrap_or(true),
        },
    );
    log_startup_traces();

    let app_init_started = Instant::now();
    let mut app = app::App::new(repo_path, yes);
    app.settings.save_local |= save_local_settings;
    if let Some(perf) = perf.as_deref_mut() {
        perf.mark_app_init(app_init_started.elapsed());
        record_app_stats(&mut app, perf);
    }

    let shutdown_requested = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGTERM, SIGQUIT, SIGHUP] {
        signal_flag::register(signal, shutdown_requested.clone())?;
    }

    // A panic anywhere (draw, input, worker join) must not strand the
    // terminal in raw mode + alt screen + mouse capture. Restore first, then
    // let the default hook print the panic normally.
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(
            stdout,
            LeaveAlternateScreen,
            DisableMouseCapture,
            DisableBracketedPaste,
            Show
        );
        let _ = stdout.flush();
        default_panic(info);
    }));

    let terminal_init_started = Instant::now();
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let backend = CrosstermBackend::new(io::BufWriter::with_capacity(1 << 19, stdout));
    let mut terminal = Terminal::new(backend)?;
    let restore_guard = TerminalRestoreGuard::new();
    if let Some(perf) = perf.as_deref_mut() {
        perf.mark_terminal_init(terminal_init_started.elapsed());
    }

    let watchdog_epoch = Instant::now();
    let heartbeat = Arc::new(AtomicU64::new(0));
    spawn_hang_watchdog(
        heartbeat.clone(),
        watchdog_epoch,
        shutdown_requested.clone(),
    );

    let result = event_loop(
        &mut terminal,
        &mut app,
        automation_delay,
        automation_cmds,
        trace_rx,
        &shutdown_requested,
        &heartbeat,
        watchdog_epoch,
        perf.as_deref_mut(),
    );

    let terminal_restore_started = Instant::now();
    restore_guard.restore(&mut terminal)?;
    app.save_settings();
    tracing::info!(
        "{} v{} exiting",
        env!("CARGO_BIN_NAME"),
        env!("CARGO_PKG_VERSION")
    );
    // Dropping the guard flushes the non-blocking writer — the exiting line
    // must be on disk before the process ends.
    drop(tracing_guard);
    if let Some(perf) = perf {
        perf.mark_terminal_restore(terminal_restore_started.elapsed());
        print!("{}", perf.report());
    }

    result
}

/// One line per trace level at startup: shows in the debugger (and log file)
/// what each level looks like and which levels the active filters let
/// through. DEBUG only passes with RUST_LOG=debug; display is additionally
/// gated by the settings `debug` field.
fn log_startup_traces() {
    let app = settings::AppInfo::current();
    tracing::error!("trace-check: ERROR level visible");
    tracing::warn!("trace-check: WARN level visible");
    tracing::info!(
        "{} v{} starting (build {} {})",
        app.name,
        app.version,
        app.build_hash,
        app.build_date
    );
    tracing::debug!("trace-check: DEBUG level visible (RUST_LOG=debug)");
    tracing::trace!("trace-check: TRACE level visible (RUST_LOG=trace)");
}

// Main-loop plumbing: the args are the loop's whole world; a struct would
// only rename them.
#[allow(clippy::too_many_arguments)]
fn event_loop(
    terminal: &mut Tui,
    app: &mut app::App,
    automation_delay: u64,
    automation_cmds: Vec<AutomationCommand>,
    trace_rx: std::sync::mpsc::Receiver<tracer::ForwardedTrace>,
    shutdown_requested: &Arc<AtomicBool>,
    heartbeat: &AtomicU64,
    watchdog_epoch: Instant,
    mut perf: Option<&mut stats::StatsCollector>,
) -> Result<()> {
    // ── replay automation commands ───────────────────────────────────────────
    let mut current_delay = automation_delay;
    for cmd in automation_cmds {
        heartbeat.store(
            watchdog_epoch.elapsed().as_millis() as u64,
            Ordering::Relaxed,
        );
        if shutdown_requested.load(Ordering::Relaxed) {
            return Ok(());
        }
        let drain_started = Instant::now();
        drain_tracing_events(app, &trace_rx);
        if let Some(perf) = perf.as_deref_mut() {
            perf.record_drain_traces(drain_started.elapsed());
        }
        app.maybe_finish_history_load();
        let screen = app.current_screen();
        let draw_started = Instant::now();
        terminal.draw(|f| ui::render(f, app))?;
        if let Some(perf) = perf.as_deref_mut() {
            perf.record_draw(screen, draw_started.elapsed());
            record_app_stats(app, perf);
        }
        let automation_started = Instant::now();
        match cmd {
            AutomationCommand::KeyPress(key) => {
                let handle_started = Instant::now();
                let should_quit = app.process_key(key);
                if let Some(perf) = perf.as_deref_mut() {
                    let elapsed = handle_started.elapsed();
                    perf.record_key_handle(elapsed);
                    if let Some(action) = app.take_perf_action() {
                        perf.record_custom(action, elapsed);
                    }
                    record_app_stats(app, perf);
                }
                if should_quit {
                    if let Some(perf) = perf.as_deref_mut() {
                        perf.record_automation_step(automation_started.elapsed());
                    }
                    return Ok(());
                }
                if current_delay > 0 {
                    let screen = app.current_screen();
                    let draw_started = Instant::now();
                    terminal.draw(|f| ui::render(f, app))?;
                    if let Some(perf) = perf.as_deref_mut() {
                        perf.record_draw(screen, draw_started.elapsed());
                    }
                    let sleep_started = Instant::now();
                    std::thread::sleep(std::time::Duration::from_millis(current_delay));
                    if let Some(perf) = perf.as_deref_mut() {
                        perf.record_sleep(sleep_started.elapsed());
                    }
                }
            }
            AutomationCommand::Sleep(ms) => {
                let sleep_started = Instant::now();
                std::thread::sleep(std::time::Duration::from_millis(ms));
                if let Some(perf) = perf.as_deref_mut() {
                    perf.record_sleep(sleep_started.elapsed());
                }
            }
            AutomationCommand::KeyPressDelay(ms) => {
                current_delay = ms;
            }
        }
        if let Some(perf) = perf.as_deref_mut() {
            perf.record_automation_step(automation_started.elapsed());
        }
    }

    // ── interactive loop ────────────────────────────────────────────────────
    let mut was_popup_visible = false;
    'main_loop: loop {
        heartbeat.store(
            watchdog_epoch.elapsed().as_millis() as u64,
            Ordering::Relaxed,
        );
        if shutdown_requested.load(Ordering::Relaxed) {
            break;
        }
        // Full repaint on Ctrl+L or when a popup/dim-overlay closes: the diff
        // renderer trusts the terminal to hold exactly the previous frame, but
        // macOS Terminal.app can drop attribute runs under the huge bursts an
        // overlay toggle emits, leaving stale dim/bright cells behind.
        let popup_open = app.popup_visible();
        if std::mem::take(&mut app.force_redraw) || (was_popup_visible && !popup_open) {
            terminal.clear()?;
        }
        was_popup_visible = popup_open;
        // Stall gauge: everything between here and the input poll is the
        // compute part of one loop turn — the number the whole perf effort
        // moves. max_ms in the report is the worst UI stall of the session.
        let compute_started = Instant::now();
        if let Some(perf) = perf.as_deref_mut() {
            perf.bump_loop_iteration();
        }
        let drain_started = Instant::now();
        drain_tracing_events(app, &trace_rx);
        if let Some(perf) = perf.as_deref_mut() {
            perf.record_drain_traces(drain_started.elapsed());
        }
        app.maybe_finish_history_load();
        app.maybe_load_selected_stats();
        if let Some(perf) = perf.as_deref_mut() {
            record_app_stats(app, perf);
        }
        let screen = app.current_screen();
        let draw_started = Instant::now();
        terminal.draw(|f| ui::render(f, app))?;
        if let Some(perf) = perf.as_deref_mut() {
            perf.record_draw(screen, draw_started.elapsed());
            record_app_stats(app, perf);
        }

        if let Some(perf) = perf.as_deref_mut() {
            let compute = compute_started.elapsed();
            perf.record_custom("loop.compute", compute);
            if compute.as_millis() > 50 {
                perf.record_custom("loop.stall_over_50ms", compute);
            }
        }
        let poll_started = Instant::now();
        let has_event = event::poll(std::time::Duration::from_millis(50))?;
        if let Some(perf) = perf.as_deref_mut() {
            perf.record_poll(poll_started.elapsed());
        }
        if has_event {
            // Drain every event the terminal already has buffered before drawing
            // again: a single wheel flick or a key repeat emits a burst, and one
            // full redraw per event turns that burst into visible scroll lag.
            let mut handled = 0usize;
            loop {
                let read_started = Instant::now();
                let event = event::read()?;
                if let Some(perf) = perf.as_deref_mut() {
                    perf.record_read(read_started.elapsed());
                }
                match event {
                    Event::Key(key) => {
                        let handle_started = Instant::now();
                        let should_quit = app.process_key(key);
                        if let Some(perf) = perf.as_deref_mut() {
                            let elapsed = handle_started.elapsed();
                            perf.record_key_handle(elapsed);
                            if let Some(action) = app.take_perf_action() {
                                perf.record_custom(action, elapsed);
                            }
                            record_app_stats(app, perf);
                        }
                        if should_quit {
                            break 'main_loop;
                        }
                        if automation_delay > 0 {
                            let screen = app.current_screen();
                            let draw_started = Instant::now();
                            terminal.draw(|f| ui::render(f, app))?;
                            if let Some(perf) = perf.as_deref_mut() {
                                perf.record_draw(screen, draw_started.elapsed());
                            }
                            let sleep_started = Instant::now();
                            std::thread::sleep(std::time::Duration::from_millis(automation_delay));
                            if let Some(perf) = perf.as_deref_mut() {
                                perf.record_sleep(sleep_started.elapsed());
                            }
                            break;
                        }
                    }
                    Event::Mouse(mouse) => {
                        let handle_started = Instant::now();
                        app.handle_mouse(mouse);
                        if let Some(perf) = perf.as_deref_mut() {
                            perf.record_mouse_handle(handle_started.elapsed());
                            record_app_stats(app, perf);
                        }
                    }
                    Event::Paste(text) => {
                        let handle_started = Instant::now();
                        app.handle_paste(text);
                        if let Some(perf) = perf.as_deref_mut() {
                            perf.record_paste_handle(handle_started.elapsed());
                            record_app_stats(app, perf);
                        }
                    }
                    _ => {}
                }
                handled += 1;
                // Cap the batch so a held key can never starve the redraw.
                if handled >= MAX_EVENT_BATCH || !event::poll(std::time::Duration::ZERO)? {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// Escape hatch for a stuck UI thread: while the main loop is silent for
/// [`HANG_THRESHOLD_MS`], an external shutdown signal or a buffered q/Ctrl+C
/// keypress force-restores the terminal and exits. Never touches input while
/// the main loop is healthy. Other keys typed at a frozen UI are dropped.
fn spawn_hang_watchdog(
    heartbeat: Arc<AtomicU64>,
    epoch: Instant,
    shutdown_requested: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let stale = |heartbeat: &AtomicU64| {
            (epoch.elapsed().as_millis() as u64).saturating_sub(heartbeat.load(Ordering::Relaxed))
                >= HANG_THRESHOLD_MS
        };
        let silent_for = |heartbeat: &AtomicU64| {
            (epoch.elapsed().as_millis() as u64).saturating_sub(heartbeat.load(Ordering::Relaxed))
        };
        loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            if !stale(&heartbeat) {
                continue;
            }
            if shutdown_requested.load(Ordering::Relaxed) {
                force_restore_and_exit("shutdown signal while UI thread is unresponsive");
            }
            if silent_for(&heartbeat) >= HANG_SELF_EXIT_MS {
                force_restore_and_exit("UI thread stalled for 5 minutes — self-exit");
            }
            // The stuck main loop is not reading input, so keystrokes queue in
            // the terminal buffer; drain them looking for a quit key. The
            // staleness re-check keeps this off the main loop's input once it
            // resumes (crossterm serializes concurrent readers internally).
            while stale(&heartbeat) && event::poll(std::time::Duration::ZERO).unwrap_or(false) {
                match event::read() {
                    Ok(Event::Key(key)) => {
                        let ctrl_c = key.modifiers.contains(event::KeyModifiers::CONTROL)
                            && key.code == event::KeyCode::Char('c');
                        if ctrl_c || key.code == event::KeyCode::Char('q') {
                            force_restore_and_exit("quit key while UI thread is unresponsive");
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        }
    });
}

/// Restore the terminal from raw/alternate-screen mode and exit immediately.
/// Used only by the hang watchdog, where the normal restore path is stuck.
fn force_restore_and_exit(reason: &str) -> ! {
    let _ = disable_raw_mode();
    let mut stdout = io::stdout();
    let _ = execute!(
        stdout,
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
        Show
    );
    let _ = stdout.flush();
    eprintln!("ghh: force exit — {reason}.");
    std::process::exit(130);
}

fn record_app_stats(app: &mut app::App, stats: &mut stats::StatsCollector) {
    stats.mark_commit_counts(app.commits.len(), app.visible.len());
    for (name, duration) in app.drain_perf_metrics() {
        stats.record_custom(name, duration);
    }
}

struct TerminalRestoreGuard {
    active: Cell<bool>,
}

impl TerminalRestoreGuard {
    fn new() -> Self {
        Self {
            active: Cell::new(true),
        }
    }

    fn restore(&self, terminal: &mut Tui) -> Result<()> {
        if !self.active.replace(false) {
            return Ok(());
        }
        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            DisableBracketedPaste,
            Show
        )?;
        terminal.show_cursor()?;
        Ok(())
    }
}

impl Drop for TerminalRestoreGuard {
    fn drop(&mut self) {
        if !self.active.replace(false) {
            return;
        }
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(
            stdout,
            LeaveAlternateScreen,
            DisableMouseCapture,
            DisableBracketedPaste,
            Show
        );
        let _ = stdout.flush();
    }
}

/// Effective file log level (default INFO). The `debug` setting is the
/// verbosity CAP: one knob to quiet everything, while `logs.log_level` can
/// only restrict further per repo, never exceed the cap.
fn file_trace_level(
    debug_cap: Option<events::TraceLevel>,
    file_level: Option<events::TraceLevel>,
) -> events::TraceLevel {
    let cap = debug_cap.unwrap_or(events::TraceLevel::Info);
    file_level.unwrap_or(cap).max(cap)
}

/// Map the settings level onto a tracing-subscriber filter for the log file.
fn level_filter_of(level: events::TraceLevel) -> tracing_subscriber::filter::LevelFilter {
    use events::TraceLevel;
    use tracing_subscriber::filter::LevelFilter;
    match level {
        TraceLevel::Trace => LevelFilter::TRACE,
        TraceLevel::Debug => LevelFilter::DEBUG,
        TraceLevel::Info => LevelFilter::INFO,
        TraceLevel::Warn => LevelFilter::WARN,
        TraceLevel::Error => LevelFilter::ERROR,
    }
}

/// Map a forwarded tracing level string onto the EventBus level.
fn forwarded_trace_level(level: &str) -> events::TraceLevel {
    use events::TraceLevel;
    match level {
        "TRACE" => TraceLevel::Trace,
        "INFO" => TraceLevel::Info,
        "WARN" => TraceLevel::Warn,
        "ERROR" => TraceLevel::Error,
        _ => TraceLevel::Debug,
    }
}

/// Drain the tracing channel and push each forwarded event into the EventBus.
fn drain_tracing_events(
    app: &mut app::App,
    rx: &std::sync::mpsc::Receiver<tracer::ForwardedTrace>,
) {
    use events::{AppEvent, EventTime};
    while let Ok(t) = rx.try_recv() {
        let level = forwarded_trace_level(&t.level);
        let msg = if t.target.is_empty() {
            t.message
        } else {
            format!("[{}] {}", t.target, t.message)
        };
        app.event_bus.emit(AppEvent::Trace {
            level,
            msg,
            at: EventTime::now(),
        });
    }
}

#[cfg(test)]
mod trace_pipeline_tests {
    use super::{file_trace_level, forwarded_trace_level};
    use crate::events::TraceLevel;

    /// debug caps verbosity for the file; logs.log_level restricts further
    /// but never exceeds the cap. Defaults land on INFO.
    #[test]
    fn debug_setting_caps_file_level() {
        assert_eq!(file_trace_level(None, None), TraceLevel::Info);
        // File may be quieter than the cap…
        assert_eq!(
            file_trace_level(Some(TraceLevel::Info), Some(TraceLevel::Warn)),
            TraceLevel::Warn
        );
        // …but never louder.
        assert_eq!(
            file_trace_level(Some(TraceLevel::Info), Some(TraceLevel::Trace)),
            TraceLevel::Info
        );
        // No file override: cap applies directly.
        assert_eq!(
            file_trace_level(Some(TraceLevel::Error), None),
            TraceLevel::Error
        );
        // Cap opened up: file may follow.
        assert_eq!(
            file_trace_level(Some(TraceLevel::Trace), Some(TraceLevel::Debug)),
            TraceLevel::Debug
        );
    }

    /// Every string the tracer forwards lands on the right bus level; the
    /// unknown fallback goes to Debug.
    #[test]
    fn forwarded_levels_map_one_to_one() {
        assert_eq!(forwarded_trace_level("TRACE"), TraceLevel::Trace);
        assert_eq!(forwarded_trace_level("DEBUG"), TraceLevel::Debug);
        assert_eq!(forwarded_trace_level("INFO"), TraceLevel::Info);
        assert_eq!(forwarded_trace_level("WARN"), TraceLevel::Warn);
        assert_eq!(forwarded_trace_level("ERROR"), TraceLevel::Error);
        assert_eq!(forwarded_trace_level("whatever"), TraceLevel::Debug);
    }
}

#[cfg(test)]
mod cli_tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn repo_path_positional_flag_or_default() {
        let cli = Cli::try_parse_from(["ghh"]).unwrap();
        assert_eq!(
            cli.repo_positional.clone().unwrap_or(cli.path.clone()),
            std::path::PathBuf::from(".")
        );
        let cli = Cli::try_parse_from(["ghh", "/tmp/repo"]).unwrap();
        assert_eq!(
            cli.repo_positional.clone().unwrap_or(cli.path.clone()),
            std::path::PathBuf::from("/tmp/repo")
        );
        let cli = Cli::try_parse_from(["ghh", "--path", "/tmp/repo"]).unwrap();
        assert_eq!(
            cli.repo_positional.clone().unwrap_or(cli.path.clone()),
            std::path::PathBuf::from("/tmp/repo")
        );
        // Both at once is a user error, not a silent precedence.
        assert!(Cli::try_parse_from(["ghh", "/a", "--path", "/b"]).is_err());
    }

    /// `faq` is a subcommand; a directory of that name is still reachable
    /// as `./faq` or `--path faq`.
    #[test]
    fn faq_is_a_subcommand_beside_the_repo_argument() {
        use super::Command;
        let cli = Cli::try_parse_from(["ghh", "faq"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Faq { .. })));
        assert!(cli.repo_positional.is_none());

        let cli = Cli::try_parse_from(["ghh", "faq", "stash", "pop", "-k", "E", "--json"]).unwrap();
        let Some(Command::Faq { words, key, json }) = cli.command else {
            panic!("not parsed as faq");
        };
        assert_eq!(
            (words, key, json),
            (
                vec!["stash".to_string(), "pop".to_string()],
                vec!["E".to_string()],
                true
            )
        );

        for args in [vec!["ghh", "./faq"], vec!["ghh", "--path", "faq"]] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert!(cli.command.is_none());
            assert!(cli.repo_positional.unwrap_or(cli.path).ends_with("faq"));
        }
        assert!(
            Cli::try_parse_from(["ghh", "/tmp/repo"])
                .unwrap()
                .command
                .is_none()
        );
    }
}
