// Core output module
pub mod aliases;
pub mod applications;
pub mod blobs;
pub mod common;
pub mod contexts;
pub mod groups;
pub mod network;
pub mod tee;

use std::sync::atomic::{AtomicBool, Ordering};

// Re-export common types
pub use blobs::{BlobDownloadResponse, BlobUploadResponse};
use clap::ValueEnum;
pub use common::{ErrorLine, InfoLine, WarnLine};
use serde::Serialize;

/// Set when an `Output::write` fails to render (e.g. JSON serialization error).
/// `main` reads this to exit non-zero instead of masking the failure with a
/// success code.
///
/// Accessed with `Ordering::Relaxed`: this is a standalone flag that guards no
/// other memory, the meaningful read happens once in `main` after the command
/// has fully returned (so the store has long since happened-before it), and a
/// set is monotonic (never cleared). No stronger ordering is needed.
static OUTPUT_FAILED: AtomicBool = AtomicBool::new(false);

/// Whether any output render has failed during this process.
pub fn output_failed() -> bool {
    OUTPUT_FAILED.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum Format {
    Json,
    #[default]
    Human,
}

#[derive(Copy, Clone, Debug, Default)]
pub struct Output {
    format: Format,
}

pub trait Report {
    fn report(&self);
}

impl Output {
    pub const fn new(output_type: Format) -> Self {
        Self {
            format: output_type,
        }
    }

    /// The configured output format (`--output-format`).
    pub const fn format(&self) -> Format {
        self.format
    }

    pub fn write<T: Serialize + Report>(&self, value: &T) {
        match self.format {
            Format::Json => match serde_json::to_string(&value) {
                Ok(json) => println!("{json}"),
                Err(err) => {
                    // Record the failure so the process exits non-zero instead
                    // of reporting success after producing no usable output.
                    OUTPUT_FAILED.store(true, Ordering::Relaxed);
                    eprintln!("Failed to serialize to JSON: {err}");
                }
            },
            Format::Human => value.report(),
        }
    }
}
