// Core output module
pub mod aliases;
pub mod applications;
pub mod blobs;
pub mod common;
pub mod contexts;
pub mod proposals;

// Re-export common types
pub use blobs::{BlobDownloadResponse, BlobUploadResponse};
use clap::ValueEnum;
pub use common::{ErrorLine, InfoLine, WarnLine};
// Re-export types from other modules
pub use proposals::ProposalDetailsResponse;
use serde::Serialize;

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

    pub fn write<T: Serialize + Report>(&self, value: &T) {
        match self.format {
            Format::Json => match serde_json::to_string(&value) {
                Ok(json) => println!("{json}"),
                Err(err) => eprintln!("Failed to serialize to JSON: {err}"),
            },
            Format::Human => value.report(),
        }
    }
}
