use std::backtrace::Backtrace;
use std::env::var;
use std::panic::{set_hook, take_hook};

use calimero_utils_actix::init_global_runtime;
use clap::Parser;
use eyre::Result as EyreResult;
use tracing_subscriber::fmt::layer;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{registry, EnvFilter};

mod cli;
mod defaults;
mod kms;
mod kms_policy;
mod version;
mod watchdog;

use cli::RootCommand;

#[tokio::main]
async fn main() -> EyreResult<()> {
    // Used by integration tests to verify panic hook logs structured info without panicking in-process.
    match std::env::var("MEROD_TEST_PANIC").as_deref() {
        Ok("1") => {
            setup()?;
            panic!("test panic message");
        }
        Ok("string") => {
            setup()?;
            std::panic::panic_any(String::from("string payload panic"));
        }
        _ => {}
    }

    setup()?;

    let command = RootCommand::parse();

    version::check_for_update();

    command.run().await
}

/// The filter used when `RUST_LOG` says nothing.
///
/// `mero_auth` is included so embedded-auth startup messages (notably the
/// how-to-provision notice on an un-provisioned node) are visible by default.
const DEFAULT_DIRECTIVES: &str = "merod=info,calimero_=info,mero_auth=info";

/// Targets turned down ahead of every filter, so that a blanket `RUST_LOG=debug`
/// still produces a log a human can read.
///
/// Measured over both node logs of one healthy e2e run (4.87 MB): together these
/// three are 40.8% of the bytes, and none of them describe anything the node did.
///
/// - `cranelift_codegen` (31.2%, 7,808 lines) — the wasm compiler narrating its
///   own optimisation passes, function by function; compiling one app emits
///   ~7,800 lines in ~400ms. Volume is not the worst of it. The burst lands
///   *during* a compile, so it is what the log consists of at exactly the moment
///   a node that dies inside `create_context` has to be explained from its log
///   alone.
/// - `multistream_select` (6.2%, 896 lines) — protocol negotiation announcing
///   every success four times over (proposed, confirming, sent confirmed,
///   received confirmation) for protocols that were never in doubt. A
///   negotiation that genuinely fails still surfaces as a swarm-level upgrade or
///   connection error, which is not touched here.
/// - `libp2p_core::transport::choice` (3.3%, 18 lines at ~9 KB each) — which
///   transport in the list did not handle an address, spelling out the entire
///   monomorphised transport type to say so. Trying each transport in turn is how
///   that transport works, so this is routine. Scoped to the one module: its
///   sibling `libp2p_core::upgrade` reports real TLS/noise upgrade failures.
///
/// `warn` rather than `off`: none of the three has said anything at warn or above
/// in 6.6 MB of captured logs, so the entire flood sits below that line and
/// leaving the level open costs nothing if one of them ever has something real to
/// report.
///
/// A target-specific directive beats a blanket level regardless of order, which
/// is what lets `RUST_LOG=debug` keep everything else. Naming one of these
/// targets explicitly drops our entry for it entirely — see `log_directives`,
/// which will not emit two directives for one target.
///
/// Deliberately NOT included, despite being the next largest: `libp2p_gossipsub`
/// (12.1%) is mesh formation — `HEARTBEAT: Mesh low`, `Updating mesh`, peer
/// counts — which is the evidence mesh-join and mesh-scoring bugs are diagnosed
/// from, and `libp2p_swarm`'s connection-closed lines are how a peer's death is
/// spotted at all.
const QUIET_TARGETS: [&str; 3] = [
    "cranelift_codegen",
    "multistream_select",
    "libp2p_core::transport::choice",
];

/// The level the quieted targets are held at. `warn` rather than `off` so a
/// genuine complaint from one of them still arrives.
const QUIET_LEVEL: &str = "warn";

/// The target a single `RUST_LOG` directive applies to, or `None` for a bare
/// level like `debug`.
///
/// Only the target is needed, so this stops at the first `=` and drops any
/// `[span{field}]` part rather than trying to model the whole grammar.
fn directive_target(directive: &str) -> Option<&str> {
    let head = directive.split('=').next().unwrap_or("").trim();
    let head = head.split('[').next().unwrap_or("").trim();

    // A bare level is not a target, and is exactly the case the quieting exists
    // for — it must not count as the operator having asked about these targets.
    if head.is_empty() || head.parse::<tracing::Level>().is_ok() || head.eq_ignore_ascii_case("off")
    {
        return None;
    }

    Some(head)
}

