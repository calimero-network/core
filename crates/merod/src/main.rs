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
