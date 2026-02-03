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
mod docker;
mod kms;
mod version;

use cli::RootCommand;

#[tokio::main]
async fn main() -> EyreResult<()> {
    setup()?;

    let command = RootCommand::parse();

    version::check_for_update();

    command.run().await
}

fn setup() -> EyreResult<()> {
    let directives = match var("RUST_LOG") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => "merod=info,calimero_=info".to_owned(),
    };

    registry()
        .with(EnvFilter::builder().parse(directives)?)
        .with(layer())
        .init();

    color_eyre::install()?;

    setup_panic_hook();

    init_global_runtime()?;

    Ok(())
}

/// Sets up a custom panic hook that logs structured panic information.
///
/// This hook captures and logs the panic message, thread name, source location,
/// and backtrace before delegating to the previous panic handler. This provides
/// better crash diagnostics for investigation.
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
    use std::panic::catch_unwind;
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::Layer;

    use super::*;

    /// A simple layer that captures log messages for testing
    struct CaptureLayer {
        logs: Arc<Mutex<Vec<String>>>,
    }

    impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut visitor = StringVisitor::default();
            event.record(&mut visitor);
            if let Ok(mut logs) = self.logs.lock() {
                logs.push(visitor.output);
            }
        }
    }

    #[derive(Default)]
    struct StringVisitor {
        output: String,
    }

    impl tracing::field::Visit for StringVisitor {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            use std::fmt::Write;
            let _ = write!(self.output, "{}={:?} ", field.name(), value);
        }

        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            use std::fmt::Write;
            let _ = write!(self.output, "{}=\"{}\" ", field.name(), value);
        }
    }

    #[test]
    fn test_panic_hook_logs_structured_info() {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let capture_layer = CaptureLayer { logs: logs.clone() };

        let subscriber = tracing_subscriber::registry().with(capture_layer);

        tracing::subscriber::with_default(subscriber, || {
            // Install our panic hook
            setup_panic_hook();

            // Trigger a panic and catch it
            let result = catch_unwind(|| {
                panic!("test panic message");
            });

            // Verify the panic was caught
            assert!(result.is_err());
        });

        // Check that our panic hook logged the expected fields
        let captured = logs.lock().unwrap();
        assert!(!captured.is_empty(), "Expected panic to be logged");

        let log_output = &captured[0];
        assert!(
            log_output.contains("panic.message"),
            "Log should contain panic.message field"
        );
        assert!(
            log_output.contains("test panic message"),
            "Log should contain the panic message"
        );
        assert!(
            log_output.contains("panic.thread"),
            "Log should contain panic.thread field"
        );
        assert!(
            log_output.contains("panic.file"),
            "Log should contain panic.file field"
        );
        assert!(
            log_output.contains("panic.line"),
            "Log should contain panic.line field"
        );
        assert!(
            log_output.contains("panic.backtrace"),
            "Log should contain panic.backtrace field"
        );
    }
}