/// Builds the tracing filter from `RUST_LOG`, or the default when it is unset
/// or blank.
///
/// A quieted target is dropped from the prefix when `RUST_LOG` names that exact
/// target, so the result never carries two directives for one target. That is
/// not a tidiness point: which of two same-target directives wins is not
/// something `EnvFilter` documents, and depending on it made
/// `RUST_LOG=cranelift_codegen=debug` work on one platform and silently not on
/// another. With one directive per target there is nothing to resolve.
///
/// A *more specific* target is left alone — `cranelift_codegen::context=debug`
/// alongside our `cranelift_codegen=warn` is unambiguous, because a longer
/// target always wins — so asking for one module back does not un-quiet the
/// rest of the crate.
fn log_directives(rust_log: Option<&str>) -> String {
    let requested = match rust_log {
        Some(value) if !value.trim().is_empty() => value,
        _ => DEFAULT_DIRECTIVES,
    };

    let spoken_for: Vec<&str> = requested.split(',').filter_map(directive_target).collect();

    let mut directives: Vec<String> = QUIET_TARGETS
        .iter()
        .filter(|target| !spoken_for.contains(*target))
        .map(|target| format!("{target}={QUIET_LEVEL}"))
        .collect();

    directives.push(requested.to_owned());
    directives.join(",")
}

fn setup() -> EyreResult<()> {
    let directives = log_directives(var("RUST_LOG").ok().as_deref());

    registry()
        .with(EnvFilter::builder().parse(directives)?)
        .with(layer())
        .init();

    color_eyre::install()?;

    // Must be called after color_eyre::install() to chain to its panic handler
    setup_panic_hook();

    init_global_runtime()?;

    Ok(())
}

/// Sets up a custom panic hook that logs structured panic information.
///
/// This hook captures and logs the panic message, thread name, source location,
/// and backtrace before delegating to the previous panic handler. This provides
/// better crash diagnostics for investigation.
///
/// # Note
///
/// - Backtraces are always captured regardless of `RUST_BACKTRACE` setting to
///   ensure crash diagnostics are available in all environments.
/// - Panic messages are logged as-is. Avoid including sensitive data (tokens,
///   passwords, keys) in panic messages as they will appear in logs.
fn setup_panic_hook() {
    let prev_hook = take_hook();

    set_hook(Box::new(move |panic_info| {
        let message = panic_info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| {
                panic_info
                    .payload()
                    .downcast_ref::<String>()
                    .map(String::as_str)
            })
            .unwrap_or("<no message>");

        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("<unnamed>");

        let (file, line, column) = panic_info
            .location()
            .map(|loc| (loc.file(), loc.line(), loc.column()))
            .unwrap_or(("<unknown>", 0, 0));

        let backtrace = Backtrace::force_capture();

        tracing::error!(
            panic.message = %message,
            panic.thread = %thread_name,
            panic.file = %file,
            panic.line = %line,
            panic.column = %column,
            panic.backtrace = %backtrace,
            "Application panic occurred"
        );

        prev_hook(panic_info);
    }));
}

#[cfg(test)]
mod tests {
    use std::io::{Result as IoResult, Write};
    use std::sync::{Arc, Mutex};

    use super::{log_directives, DEFAULT_DIRECTIVES, QUIET_TARGETS};
    use tracing_subscriber::fmt::layer;
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{registry, EnvFilter};

    /// The resolved filter, as `EnvFilter` itself reports it.
    ///
    /// Assertions run against this rather than against emitted output. Emission
    /// depends on process-global state — `tracing` caches a per-callsite decision
    /// the first time a callsite runs, and the max-level hint is shared — so tests
    /// that emit through the same callsites are order- and parallelism-sensitive,
    /// which is how the previous version of this suite passed locally and failed
    /// in CI. `EnvFilter`'s own `Display` is a pure function of the directives.
    fn resolved(directives: &str) -> String {
        EnvFilter::builder()
            .parse(directives)
            .expect("directives must parse")
            .to_string()
    }

    #[test]
    fn a_blanket_level_keeps_the_quieted_targets_quiet() {
        let filter = resolved(&log_directives(Some("debug")));

        for target in QUIET_TARGETS {
            assert!(
                filter.contains(&format!("{target}=warn")),
                "{target} must be held at warn under a blanket debug, got: {filter}"
            );
        }
        assert!(
            filter.contains("debug"),
            "the blanket level must survive, got: {filter}"
        );
    }

