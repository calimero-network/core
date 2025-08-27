// Import types needed for Report implementations
use calimero_server_primitives::admin::{
    CreateAliasResponse, DeleteAliasResponse, ListAliasesResponse, LookupAliasResponse,
};
use clap::ValueEnum;
use color_eyre::owo_colors::OwoColorize;
use comfy_table::{Cell, Color, Table};
use serde::Serialize;

// Import the response types from mero_client
use crate::mero_client::{
    BlobDeleteResponse, BlobInfoResponse, BlobListResponse, ResolveResponse, ResolveResponseValue,
};

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

#[derive(Clone, Debug, Serialize)]
pub struct InfoLine<'a>(pub &'a str);

impl Report for InfoLine<'_> {
    fn report(&self) {
        println!("{} {}", "[INFO]".green(), self.0);
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ErrorLine<'a>(pub &'a str);

impl Report for ErrorLine<'_> {
    fn report(&self) {
        println!("{} {}", "[ERROR]".red(), self.0);
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct WarnLine<'a>(pub &'a str);

impl Report for WarnLine<'_> {
    fn report(&self) {
        println!("{} {}", "[WARN]".yellow(), self.0);
    }
}

// Blob-related Report implementations
impl Report for BlobDeleteResponse {
    fn report(&self) {
        if self.deleted {
            println!("Successfully deleted blob '{}'", self.blob_id);
        } else {
            println!(
                "Failed to delete blob '{}' (blob may not exist)",
                self.blob_id
            );
        }
    }
}

impl Report for BlobListResponse {
    fn report(&self) {
        if self.data.blobs.is_empty() {
            println!("No blobs found");
        } else {
            let mut table = Table::new();
            let _ = table.set_header(vec![
                Cell::new("Blob ID").fg(Color::Blue),
                Cell::new("Size").fg(Color::Blue),
            ]);
            for blob in &self.data.blobs {
                let _ = table.add_row(vec![
                    blob.blob_id.to_string(),
                    format!("{} bytes", blob.size),
                ]);
            }
            println!("{table}");
        }
    }
}

impl Report for BlobInfoResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Blob ID").fg(Color::Blue),
            Cell::new("Size (bytes)").fg(Color::Blue),
            Cell::new("MIME Type").fg(Color::Blue),
            Cell::new("Hash").fg(Color::Blue),
        ]);

        let _ = table.add_row(vec![
            &self.data.blob_id.to_string(),
            &self.data.size.to_string(),
            &self.data.mime_type,
            &hex::encode(self.data.hash),
        ]);

        println!("{table}");
    }
}

// Alias-related Report implementations
impl Report for CreateAliasResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Alias Created").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully created alias"]);
        println!("{table}");
    }
}

impl Report for DeleteAliasResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Alias Deleted").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully deleted alias"]);
        println!("{table}");
    }
}

impl<T: std::fmt::Display> Report for ListAliasesResponse<T> {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Value").fg(Color::Blue),
            Cell::new("Alias").fg(Color::Blue),
        ]);

        for (alias, value) in &self.data {
            let _ = table.add_row(vec![
                Cell::new(value.to_string()),
                Cell::new(alias.as_str()),
            ]);
        }

        println!("{table}");
    }
}

impl<T: std::fmt::Display> Report for LookupAliasResponse<T> {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Alias Lookup").fg(Color::Blue)]);

        match &self.data.value {
            Some(value) => {
                let _ = table.add_row(vec!["Status", "Found"]);
                let _ = table.add_row(vec!["Value", &value.to_string()]);
            }
            None => {
                let _ = table.add_row(vec!["Status", "Not Found"]);
            }
        }
        println!("{table}");
    }
}

impl<T: std::fmt::Display> Report for ResolveResponse<T> {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Alias Resolution").fg(Color::Blue)]);
        let _ = table.add_row(vec!["Alias", self.alias().as_str()]);

        match self.value_enum() {
            Some(ResolveResponseValue::Lookup(value)) => {
                let _ = table.add_row(vec!["Type", "Lookup"]);
                value.report();
            }
            Some(ResolveResponseValue::Parsed(value)) => {
                let _ = table.add_row(vec!["Type", "Direct"]);
                let _ = table.add_row(vec!["Value", &value.to_string()]);
            }
            None => {
                let _ = table.add_row(vec!["Status", "Not Resolved"]);
            }
        }
        println!("{table}");
    }
}
