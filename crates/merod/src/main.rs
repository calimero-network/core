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
#[cfg(unix)]
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
/// Prepended rather than appended: a target-specific directive beats the blanket
/// level regardless of order, while an explicit `RUST_LOG=<target>=debug` names
/// the same target and so wins by coming later. Both halves of that, and the
/// warn-still-gets-through property, are pinned by the tests below.
///
/// Deliberately NOT included, despite being the next largest: `libp2p_gossipsub`
/// (12.1%) is mesh formation — `HEARTBEAT: Mesh low`, `Updating mesh`, peer
/// counts — which is the evidence mesh-join and mesh-scoring bugs are diagnosed
/// from, and `libp2p_swarm`'s connection-closed lines are how a peer's death is
/// spotted at all.
const QUIET_TARGETS: &str =
    "cranelift_codegen=warn,multistream_select=warn,libp2p_core::transport::choice=warn";

/// Builds the tracing filter from `RUST_LOG`, or the default when it is unset
/// or blank.
fn log_directives(rust_log: Option<&str>) -> String {
    match rust_log {
        Some(value) if !value.trim().is_empty() => format!("{QUIET_TARGETS},{value}"),
        _ => format!("{QUIET_TARGETS},{DEFAULT_DIRECTIVES}"),
    }
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

    use super::{log_directives, DEFAULT_DIRECTIVES};
    use tracing_subscriber::fmt::layer;
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{registry, EnvFilter};

    /// A `MakeWriter` sink that keeps the formatted output in memory.
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

    /// Emits one line per quieted target at both `debug` and `warn`, plus a node
    /// line, and returns whatever `directives` let through.
    ///
    /// Asserting on real emitted output rather than on the directive string is
    /// the point: the precedence being pinned here is `EnvFilter`'s, not ours.
    /// The targets are literals because a callsite's metadata is static — they
    /// cannot be parameterised at runtime.
    fn emit_under(directives: &str) -> String {
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

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!(target: "cranelift_codegen::context", "compiler chatter");
            tracing::debug!(target: "multistream_select::dialer_select", "negotiation chatter");
            tracing::debug!(target: "libp2p_core::transport::choice", "transport chatter");

            tracing::warn!(target: "cranelift_codegen::context", "compiler complaint");
            tracing::warn!(target: "multistream_select::dialer_select", "negotiation complaint");
            tracing::warn!(target: "libp2p_core::transport::choice", "transport complaint");

            // Kept loud: mesh formation and connection teardown are how mesh-join
            // bugs and a peer's death are diagnosed.
            tracing::debug!(target: "libp2p_gossipsub::behaviour", "HEARTBEAT: Mesh low");
            tracing::debug!(target: "libp2p_swarm", "Connection closed with error");
            tracing::debug!(target: "libp2p_core::upgrade::apply", "Failed to upgrade outbound stream");

            tracing::info!(target: "calimero_node::sync", "performing interval sync");
        });

        let bytes = buffer.lock().expect("log buffer poisoned").clone();
        String::from_utf8(bytes).expect("formatted output must be utf-8")
    }

    const CHATTER: [&str; 3] = [
        "compiler chatter",
        "negotiation chatter",
        "transport chatter",
    ];

    #[test]
    fn blanket_debug_drops_every_quieted_target() {
        let output = emit_under(&log_directives(Some("debug")));

        for chatter in CHATTER {
            assert!(
                !output.contains(chatter),
                "{chatter:?} must stay quiet under a blanket debug, got: {output}"
            );
        }
    }

    #[test]
    fn blanket_debug_still_keeps_everything_worth_reading() {
        let output = emit_under(&log_directives(Some("debug")));

        for kept in [
            "performing interval sync",
            "HEARTBEAT: Mesh low",
            "Connection closed with error",
            "Failed to upgrade outbound stream",
        ] {
            assert!(
                output.contains(kept),
                "{kept:?} must survive the quieting, got: {output}"
            );
        }
    }

    #[test]
    fn a_quieted_target_can_still_raise_a_warning() {
        let output = emit_under(&log_directives(Some("debug")));

        for complaint in [
            "compiler complaint",
            "negotiation complaint",
            "transport complaint",
        ] {
            assert!(
                output.contains(complaint),
                "{complaint:?} must get through — these are turned down to warn, \
                 not off, got: {output}"
            );
        }
    }

    #[test]
    fn an_explicit_directive_opts_a_quieted_target_back_in() {
        for (directive, chatter) in [
            ("cranelift_codegen=debug", "compiler chatter"),
            ("multistream_select=debug", "negotiation chatter"),
            ("libp2p_core::transport::choice=debug", "transport chatter"),
        ] {
            let output = emit_under(&log_directives(Some(directive)));
            assert!(
                output.contains(chatter),
                "{directive:?} must opt {chatter:?} back in, got: {output}"
            );
        }
    }

    #[test]
    fn an_unset_or_blank_rust_log_falls_back_to_the_defaults() {
        for unset in [None, Some(""), Some("   ")] {
            let directives = log_directives(unset);
            assert!(
                directives.ends_with(DEFAULT_DIRECTIVES),
                "{unset:?} must fall back to the defaults, got: {directives}"
            );

            let output = emit_under(&directives);
            assert!(
                output.contains("performing interval sync"),
                "the default filter must show calimero_ info lines, got: {output}"
            );
            for chatter in CHATTER {
                assert!(
                    !output.contains(chatter),
                    "the default filter must not show {chatter:?}, got: {output}"
                );
            }
        }
    }
}
