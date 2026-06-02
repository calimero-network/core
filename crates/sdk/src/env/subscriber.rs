//! Host-backed `tracing` subscriber for Calimero WASM applications.
//!
//! `tracing` is a facade: its macros forward each event to whatever global
//! subscriber is installed, and when none is, the events are dropped before
//! they are even formatted. WASM guests never installed one, so
//! `tracing::info!`/`debug!`/… — including calls inside crates the app
//! imports — produced no output and could not be extracted.
//!
//! This module installs a subscriber whose writer forwards every formatted
//! line through [`crate::env::log`] to the host, so `tracing` output lands in
//! the execution outcome alongside `app::log!`. The level is held in a global
//! atomic and re-read on every event, so [`set_log_level`] retunes verbosity
//! at runtime even after the subscriber is installed.
//!
//! Gated behind the `tracing` cargo feature so apps that don't want the
//! dependency (and the binary-size cost) pay nothing.

use core::sync::atomic::{AtomicU8, Ordering};
use std::io::{self, Write};
use std::sync::Once;

use tracing::level_filters::LevelFilter;
use tracing::Level;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{Layer, Registry};

/// Maximum enabled log level, encoded as a small ordinal (0 = OFF … 5 = TRACE)
/// so it lives in a single relaxed atomic. Read on every event by the filter,
/// which is why the level can change after the subscriber is already global.
///
/// Defaults to WARN, not INFO: dependency crates compiled into the guest (most
/// notably `calimero_storage`) log routine operations at INFO, so an INFO
/// default would flood every execution's log buffer with internals. WARN
/// surfaces warnings/errors out of the box; apps opt into INFO/DEBUG via
/// [`set_log_level`] when they actually want the detail (e.g. debugging a
/// storage divergence).
static MAX_LEVEL: AtomicU8 = AtomicU8::new(LEVEL_WARN);

const LEVEL_OFF: u8 = 0;
const LEVEL_ERROR: u8 = 1;
const LEVEL_WARN: u8 = 2;
const LEVEL_INFO: u8 = 3;
const LEVEL_DEBUG: u8 = 4;
const LEVEL_TRACE: u8 = 5;

/// Installs the subscriber once per WASM instance. Re-attempting is cheap and
/// harmless: `set_global_default` would reject a second subscriber anyway, and
/// `Once` saves rebuilding one on every method call within the same instance.
static INIT: Once = Once::new();

fn level_filter_to_u8(level: LevelFilter) -> u8 {
    match level.into_level() {
        None => LEVEL_OFF,
        Some(Level::ERROR) => LEVEL_ERROR,
        Some(Level::WARN) => LEVEL_WARN,
        Some(Level::INFO) => LEVEL_INFO,
        Some(Level::DEBUG) => LEVEL_DEBUG,
        Some(Level::TRACE) => LEVEL_TRACE,
    }
}

fn current_max() -> LevelFilter {
    match MAX_LEVEL.load(Ordering::Relaxed) {
        LEVEL_OFF => LevelFilter::OFF,
        LEVEL_ERROR => LevelFilter::ERROR,
        LEVEL_WARN => LevelFilter::WARN,
        LEVEL_INFO => LevelFilter::INFO,
        LEVEL_DEBUG => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    }
}

/// Sets the maximum `tracing` level forwarded to the host. Takes effect
/// immediately, including for an already-installed subscriber, since the
/// filter re-reads the level on each event. Pass [`LevelFilter::OFF`] to
/// silence `tracing` output entirely.
///
/// Call this from your app's initializer (or a dedicated method) to control
/// verbosity, e.g. `env::set_log_level(LevelFilter::DEBUG)`.
pub fn set_log_level(level: LevelFilter) {
    MAX_LEVEL.store(level_filter_to_u8(level), Ordering::Relaxed);
}

/// A `Write` sink that buffers one event's bytes and, on drop (end of the
/// event), forwards them to the host as a single log line. `tracing`'s fmt
/// layer calls `make_writer` once per event, writes the formatted record, then
/// drops the writer — so one writer lifetime maps to exactly one host log.
struct HostWriter {
    buf: Vec<u8>,
}