    /// The bug this replaces: the quiet prefix and the operator's own directive
    /// both named the same target, and which one won was left to `EnvFilter`.
    #[test]
    fn naming_a_target_replaces_our_directive_rather_than_competing_with_it() {
        for target in QUIET_TARGETS {
            let directives = log_directives(Some(&format!("{target}=debug")));

            assert!(
                !directives.contains(&format!("{target}=warn")),
                "{target} must not appear twice with two levels, got: {directives}"
            );
            assert_eq!(
                directives.matches(target).count(),
                1,
                "{target} must appear exactly once, got: {directives}"
            );
            assert!(
                resolved(&directives).contains(&format!("{target}=debug")),
                "{target} must end up at debug, got: {}",
                resolved(&directives)
            );
        }
    }

    /// Asking for one module back must not un-quiet the whole crate: a longer
    /// target always wins in `EnvFilter`, so both directives can coexist.
    #[test]
    fn a_more_specific_target_is_added_without_dropping_the_quieting() {
        let directives = log_directives(Some("cranelift_codegen::context=debug"));

        assert!(
            directives.contains("cranelift_codegen=warn"),
            "the crate-wide quieting must stay, got: {directives}"
        );

        let filter = resolved(&directives);
        assert!(
            filter.contains("cranelift_codegen::context=debug"),
            "the requested module must be at debug, got: {filter}"
        );
        assert!(
            filter.contains("cranelift_codegen=warn"),
            "the rest of the crate must stay at warn, got: {filter}"
        );
    }

    #[test]
    fn an_unset_or_blank_rust_log_falls_back_to_the_defaults() {
        for unset in [None, Some(""), Some("   ")] {
            let directives = log_directives(unset);

            assert!(
                directives.ends_with(DEFAULT_DIRECTIVES),
                "{unset:?} must fall back to the defaults, got: {directives}"
            );
            for target in QUIET_TARGETS {
                assert!(
                    directives.contains(&format!("{target}=warn")),
                    "{target} must be quiet by default too, got: {directives}"
                );
            }
        }
    }

    /// One emission test, kept because the levels above are only worth anything
    /// if they actually gate output. It is a single test with one callsite per
    /// case, so no callsite's cached decision can leak between cases, and it does
    /// not race the tests above because they never emit.
    #[test]
    fn the_levels_gate_real_output() {
        struct Buffer(Arc<Mutex<Vec<u8>>>);

        impl Write for Buffer {
            fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
                self.0
                    .lock()
                    .expect("log buffer poisoned")
                    .extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> IoResult<()> {
                Ok(())
            }
        }

        fn capture(directives: &str, emit: impl FnOnce()) -> String {
            let buffer = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&buffer);

            let subscriber = registry()
                .with(
                    EnvFilter::builder()
                        .parse(directives)
                        .expect("directives must parse"),
                )
                .with(
                    layer()
                        .with_ansi(false)
                        .with_writer(move || Buffer(Arc::clone(&sink))),
                );

            tracing::subscriber::with_default(subscriber, emit);

            let bytes = buffer.lock().expect("log buffer poisoned").clone();
            String::from_utf8(bytes).expect("formatted output must be utf-8")
        }

        // Under a blanket debug: the compiler's chatter is gone, a warning from
        // the same target still arrives, and the node's own lines are untouched.
        let output = capture(&log_directives(Some("debug")), || {
            tracing::debug!(target: "cranelift_codegen::context", "quieted chatter");
            tracing::warn!(target: "cranelift_codegen::context", "quieted complaint");
            tracing::debug!(target: "libp2p_gossipsub::behaviour", "HEARTBEAT: Mesh low");
            tracing::info!(target: "calimero_node::sync", "performing interval sync");
        });

        assert!(!output.contains("quieted chatter"), "got: {output}");
        assert!(output.contains("quieted complaint"), "got: {output}");
        assert!(output.contains("HEARTBEAT: Mesh low"), "got: {output}");
        assert!(output.contains("performing interval sync"), "got: {output}");

        // Naming the target opts its debug output back in. A distinct callsite
        // from the one above, on purpose.
        let output = capture(&log_directives(Some("cranelift_codegen=debug")), || {
            tracing::debug!(target: "cranelift_codegen::context", "requested chatter");
        });

        assert!(output.contains("requested chatter"), "got: {output}");
    }
}