impl Write for HostWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for HostWriter {
    fn drop(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        // The host frames each log entry itself, so drop the trailing newline
        // the fmt layer appends. `from_utf8_lossy` guards against a partial
        // multi-byte sequence (shouldn't happen, but must never panic here).
        let line = String::from_utf8_lossy(&self.buf);
        crate::env::log(line.trim_end_matches('\n'));
    }
}

struct HostMakeWriter;

impl MakeWriter<'_> for HostMakeWriter {
    type Writer = HostWriter;

    fn make_writer(&self) -> Self::Writer {
        HostWriter { buf: Vec::new() }
    }
}

/// Installs the host-backed subscriber as the global default, once.
///
/// Called automatically at the entry of every generated WASM export (next to
/// `setup_panic_hook`), so apps get `tracing` output without any setup. Safe to
/// call repeatedly.
pub fn init() {
    INIT.call_once(|| {
        // `without_time`: the default fmt timer calls `SystemTime::now()`,
        // which traps on `wasm32-unknown-unknown`. `with_ansi(false)`: no
        // colour escapes in plain-text host logs. The level filter reads the
        // global atomic per event so `set_log_level` stays effective.
        let layer = tracing_subscriber::fmt::layer()
            .with_writer(HostMakeWriter)
            .without_time()
            .with_ansi(false)
            .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
                meta.level() <= &current_max()
            }));

        let subscriber = Registry::default().with(layer);
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
}

#[cfg(test)]
mod tests {
    use tracing::level_filters::LevelFilter;
    use tracing::{debug, info, warn};
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::{Layer, Registry};

    use super::{current_max, set_log_level, HostMakeWriter};
    use crate::env::host;

    /// `MAX_LEVEL` is a process-global atomic, so these tests must not run
    /// concurrently or one's `set_log_level` would perturb another's filter.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Builds a fresh subscriber identical to the installed one. Tests use a
    /// thread-local default (`with_default`) rather than the process-global
    /// default so they don't collide when run in parallel, and they read the
    /// host mock's captured logs directly (no full `TestHost` app needed).
    fn test_subscriber() -> impl tracing::Subscriber + Send + Sync {
        let layer = tracing_subscriber::fmt::layer()
            .with_writer(HostMakeWriter)
            .without_time()
            .with_ansi(false)
            .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
                meta.level() <= &current_max()
            }));
        Registry::default().with(layer)
    }

    #[test]
    fn forwards_events_to_host_with_level() {
        let _guard = TEST_LOCK.lock().unwrap();
        host::reset();
        set_log_level(LevelFilter::INFO);

        tracing::subscriber::with_default(test_subscriber(), || {
            info!(item = 7, "processing");
        });

        let logs = host::logs();
        assert_eq!(logs.len(), 1, "exactly one host log line per event");
        assert!(logs[0].contains("INFO"), "level is rendered: {:?}", logs[0]);
        assert!(logs[0].contains("processing"), "message body present");
        assert!(logs[0].contains("item=7"), "structured field present");
        assert!(!logs[0].ends_with('\n'), "trailing newline trimmed");
    }

    #[test]
    fn level_filters_below_threshold() {
        let _guard = TEST_LOCK.lock().unwrap();
        host::reset();
        set_log_level(LevelFilter::INFO);

        tracing::subscriber::with_default(test_subscriber(), || {
            debug!("dropped at INFO");
            warn!("kept at INFO");
        });

        let logs = host::logs();
        assert_eq!(logs.len(), 1, "debug dropped, warn kept");
        assert!(logs[0].contains("kept at INFO"));
    }

    #[test]
    fn level_change_takes_effect_immediately() {
        let _guard = TEST_LOCK.lock().unwrap();
        host::reset();

        tracing::subscriber::with_default(test_subscriber(), || {
            set_log_level(LevelFilter::INFO);
            debug!("first, dropped");
            set_log_level(LevelFilter::DEBUG);
            debug!("second, kept");
        });

        let logs = host::logs();
        assert_eq!(logs.len(), 1);
        assert!(logs[0].contains("second, kept"));
    }
}
